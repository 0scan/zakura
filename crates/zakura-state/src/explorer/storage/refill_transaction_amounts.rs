//! One-time offline refill for explorer transaction metadata.

use std::{collections::HashMap, time::Instant};

use semver::Version;
use zakura_chain::{
    parameters::Network,
    transaction::Transaction,
    transparent::{OutPoint, Utxo},
};

use crate::{
    config::database_format_version_on_disk,
    constants::{state_database_format_version_in_code, STATE_DATABASE_KIND},
    service::finalized_state::{
        DiskWriteBatch, RawBytes, TransactionLocation, TypedColumnFamily, WriteDisk, ZakuraDb,
    },
    BoxError, Config, IntoDisk, StateInitError,
};

use super::{
    disk_format::{ExplorerSchemaVersion, EXPLORER_TRANSACTION_RECORD_BYTES},
    explorer_transaction_record_with_utxos, EXPLORER_SCHEMA, EXPLORER_TRANSACTION_META_BY_LOC,
};

const EXPLORER_SCHEMA_V1: u32 = 1;
const TRANSACTION_RECORD_V1_BYTES: usize = 72;
const TRANSACTION_RECORD_V2_BYTES: usize = EXPLORER_TRANSACTION_RECORD_BYTES;

/// Controls one explorer transaction amount refill run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RefillTransactionAmountsOptions {
    /// Maximum number of upgraded records written in one RocksDB batch.
    pub batch_size: usize,
    /// Optional record limit for benchmarking or partial refill runs.
    pub limit: Option<u64>,
    /// Calculate and validate records without writing them.
    pub dry_run: bool,
}

impl Default for RefillTransactionAmountsOptions {
    fn default() -> Self {
        Self {
            batch_size: 10_000,
            limit: None,
            dry_run: true,
        }
    }
}

/// Outcome of one explorer transaction amount refill run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RefillTransactionAmountsSummary {
    /// Records inspected after the first legacy record.
    pub scanned_records: u64,
    /// Legacy records calculated during this run.
    pub refilled_records: u64,
    /// Version-two records skipped while locating or processing legacy records.
    pub already_refilled_records: u64,
    /// Whether this run reached the end of the transaction metadata index.
    pub scan_complete: bool,
    /// Whether the explorer schema marker was advanced to version two.
    pub schema_updated: bool,
    /// Whole seconds spent in the refill function.
    pub elapsed_seconds: u64,
}

/// Errors returned by the one-time transaction amount refill.
#[derive(Debug, thiserror::Error)]
pub enum RefillTransactionAmountsError {
    /// A zero-sized write batch would never make progress.
    #[error("refill batch size must be greater than zero")]
    InvalidBatchSize,

    /// The on-disk state format does not match this build.
    #[error(
        "state database format mismatch: on disk {on_disk:?}, running code {in_code}; \
         use a Zakura binary built for the same state format"
    )]
    FormatMismatch {
        /// Version read from disk.
        on_disk: Option<Version>,
        /// Version implemented by this build.
        in_code: Version,
    },

    /// The database format version file could not be read.
    #[error("could not read the state database format version file")]
    UnreadableFormatVersion(#[source] BoxError),

    /// The finalized database could not be opened.
    #[error("could not open the finalized state database")]
    DatabaseOpen(#[from] StateInitError),

    /// The explorer database has no schema marker.
    #[error("the explorer database has no schema marker")]
    MissingExplorerSchema,

    /// The explorer schema is neither the legacy nor target version.
    #[error("unsupported explorer schema version {0}")]
    UnsupportedExplorerSchema(u32),

    /// Historical raw transactions have already been pruned.
    #[error("transaction amounts cannot be refilled after historical raw transactions are pruned")]
    PrunedDatabase,

    /// A transaction metadata value has an unknown byte length.
    #[error("transaction metadata at {location:?} has unexpected length {length}")]
    InvalidRecordLength {
        /// Canonical transaction position.
        location: TransactionLocation,
        /// Encoded metadata length.
        length: usize,
    },

    /// A metadata row has no corresponding raw transaction.
    #[error("raw transaction is missing at {0:?}")]
    MissingRawTransaction(TransactionLocation),

    /// A raw transaction appears at a different location than its metadata row.
    #[error(
        "transaction metadata at {metadata:?} does not match raw transaction at {transaction:?}"
    )]
    LocationMismatch {
        /// Metadata position.
        metadata: TransactionLocation,
        /// Raw transaction position.
        transaction: TransactionLocation,
    },

    /// A transparent input references a transaction without an indexed location.
    #[error("spent transaction location is missing for {0:?}")]
    MissingSpentTransactionLocation(OutPoint),

    /// A transparent input references a transaction without retained raw bytes.
    #[error("spent transaction is missing at {location:?} for {outpoint:?}")]
    MissingSpentTransaction {
        /// The input being resolved.
        outpoint: OutPoint,
        /// The indexed location of its source transaction.
        location: TransactionLocation,
    },

    /// A transparent input references an output absent from its source transaction.
    #[error("spent output is missing for {0:?}")]
    MissingSpentOutput(OutPoint),

    /// RocksDB rejected a write or flush.
    #[error("RocksDB operation failed")]
    RocksDb(#[from] rocksdb::Error),
}

