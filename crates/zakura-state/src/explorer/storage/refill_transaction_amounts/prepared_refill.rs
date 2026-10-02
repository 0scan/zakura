//! Two-phase refill workflow that moves expensive derivation outside node downtime.

mod staging_file;

use std::{path::Path, time::Instant};

use zakura_chain::{
    block::{self, Height},
    parameters::Network,
};

use crate::{
    service::finalized_state::{DiskWriteBatch, TransactionLocation, WriteDisk, ZakuraDb},
    Config,
};

use super::{
    calculate_refill_records, count_as_u64, explorer_schema_is_current, open_refill_database,
    refill_transaction_amounts_from, refill_worker_pool, validate_refill_options,
    RefillTransactionAmountsError, RefillTransactionAmountsOptions,
    RefillTransactionAmountsSummary, EXPLORER_TRANSACTION_META_BY_LOC,
};
use staging_file::{PreparedRefillReader, PreparedRefillWriter};

/// Outcome of preparing transaction metadata while the node remains online.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PreparedTransactionAmountRefillSummary {
    /// Finalized source height captured by the read-only secondary.
    pub source_tip_height: Height,
    /// Finalized source hash used to bind the artifact to the target chain.
    pub source_tip_hash: block::Hash,
    /// Version-two records written to the prepared artifact.
    pub prepared_records: u64,
    /// Whole seconds spent deriving and writing the artifact.
    pub elapsed_seconds: u64,
}

/// Outcome of importing a prepared artifact and calculating its missing tail.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AppliedPreparedTransactionAmountRefillSummary {
    /// Records imported from the prepared artifact.
    pub imported_records: u64,
    /// Records calculated from blocks committed after the artifact snapshot.
    pub tail_refill: RefillTransactionAmountsSummary,
    /// Whole seconds spent validating, importing, and completing the refill.
    pub elapsed_seconds: u64,
}

/// Calculate all snapshot records through a read-only secondary and atomically create `output`.
///
/// The source database must still use schema version one and must not contain records from a
/// previous partial refill. The running node may append blocks while this function operates; those
/// later records are deliberately completed by [`apply_prepared_transaction_amount_refill`].
pub fn prepare_transaction_amount_refill(
    config: Config,
    network: &Network,
    options: RefillTransactionAmountsOptions,
    output: &Path,
) -> Result<PreparedTransactionAmountRefillSummary, RefillTransactionAmountsError> {
    let started = Instant::now();
    let options = RefillTransactionAmountsOptions {
        limit: None,
        dry_run: true,
        ..options
    };
    let worker_pool = refill_worker_pool(options)?;
    let db = open_refill_database(&config, network, true)?;
    if explorer_schema_is_current(&db)? {
        return Err(RefillTransactionAmountsError::ExplorerSchemaAlreadyCurrent);
    }
    let (source_tip_height, source_tip_hash) = db
        .tip()
        .ok_or(RefillTransactionAmountsError::EmptyDatabase)?;
    let mut writer =
        PreparedRefillWriter::create(output, network.kind(), source_tip_height, source_tip_hash)?;
    let calculation =
        calculate_refill_records(&db, network, options, &worker_pool, None, |entries| {
            writer.write_entries(entries)
        })?;
    if calculation.already_refilled_records != 0 {
        return Err(RefillTransactionAmountsError::PartiallyRefilledDatabase(
            calculation.already_refilled_records,
        ));
    }
    debug_assert!(calculation.scan_complete);
    debug_assert_eq!(calculation.refilled_records, calculation.scanned_records);
    let prepared_records = writer.finish()?;
    debug_assert_eq!(prepared_records, calculation.refilled_records);

    Ok(PreparedTransactionAmountRefillSummary {
        source_tip_height,
        source_tip_hash,
        prepared_records,
        elapsed_seconds: started.elapsed().as_secs(),
    })
}

/// Import a prepared snapshot, derive records appended after it, and advance the schema marker.
///
/// This function opens the primary RocksDB instance, so the node must be fully stopped first. The
/// schema marker remains at version one after any interrupted or failed run, making the operation
/// safe to retry with the same artifact.
pub fn apply_prepared_transaction_amount_refill(
    config: Config,
    network: &Network,
    options: RefillTransactionAmountsOptions,
    input: &Path,
) -> Result<AppliedPreparedTransactionAmountRefillSummary, RefillTransactionAmountsError> {
    let started = Instant::now();
    let options = RefillTransactionAmountsOptions {
        limit: None,
        dry_run: false,
        ..options
    };
    validate_refill_options(options)?;
    let mut reader = PreparedRefillReader::open_validated(input, network.kind())?;
    let manifest = reader.manifest();
    let db = open_refill_database(&config, network, false)?;
    if explorer_schema_is_current(&db)? {
        return Ok(AppliedPreparedTransactionAmountRefillSummary {
            imported_records: 0,
            tail_refill: completed_noop_summary(started),
            elapsed_seconds: started.elapsed().as_secs(),
        });
    }
    validate_source_tip(&db, manifest.source_tip_height, manifest.source_tip_hash)?;
    let imported_records = import_prepared_records(&db, &mut reader, options.batch_size)?;
    db.flush()?;
    drop(db);

    let tail_refill = refill_transaction_amounts_from(
        config,
        network,
        options,
        Some(TransactionLocation::max_for_height(
            manifest.source_tip_height,
        )),
    )?;

    Ok(AppliedPreparedTransactionAmountRefillSummary {
        imported_records,
        tail_refill,
        elapsed_seconds: started.elapsed().as_secs(),
    })
}

fn validate_source_tip(
    db: &ZakuraDb,
    source_tip_height: Height,
    source_tip_hash: block::Hash,
) -> Result<(), RefillTransactionAmountsError> {
    let database_hash = db.hash(source_tip_height);
    if database_hash != Some(source_tip_hash) {
        return Err(RefillTransactionAmountsError::PreparedTipMismatch {
            height: source_tip_height,
            prepared_hash: source_tip_hash,
            database_hash,
        });
    }

    Ok(())
}

fn import_prepared_records(
    db: &ZakuraDb,
    reader: &mut PreparedRefillReader,
    batch_size: usize,
) -> Result<u64, RefillTransactionAmountsError> {
    let metadata_handle = db
        .disk_db()
        .cf_handle(EXPLORER_TRANSACTION_META_BY_LOC)
        .expect("explorer transaction metadata column family is registered");
    let mut imported_records = 0_u64;
    while !reader.is_finished() {
        let entries = reader.read_batch(batch_size)?;
        let entry_count = count_as_u64(entries.len());
        let mut batch = DiskWriteBatch::new();
        for entry in entries {
            batch.zs_insert(&metadata_handle, entry.location, entry.record);
        }
        db.write_batch(batch)?;
        imported_records = imported_records
            .checked_add(entry_count)
            .expect("transaction count fits in u64");
        tracing::info!(imported_records, "imported prepared transaction metadata");
    }

    Ok(imported_records)
}

fn completed_noop_summary(started: Instant) -> RefillTransactionAmountsSummary {
    RefillTransactionAmountsSummary {
        scanned_records: 0,
        refilled_records: 0,
        already_refilled_records: 0,
        scan_complete: true,
        schema_updated: false,
        elapsed_seconds: started.elapsed().as_secs(),
    }
}
