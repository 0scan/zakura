//! One-time offline refill for explorer transaction metadata.

mod prepared_refill;
mod source_transactions;

use std::{ops::Bound, path::PathBuf, time::Instant};

use rayon::ThreadPoolBuilder;
use semver::Version;
use zakura_chain::{
    parameters::Network,
    transaction::{self, PrimaryValueEndpointsError, Transaction},
    transparent::OutPoint,
};

use crate::{
    config::database_format_version_on_disk,
    constants::{state_database_format_version_in_code, STATE_DATABASE_KIND},
    service::finalized_state::{
        DiskWriteBatch, RawBytes, TransactionLocation, TypedColumnFamily, WriteDisk, ZakuraDb,
    },
    BoxError, Config, StateInitError,
};

use super::{
    disk_format::{
        ExplorerSchemaVersion, EXPLORER_TRANSACTION_RECORD_BYTES,
        EXPLORER_TRANSACTION_RECORD_V1_BYTES,
    },
    EXPLORER_SCHEMA, EXPLORER_TRANSACTION_META_BY_LOC,
};
use source_transactions::{
    upgrade_transaction_entries, LegacyTransactionEntry, SourceOutputCache,
    UpgradedTransactionEntry,
};

pub use prepared_refill::{
    apply_prepared_transaction_amount_refill, prepare_transaction_amount_refill,
    prepared_transaction_amount_refill_partial_path, resume_prepared_transaction_amount_refill,
    AppliedPreparedTransactionAmountRefillSummary, PreparedTransactionAmountRefillSummary,
};

const EXPLORER_SCHEMA_V1: u32 = 1;
const DEFAULT_SOURCE_CACHE_ENTRIES: usize = 10_000;
const TRANSACTION_RECORD_V2_BYTES: usize = EXPLORER_TRANSACTION_RECORD_BYTES;

/// Controls one explorer transaction amount refill run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RefillTransactionAmountsOptions {
    /// Maximum number of upgraded records written in one RocksDB batch.
    pub batch_size: usize,
    /// Number of worker threads used for historical transaction reads and endpoint derivation.
    pub workers: usize,
    /// Maximum number of decoded historical source outputs retained between batches.
    pub source_cache_entries: usize,
    /// Optional record limit for benchmarking or partial refill runs.
    pub limit: Option<u64>,
    /// Calculate and validate records without writing them.
    pub dry_run: bool,
}