/// Refill transaction amounts and primary endpoints, then advance explorer schema v1 to v2.
///
/// Apply mode opens the primary database and therefore requires the node to be stopped. Dry-run
/// mode opens a read-only secondary and can inspect a running node.
pub fn refill_transaction_amounts(
    config: Config,
    network: &Network,
    options: RefillTransactionAmountsOptions,
) -> Result<RefillTransactionAmountsSummary, RefillTransactionAmountsError> {
    if options.batch_size == 0 {
        return Err(RefillTransactionAmountsError::InvalidBatchSize);
    }

    check_format_version(&config, network)?;
    let started = Instant::now();
    let db = ZakuraDb::new_for_explorer_amount_refill(&config, network, options.dry_run)?;
    if db.is_pruned() {
        return Err(RefillTransactionAmountsError::PrunedDatabase);
    }

    let schema_cf =
        TypedColumnFamily::<(), ExplorerSchemaVersion>::new(db.disk_db(), EXPLORER_SCHEMA)
            .expect("explorer schema column family is registered");
    let schema = schema_cf
        .zs_get(&())
        .ok_or(RefillTransactionAmountsError::MissingExplorerSchema)?;
    if schema == ExplorerSchemaVersion::CURRENT {
        return Ok(RefillTransactionAmountsSummary {
            scanned_records: 0,
            refilled_records: 0,
            already_refilled_records: 0,
            scan_complete: true,
            schema_updated: false,
            elapsed_seconds: started.elapsed().as_secs(),
        });
    }
    if schema.0 != EXPLORER_SCHEMA_V1 {
        return Err(RefillTransactionAmountsError::UnsupportedExplorerSchema(
            schema.0,
        ));
    }

    let metadata_cf = TypedColumnFamily::<TransactionLocation, RawBytes>::new(
        db.disk_db(),
        EXPLORER_TRANSACTION_META_BY_LOC,
    )
    .expect("explorer transaction metadata column family is registered");
    let transaction_cf =
        TypedColumnFamily::<TransactionLocation, Transaction>::new(db.disk_db(), "tx_by_loc")
            .expect("raw transaction column family is registered");

    let mut already_refilled_records = 0_u64;
    let mut first_legacy_location = None;
    for (location, record) in metadata_cf.zs_forward_range_iter(..) {
        match record.raw_bytes().len() {
            TRANSACTION_RECORD_V1_BYTES => {
                first_legacy_location = Some(location);
                break;
            }
            TRANSACTION_RECORD_V2_BYTES => {
                already_refilled_records = already_refilled_records
                    .checked_add(1)
                    .expect("transaction count fits in u64");
            }
            length => {
                return Err(RefillTransactionAmountsError::InvalidRecordLength {
                    location,
                    length,
                });
            }
        }
    }

    let Some(first_legacy_location) = first_legacy_location else {
        let schema_updated = if options.dry_run {
            false
        } else {
            let mut batch = DiskWriteBatch::new();
            let _ = schema_cf
                .with_batch_for_writing(&mut batch)
                .zs_insert(&(), &ExplorerSchemaVersion::CURRENT);
            db.write_batch(batch)?;
            db.flush()?;
            true
        };
        return Ok(RefillTransactionAmountsSummary {
            scanned_records: 0,
            refilled_records: 0,
            already_refilled_records,
            scan_complete: true,
            schema_updated,
            elapsed_seconds: started.elapsed().as_secs(),
        });
    };

    let mut transaction_iter = transaction_cf
        .zs_forward_range_iter(first_legacy_location..)
        .peekable();
    let mut batch = DiskWriteBatch::new();
    let metadata_handle = db
        .disk_db()
        .cf_handle(EXPLORER_TRANSACTION_META_BY_LOC)
        .expect("explorer transaction metadata column family is registered");
    let mut scanned_records = 0_u64;
    let mut refilled_records = 0_u64;
    let mut pending_writes = 0_usize;
    let mut scan_complete = true;

    for (location, record) in metadata_cf.zs_forward_range_iter(first_legacy_location..) {
        if options.limit.is_some_and(|limit| refilled_records >= limit) {
            scan_complete = false;
            break;
        }

        scanned_records = scanned_records
            .checked_add(1)
            .expect("transaction count fits in u64");
        let (transaction_location, transaction) =
            transaction_iter
                .next()
                .ok_or(RefillTransactionAmountsError::MissingRawTransaction(
                    location,
                ))?;
        if transaction_location != location {
            return Err(RefillTransactionAmountsError::LocationMismatch {
                metadata: location,
                transaction: transaction_location,
            });
        }

        match record.raw_bytes().len() {
            TRANSACTION_RECORD_V2_BYTES => {
                already_refilled_records = already_refilled_records
                    .checked_add(1)
                    .expect("transaction count fits in u64");
            }
            TRANSACTION_RECORD_V1_BYTES => {
                let spent_utxos = spent_utxos(&db, &transaction_cf, &transaction)?;
                let upgraded = explorer_transaction_record_with_utxos(
                    &transaction,
                    location.index.as_usize(),
                    network,
                    &spent_utxos,
                );
                if !options.dry_run {
                    batch.zs_insert(
                        &metadata_handle,
                        location,
                        RawBytes::new_raw_bytes(upgraded.as_bytes().to_vec()),
                    );
                    pending_writes += 1;
                }
                refilled_records = refilled_records
                    .checked_add(1)
                    .expect("transaction count fits in u64");
            }
            length => {
                return Err(RefillTransactionAmountsError::InvalidRecordLength {
                    location,
                    length,
                });
            }
        }

        if pending_writes == options.batch_size {
            db.write_batch(batch)?;
            tracing::info!(
                refilled_records,
                location = ?location,
                "refilled explorer transaction amounts"
            );
            batch = DiskWriteBatch::new();
            pending_writes = 0;
        }
    }

    let schema_updated = if options.dry_run {
        false
    } else {
        if scan_complete {
            let _ = schema_cf
                .with_batch_for_writing(&mut batch)
                .zs_insert(&(), &ExplorerSchemaVersion::CURRENT);
        }
        if pending_writes > 0 || scan_complete {
            db.write_batch(batch)?;
        }
        db.flush()?;
        scan_complete
    };

    Ok(RefillTransactionAmountsSummary {
        scanned_records,
        refilled_records,
        already_refilled_records,
        scan_complete,
        schema_updated,
        elapsed_seconds: started.elapsed().as_secs(),
    })
}

