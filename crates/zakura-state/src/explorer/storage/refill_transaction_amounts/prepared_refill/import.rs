//! Resumable import of validated prepared-refill records into RocksDB.

use crate::service::finalized_state::{
    DiskWriteBatch, RawBytes, TransactionLocation, TypedColumnFamily, WriteDisk, ZakuraDb,
};

use super::super::{
    count_as_u64, RefillTransactionAmountsError, EXPLORER_TRANSACTION_META_BY_LOC,
    EXPLORER_TRANSACTION_RECORD_V1_BYTES, TRANSACTION_RECORD_V2_BYTES,
};
use super::staging_file::PreparedRefillReader;

/// Records recovered from RocksDB and newly imported during this invocation.
pub(super) struct PreparedImportSummary {
    pub(super) resumed_records: u64,
    pub(super) imported_records: u64,
}

/// Resume after the contiguous version-two prefix already committed by an earlier invocation.
///
/// RocksDB is the authoritative checkpoint: each import batch commits atomically, so no separate
/// progress file can lag or advance independently of the records it describes.
pub(super) fn import_prepared_records(
    db: &ZakuraDb,
    reader: &mut PreparedRefillReader,
    batch_size: usize,
) -> Result<PreparedImportSummary, RefillTransactionAmountsError> {
    let resumed_records = prepared_import_progress(db, reader)?;
    reader.seek_to_record(resumed_records)?;
    tracing::info!(
        resumed_imported_records = resumed_records,
        "resuming prepared transaction metadata import"
    );

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
        let total_imported_records = resumed_records
            .checked_add(imported_records)
            .expect("transaction count fits in u64");
        tracing::info!(
            imported_records,
            total_imported_records,
            "imported prepared transaction metadata"
        );
    }

    Ok(PreparedImportSummary {
        resumed_records,
        imported_records,
    })
}

/// Locate the first version-one record using the ordered-prefix invariant of atomic imports.
///
/// Every import batch writes consecutive artifact records in one RocksDB batch. Therefore, after
/// any interruption, version-two rows form one contiguous prefix and version-one rows form its
/// suffix. Binary search keeps recovery proportional to the logarithm of the artifact size.
fn prepared_import_progress(
    db: &ZakuraDb,
    reader: &mut PreparedRefillReader,
) -> Result<u64, RefillTransactionAmountsError> {
    let metadata_cf = TypedColumnFamily::<TransactionLocation, RawBytes>::new(
        db.disk_db(),
        EXPLORER_TRANSACTION_META_BY_LOC,
    )
    .expect("explorer transaction metadata column family is registered");

    let mut first_unimported = 0_u64;
    let mut after_last_record = reader.manifest().record_count;
    while first_unimported < after_last_record {
        let distance = after_last_record
            .checked_sub(first_unimported)
            .expect("binary search bounds remain ordered");
        let record_index = first_unimported
            .checked_add(distance / 2)
            .expect("record index remains within the artifact record count");
        let location = reader.location_at(record_index)?;
        if prepared_record_is_imported(&metadata_cf, location)? {
            first_unimported = record_index
                .checked_add(1)
                .expect("artifact record index fits in u64");
        } else {
            after_last_record = record_index;
        }
    }

    Ok(first_unimported)
}

fn prepared_record_is_imported(
    metadata_cf: &TypedColumnFamily<'_, TransactionLocation, RawBytes>,
    location: TransactionLocation,
) -> Result<bool, RefillTransactionAmountsError> {
    let record = metadata_cf.zs_get(&location).ok_or(
        RefillTransactionAmountsError::MissingPreparedImportRecord(location),
    )?;
    match record.raw_bytes().len() {
        TRANSACTION_RECORD_V2_BYTES => Ok(true),
        EXPLORER_TRANSACTION_RECORD_V1_BYTES => Ok(false),
        length => Err(RefillTransactionAmountsError::InvalidRecordLength { location, length }),
    }
}