impl Default for RefillTransactionAmountsOptions {
    fn default() -> Self {
        Self {
            batch_size: 10_000,
            workers: default_worker_count(),
            source_cache_entries: DEFAULT_SOURCE_CACHE_ENTRIES,
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

    /// A zero-sized worker pool cannot process refill batches.
    #[error("refill worker count must be greater than zero")]
    InvalidWorkerCount,

    /// The dedicated refill worker pool could not be created.
    #[error("could not create the refill worker pool")]
    ThreadPoolBuild(#[from] rayon::ThreadPoolBuildError),

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
    #[error("spent transaction location is missing for {0}")]
    MissingSpentTransactionLocation(transaction::Hash),

    /// A transparent input references a transaction without retained raw bytes.
    #[error("spent transaction {hash} is missing at {location:?}")]
    MissingSpentTransaction {
        /// The transaction being resolved.
        hash: transaction::Hash,
        /// The indexed location of its source transaction.
        location: TransactionLocation,
    },

    /// A transparent input references an output absent from its source transaction.
    #[error("spent output is missing for {0:?}")]
    MissingSpentOutput(OutPoint),

    /// Transparent output values exceeded the supported integer range.
    #[error("transparent output total exceeds i64 at {0:?}")]
    OutputTotalOverflow(TransactionLocation),

    /// Primary endpoints could not be derived from retained transaction data.
    #[error("could not derive primary endpoints at {location:?}")]
    PrimaryEndpoints {
        /// Canonical transaction position.
        location: TransactionLocation,
        /// Endpoint derivation failure.
        #[source]
        source: PrimaryValueEndpointsError,
    },

    /// RocksDB rejected a write or flush.
    #[error("RocksDB operation failed")]
    RocksDb(#[from] rocksdb::Error),

    /// The prepared refill artifact could not be read or written.
    #[error("prepared refill artifact I/O failed at {path}")]
    PreparedFileIo {
        /// Artifact path involved in the failed operation.
        path: PathBuf,
        /// Underlying filesystem error.
        #[source]
        source: std::io::Error,
    },

    /// Refusing to replace an existing prepared refill artifact.
    #[error("prepared refill artifact already exists at {0}")]
    PreparedFileAlreadyExists(PathBuf),

    /// A prepared refill artifact is incomplete, corrupt, or from an unsupported format.
    #[error("invalid prepared refill artifact at {path}: {reason}")]
    InvalidPreparedFile {
        /// Artifact path that failed validation.
        path: PathBuf,
        /// Concise validation failure.
        reason: String,
    },

    /// The artifact was prepared from a different network kind.
    #[error(
        "prepared refill network mismatch: artifact is {prepared:?}, requested database is {requested:?}"
    )]
    PreparedNetworkMismatch {
        /// Network encoded in the artifact.
        prepared: zakura_chain::parameters::NetworkKind,
        /// Network requested for the target database.
        requested: zakura_chain::parameters::NetworkKind,
    },

    /// The target database does not contain the artifact's source chain tip.
    #[error(
        "prepared refill source tip mismatch at {height:?}: artifact has {prepared_hash}, database has {database_hash:?}"
    )]
    PreparedTipMismatch {
        /// Height captured when the artifact was prepared.
        height: zakura_chain::block::Height,
        /// Finalized block hash captured at that height.
        prepared_hash: zakura_chain::block::Hash,
        /// Finalized block hash currently stored at that height, if any.
        database_hash: Option<zakura_chain::block::Hash>,
    },

    /// Preparing an artifact requires a non-empty finalized chain.
    #[error("cannot prepare a transaction refill artifact from an empty database")]
    EmptyDatabase,

    /// A prepared artifact must contain every transaction row in its source snapshot.
    #[error(
        "cannot prepare from a partially refilled database: found {0} version-two transaction records"
    )]
    PartiallyRefilledDatabase(u64),

    /// A prepared artifact is unnecessary and incomplete after the schema marker advances.
    #[error("cannot prepare a transaction refill artifact after the explorer schema reached version two")]
    ExplorerSchemaAlreadyCurrent,
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
    refill_transaction_amounts_from(config, network, options, None)
}

fn refill_transaction_amounts_from(
    config: Config,
    network: &Network,
    options: RefillTransactionAmountsOptions,
    start_after: Option<TransactionLocation>,
) -> Result<RefillTransactionAmountsSummary, RefillTransactionAmountsError> {
    let started = Instant::now();
    let worker_pool = refill_worker_pool(options)?;
    let db = open_refill_database(&config, network, options.dry_run)?;
    if explorer_schema_is_current(&db)? {
        return Ok(RefillTransactionAmountsSummary {
            scanned_records: 0,
            refilled_records: 0,
            already_refilled_records: 0,
            scan_complete: true,
            schema_updated: false,
            elapsed_seconds: started.elapsed().as_secs(),
        });
    }

    let metadata_handle = db
        .disk_db()
        .cf_handle(EXPLORER_TRANSACTION_META_BY_LOC)
        .expect("explorer transaction metadata column family is registered");
    let calculation = calculate_refill_records(
        &db,
        network,
        options,
        &worker_pool,
        start_after,
        None,
        |upgraded_entries| {
            if options.dry_run || upgraded_entries.is_empty() {
                return Ok(());
            }

            let mut batch = DiskWriteBatch::new();
            for upgraded in upgraded_entries {
                batch.zs_insert(&metadata_handle, upgraded.location, upgraded.record);
            }
            db.write_batch(batch)?;
            Ok(())
        },
    )?;

    let schema_updated = if options.dry_run {
        false
    } else {
        if calculation.scan_complete {
            let schema_cf =
                TypedColumnFamily::<(), ExplorerSchemaVersion>::new(db.disk_db(), EXPLORER_SCHEMA)
                    .expect("explorer schema column family is registered");
            let mut batch = DiskWriteBatch::new();
            let _ = schema_cf
                .with_batch_for_writing(&mut batch)
                .zs_insert(&(), &ExplorerSchemaVersion::CURRENT);
            db.write_batch(batch)?;
        }
        db.flush()?;
        calculation.scan_complete
    };

    Ok(RefillTransactionAmountsSummary {
        scanned_records: calculation.scanned_records,
        refilled_records: calculation.refilled_records,
        already_refilled_records: calculation.already_refilled_records,
        scan_complete: calculation.scan_complete,
        schema_updated,
        elapsed_seconds: started.elapsed().as_secs(),
    })
}