fn check_format_version(
    config: &Config,
    network: &Network,
) -> Result<(), RefillTransactionAmountsError> {
    let in_code = state_database_format_version_in_code();
    let on_disk =
        database_format_version_on_disk(config, STATE_DATABASE_KIND, in_code.major, network)
            .map_err(RefillTransactionAmountsError::UnreadableFormatVersion)?;
    if on_disk.as_ref() != Some(&in_code) {
        return Err(RefillTransactionAmountsError::FormatMismatch { on_disk, in_code });
    }

    Ok(())
}

fn spent_utxos(
    db: &ZakuraDb,
    transaction_cf: &TypedColumnFamily<'_, TransactionLocation, Transaction>,
    transaction: &Transaction,
) -> Result<HashMap<OutPoint, Utxo>, RefillTransactionAmountsError> {
    transaction
        .spent_outpoints()
        .map(|outpoint| {
            let source_location = db
                .transaction_location(outpoint.hash)
                .ok_or(RefillTransactionAmountsError::MissingSpentTransactionLocation(outpoint))?;
            let source_transaction = transaction_cf.zs_get(&source_location).ok_or(
                RefillTransactionAmountsError::MissingSpentTransaction {
                    outpoint,
                    location: source_location,
                },
            )?;
            let output_index = usize::try_from(outpoint.index)
                .expect("u32 output indexes fit in usize on supported targets");
            let output = source_transaction
                .outputs()
                .get(output_index)
                .cloned()
                .ok_or(RefillTransactionAmountsError::MissingSpentOutput(outpoint))?;
            let utxo = Utxo::from_location(
                output,
                source_location.height,
                source_location.index.as_usize(),
            );

            Ok((outpoint, utxo))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use zakura_chain::{
        amount::{Amount, NonNegative},
        block::Height,
        parameters::{testnet::RegtestParameters, NetworkKind},
        transaction::{LockTime, TransactionValueEndpoint},
        transparent::{Address, Input, Output, Script},
    };

    use crate::service::finalized_state::STATE_COLUMN_FAMILIES_IN_CODE;

    use super::*;

    #[test]
    fn refills_legacy_records_and_advances_the_schema() {
        let cache_dir = tempfile::tempdir().expect("temporary cache directory is created");
        let config = Config {
            cache_dir: cache_dir.path().to_path_buf(),
            ..Config::default()
        };
        let network = Network::new_regtest(RegtestParameters::default());
        let source_address = Address::from_script_hash(NetworkKind::Testnet, [1; 20]);
        let destination_address = Address::from_script_hash(NetworkKind::Testnet, [2; 20]);
        let source_height = Height(1);
        let source_transaction = Transaction::V1 {
            inputs: vec![Input::Coinbase {
                height: source_height,
                data: vec![0; 8],
                sequence: u32::MAX,
            }],
            outputs: vec![Output::new(
                Amount::<NonNegative>::try_from(123_456_i64).expect("test amount is in range"),
                source_address.script(),
            )],
            lock_time: LockTime::unlocked(),
        };
        let source_hash = source_transaction.hash();
        let spent_outpoint = OutPoint::from_usize(source_hash, 0);
        let transaction = Transaction::V1 {
            inputs: vec![Input::PrevOut {
                outpoint: spent_outpoint,
                unlock_script: Script::new(&[]),
                sequence: u32::MAX,
            }],
            outputs: vec![Output::new(
                Amount::<NonNegative>::try_from(120_000_i64).expect("test amount is in range"),
                destination_address.script(),
            )],
            lock_time: LockTime::unlocked(),
        };
        let source_location = TransactionLocation::from_usize(source_height, 0);
        let location = TransactionLocation::from_usize(Height(2), 1);

        {
            let db = ZakuraDb::new(
                &config,
                STATE_DATABASE_KIND,
                &state_database_format_version_in_code(),
                &network,
                false,
                STATE_COLUMN_FAMILIES_IN_CODE
                    .iter()
                    .map(ToString::to_string),
                false,
            )
            .expect("test database opens");
            let schema_handle = db
                .disk_db()
                .cf_handle(EXPLORER_SCHEMA)
                .expect("schema column family exists");
            let metadata_handle = db
                .disk_db()
                .cf_handle(EXPLORER_TRANSACTION_META_BY_LOC)
                .expect("metadata column family exists");
            let transaction_handle = db
                .disk_db()
                .cf_handle("tx_by_loc")
                .expect("transaction column family exists");
            let location_handle = db
                .disk_db()
                .cf_handle("tx_loc_by_hash")
                .expect("transaction location column family exists");
            let mut batch = DiskWriteBatch::new();
            batch.zs_insert(
                &schema_handle,
                (),
                ExplorerSchemaVersion(EXPLORER_SCHEMA_V1),
            );
            batch.zs_insert(
                &metadata_handle,
                source_location,
                RawBytes::new_raw_bytes(vec![0; TRANSACTION_RECORD_V1_BYTES]),
            );
            batch.zs_insert(
                &metadata_handle,
                location,
                RawBytes::new_raw_bytes(vec![0; TRANSACTION_RECORD_V1_BYTES]),
            );
            batch.zs_insert(
                &transaction_handle,
                source_location,
                source_transaction.clone(),
            );
            batch.zs_insert(&transaction_handle, location, transaction);
            batch.zs_insert(&location_handle, source_hash, source_location);
            db.write_batch(batch).expect("legacy test rows are written");
            db.flush().expect("legacy test rows are flushed");
        }

        let summary = refill_transaction_amounts(
            config.clone(),
            &network,
            RefillTransactionAmountsOptions {
                dry_run: false,
                ..Default::default()
            },
        )
        .expect("legacy transaction amount is refilled");
        assert_eq!(summary.refilled_records, 2);
        assert!(summary.scan_complete);
        assert!(summary.schema_updated);

        let db = ZakuraDb::new(
            &config,
            STATE_DATABASE_KIND,
            &state_database_format_version_in_code(),
            &network,
            true,
            STATE_COLUMN_FAMILIES_IN_CODE
                .iter()
                .map(ToString::to_string),
            false,
        )
        .expect("upgraded test database opens normally");
        let record = db
            .explorer_transaction_record(location)
            .expect("upgraded metadata row exists");
        assert_eq!(record.transparent_output_total_zat, 120_000);
        assert_eq!(
            record.primary_from,
            Some(TransactionValueEndpoint::Transparent(source_address))
        );
        assert_eq!(
            record.primary_to,
            Some(TransactionValueEndpoint::Transparent(destination_address))
        );
        assert_eq!(
            db.explorer_transaction_record(source_location)
                .expect("upgraded source metadata row exists")
                .primary_from,
            Some(TransactionValueEndpoint::Coinbase)
        );
    }
}