struct RefillCalculationSummary {
    scanned_records: u64,
    refilled_records: u64,
    already_refilled_records: u64,
    scan_complete: bool,
}

fn refill_worker_pool(
    options: RefillTransactionAmountsOptions,
) -> Result<rayon::ThreadPool, RefillTransactionAmountsError> {
    validate_refill_options(options)?;

    Ok(ThreadPoolBuilder::new()
        .num_threads(options.workers)
        .thread_name(|index| format!("explorer-refill-{index}"))
        .build()?)
}

fn validate_refill_options(
    options: RefillTransactionAmountsOptions,
) -> Result<(), RefillTransactionAmountsError> {
    if options.batch_size == 0 {
        return Err(RefillTransactionAmountsError::InvalidBatchSize);
    }
    if options.workers == 0 {
        return Err(RefillTransactionAmountsError::InvalidWorkerCount);
    }

    Ok(())
}

fn open_refill_database(
    config: &Config,
    network: &Network,
    read_only: bool,
) -> Result<ZakuraDb, RefillTransactionAmountsError> {
    check_format_version(config, network)?;
    let db = ZakuraDb::new_for_explorer_amount_refill(config, network, read_only)?;
    if db.is_pruned() {
        return Err(RefillTransactionAmountsError::PrunedDatabase);
    }

    Ok(db)
}

fn explorer_schema_is_current(db: &ZakuraDb) -> Result<bool, RefillTransactionAmountsError> {
    let schema_cf =
        TypedColumnFamily::<(), ExplorerSchemaVersion>::new(db.disk_db(), EXPLORER_SCHEMA)
            .expect("explorer schema column family is registered");
    let schema = schema_cf
        .zs_get(&())
        .ok_or(RefillTransactionAmountsError::MissingExplorerSchema)?;
    if schema == ExplorerSchemaVersion::CURRENT {
        return Ok(true);
    }
    if schema.0 != EXPLORER_SCHEMA_V1 {
        return Err(RefillTransactionAmountsError::UnsupportedExplorerSchema(
            schema.0,
        ));
    }

    Ok(false)
}

fn calculate_refill_records(
    db: &ZakuraDb,
    network: &Network,
    options: RefillTransactionAmountsOptions,
    worker_pool: &rayon::ThreadPool,
    start_after: Option<TransactionLocation>,
    end_at: Option<TransactionLocation>,
    mut consume_batch: impl FnMut(
        Vec<UpgradedTransactionEntry>,
    ) -> Result<(), RefillTransactionAmountsError>,
) -> Result<RefillCalculationSummary, RefillTransactionAmountsError> {
    let metadata_cf = TypedColumnFamily::<TransactionLocation, RawBytes>::new(
        db.disk_db(),
        EXPLORER_TRANSACTION_META_BY_LOC,
    )
    .expect("explorer transaction metadata column family is registered");
    let transaction_cf =
        TypedColumnFamily::<TransactionLocation, Transaction>::new(db.disk_db(), "tx_by_loc")
            .expect("raw transaction column family is registered");
    let refill_range = (
        start_after.map_or(Bound::Unbounded, Bound::Excluded),
        end_at.map_or(Bound::Unbounded, Bound::Included),
    );

    let mut already_refilled_records = 0_u64;
    let mut first_legacy_location = None;
    for (location, record) in metadata_cf.zs_forward_range_iter(refill_range) {
        match record.raw_bytes().len() {
            EXPLORER_TRANSACTION_RECORD_V1_BYTES => {
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
        return Ok(RefillCalculationSummary {
            scanned_records: 0,
            refilled_records: 0,
            already_refilled_records,
            scan_complete: true,
        });
    };

    let remaining_range = (
        Bound::Included(first_legacy_location),
        end_at.map_or(Bound::Unbounded, Bound::Included),
    );
    let mut metadata_iter = metadata_cf
        .zs_forward_range_iter(remaining_range)
        .peekable();
    let mut transaction_iter = transaction_cf.zs_forward_range_iter(remaining_range);
    let mut scanned_records = 0_u64;
    let mut refilled_records = 0_u64;
    let mut scan_complete = true;
    let mut source_cache = SourceOutputCache::new(options.source_cache_entries);

    while metadata_iter.peek().is_some() {
        let mut legacy_entries = Vec::with_capacity(options.batch_size);
        while legacy_entries.len() < options.batch_size {
            if options.limit.is_some_and(|limit| {
                refilled_records
                    .checked_add(
                        u64::try_from(legacy_entries.len())
                            .expect("in-memory refill batch length fits in u64"),
                    )
                    .expect("transaction count fits in u64")
                    >= limit
            }) {
                break;
            }
            let Some((location, record)) = metadata_iter.next() else {
                break;
            };
            scanned_records = scanned_records
                .checked_add(1)
                .expect("transaction count fits in u64");
            let (transaction_location, transaction) = transaction_iter.next().ok_or(
                RefillTransactionAmountsError::MissingRawTransaction(location),
            )?;
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
                EXPLORER_TRANSACTION_RECORD_V1_BYTES => {
                    legacy_entries.push(LegacyTransactionEntry {
                        location,
                        record,
                        transaction,
                    });
                }
                length => {
                    return Err(RefillTransactionAmountsError::InvalidRecordLength {
                        location,
                        length,
                    });
                }
            }
        }

        let upgraded_entries = upgrade_transaction_entries(
            db,
            &transaction_cf,
            network,
            worker_pool,
            &mut source_cache,
            &legacy_entries,
        )?;
        let upgraded_count = count_as_u64(upgraded_entries.len());
        consume_batch(upgraded_entries)?;
        refilled_records = refilled_records
            .checked_add(upgraded_count)
            .expect("transaction count fits in u64");
        tracing::info!(
            refilled_records,
            workers = options.workers,
            source_cache_entries = options.source_cache_entries,
            "calculated explorer transaction metadata"
        );

        if options.limit.is_some_and(|limit| refilled_records >= limit) {
            scan_complete = metadata_iter.peek().is_none();
            break;
        }
    }

    Ok(RefillCalculationSummary {
        scanned_records,
        refilled_records,
        already_refilled_records,
        scan_complete,
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

fn default_worker_count() -> usize {
    std::thread::available_parallelism()
        .map(|parallelism| parallelism.get().saturating_sub(1).max(1))
        .unwrap_or(1)
}

fn count_as_u64(count: usize) -> u64 {
    u64::try_from(count).expect("supported targets have at most 64-bit usize values")
}

#[cfg(test)]
mod tests {
    use std::{
        fs::OpenOptions,
        io::{Seek, SeekFrom, Write},
    };

    use zakura_chain::{
        amount::{Amount, NonNegative},
        block::{self, Height},
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
                RawBytes::new_raw_bytes(vec![0; EXPLORER_TRANSACTION_RECORD_V1_BYTES]),
            );
            batch.zs_insert(
                &metadata_handle,
                location,
                RawBytes::new_raw_bytes(vec![0; EXPLORER_TRANSACTION_RECORD_V1_BYTES]),
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

        let partial_summary = refill_transaction_amounts(
            config.clone(),
            &network,
            RefillTransactionAmountsOptions {
                limit: Some(1),
                dry_run: false,
                ..Default::default()
            },
        )
        .expect("partial legacy transaction metadata is refilled");
        assert_eq!(partial_summary.refilled_records, 1);
        assert!(!partial_summary.scan_complete);
        assert!(!partial_summary.schema_updated);

        let summary = refill_transaction_amounts(
            config.clone(),
            &network,
            RefillTransactionAmountsOptions {
                dry_run: false,
                ..Default::default()
            },
        )
        .expect("remaining legacy transaction metadata is refilled");
        assert_eq!(summary.refilled_records, 1);
        assert_eq!(summary.already_refilled_records, 1);
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

    #[test]
    fn prepared_refill_imports_snapshot_and_calculates_missing_tail() {
        const PREPARED_RECORD_COUNT_OFFSET: u64 = 52;
        const PREPARED_HEADER_BYTES: u64 = 60;
        const PREPARED_ENTRY_BYTES: u64 = 129;

        let fixture = legacy_refill_fixture();
        let artifact_dir = tempfile::tempdir().expect("temporary artifact directory is created");
        let artifact_path = artifact_dir.path().join("transaction-refill.zkr");

        let prepared = prepare_transaction_amount_refill(
            fixture.config.clone(),
            &fixture.network,
            RefillTransactionAmountsOptions::default(),
            &artifact_path,
        )
        .expect("legacy transaction metadata is prepared");
        assert_eq!(prepared.source_tip_height, Height(2));
        assert_eq!(prepared.prepared_records, 2);
        assert_eq!(prepared.resumed_records, 0);

        let partial_path = prepared_transaction_amount_refill_partial_path(&artifact_path);
        std::fs::rename(&artifact_path, &partial_path)
            .expect("completed artifact becomes an interrupted checkpoint");
        let mut partial = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&partial_path)
            .expect("checkpoint reopens");
        partial
            .set_len(PREPARED_HEADER_BYTES + PREPARED_ENTRY_BYTES)
            .and_then(|()| {
                partial
                    .seek(SeekFrom::Start(PREPARED_RECORD_COUNT_OFFSET))
                    .map(|_| ())
            })
            .and_then(|()| partial.write_all(&1_u64.to_be_bytes()))
            .and_then(|()| partial.sync_all())
            .expect("checkpoint is truncated after its first record");
        drop(partial);

        let resumed = resume_prepared_transaction_amount_refill(
            fixture.config.clone(),
            &fixture.network,
            RefillTransactionAmountsOptions::default(),
            &artifact_path,
            &partial_path,
        )
        .expect("preparation resumes after its first checkpointed record");
        assert_eq!(resumed.prepared_records, 2);
        assert_eq!(resumed.resumed_records, 1);

        let tail_location = TransactionLocation::from_usize(Height(3), 0);
        let tail_transaction = Transaction::V1 {
            inputs: vec![Input::Coinbase {
                height: Height(3),
                data: vec![0; 8],
                sequence: u32::MAX,
            }],
            outputs: vec![Output::new(
                Amount::<NonNegative>::try_from(42_000_i64).expect("test amount is in range"),
                fixture.destination_address.script(),
            )],
            lock_time: LockTime::unlocked(),
        };
        {
            let db = open_test_database(&fixture.config, &fixture.network, false);
            let metadata_handle = db
                .disk_db()
                .cf_handle(EXPLORER_TRANSACTION_META_BY_LOC)
                .expect("metadata column family exists");
            let transaction_handle = db
                .disk_db()
                .cf_handle("tx_by_loc")
                .expect("transaction column family exists");
            let hash_by_height = db
                .disk_db()
                .cf_handle("hash_by_height")
                .expect("hash-by-height column family exists");
            let mut batch = DiskWriteBatch::new();
            batch.zs_insert(
                &metadata_handle,
                tail_location,
                RawBytes::new_raw_bytes(vec![0; EXPLORER_TRANSACTION_RECORD_V1_BYTES]),
            );
            batch.zs_insert(&transaction_handle, tail_location, tail_transaction);
            batch.zs_insert(&hash_by_height, Height(3), block::Hash([3; 32]));
            db.write_batch(batch).expect("tail test row is written");
            db.flush().expect("tail test row is flushed");
        }

        let applied = apply_prepared_transaction_amount_refill(
            fixture.config.clone(),
            &fixture.network,
            RefillTransactionAmountsOptions::default(),
            &artifact_path,
        )
        .expect("prepared metadata and its missing tail are applied");
        assert_eq!(applied.imported_records, 2);
        assert_eq!(applied.tail_refill.refilled_records, 1);
        assert!(applied.tail_refill.scan_complete);
        assert!(applied.tail_refill.schema_updated);

        let db = open_test_database(&fixture.config, &fixture.network, true);
        let source_record = db
            .explorer_transaction_record(fixture.source_location)
            .expect("prepared source metadata row exists");
        assert_eq!(
            source_record.primary_from,
            Some(TransactionValueEndpoint::Coinbase)
        );
        let spending_record = db
            .explorer_transaction_record(fixture.spending_location)
            .expect("prepared spending metadata row exists");
        assert_eq!(
            spending_record.primary_from,
            Some(TransactionValueEndpoint::Transparent(
                fixture.source_address
            ))
        );
        let tail_record = db
            .explorer_transaction_record(tail_location)
            .expect("calculated tail metadata row exists");
        assert_eq!(tail_record.transparent_output_total_zat, 42_000);
        assert_eq!(
            tail_record.primary_from,
            Some(TransactionValueEndpoint::Coinbase)
        );
    }

    struct LegacyRefillFixture {
        _cache_dir: tempfile::TempDir,
        config: Config,
        network: Network,
        source_address: Address,
        destination_address: Address,
        source_location: TransactionLocation,
        spending_location: TransactionLocation,
    }

    fn legacy_refill_fixture() -> LegacyRefillFixture {
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
        let spending_transaction = Transaction::V1 {
            inputs: vec![Input::PrevOut {
                outpoint: OutPoint::from_usize(source_hash, 0),
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
        let spending_location = TransactionLocation::from_usize(Height(2), 1);
        {
            let db = open_test_database(&config, &network, false);
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
            let hash_by_height = db
                .disk_db()
                .cf_handle("hash_by_height")
                .expect("hash-by-height column family exists");
            let mut batch = DiskWriteBatch::new();
            batch.zs_insert(
                &schema_handle,
                (),
                ExplorerSchemaVersion(EXPLORER_SCHEMA_V1),
            );
            batch.zs_insert(
                &metadata_handle,
                source_location,
                RawBytes::new_raw_bytes(vec![0; EXPLORER_TRANSACTION_RECORD_V1_BYTES]),
            );
            batch.zs_insert(
                &metadata_handle,
                spending_location,
                RawBytes::new_raw_bytes(vec![0; EXPLORER_TRANSACTION_RECORD_V1_BYTES]),
            );
            batch.zs_insert(&transaction_handle, source_location, source_transaction);
            batch.zs_insert(&transaction_handle, spending_location, spending_transaction);
            batch.zs_insert(&location_handle, source_hash, source_location);
            batch.zs_insert(&hash_by_height, Height(1), block::Hash([1; 32]));
            batch.zs_insert(&hash_by_height, Height(2), block::Hash([2; 32]));
            db.write_batch(batch).expect("legacy test rows are written");
            db.flush().expect("legacy test rows are flushed");
        }
        crate::write_state_database_format_version_to_disk(
            &config,
            &state_database_format_version_in_code(),
            &network,
        )
        .expect("test database format version is current");

        LegacyRefillFixture {
            _cache_dir: cache_dir,
            config,
            network,
            source_address,
            destination_address,
            source_location,
            spending_location,
        }
    }

    fn open_test_database(config: &Config, network: &Network, read_only: bool) -> ZakuraDb {
        ZakuraDb::new_for_explorer_amount_refill(config, network, read_only)
            .expect("test database opens")
    }
}
