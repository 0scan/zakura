//! Low-level ownership and atomic operations for the indexer RocksDB.

mod column;
mod position;

use std::{path::Path, sync::Arc};

use rocksdb::{ColumnFamilyDescriptor, Direction, IteratorMode, Options, WriteBatch, DB};

pub(crate) use column::{DatabaseColumn, MetadataKey};
pub(crate) use position::{
    decode_trailing_transaction_position, transaction_position_bytes, TRANSACTION_POSITION_BYTES,
};

use crate::Error;

type DatabaseEntry = (Vec<u8>, Vec<u8>);

/// On-disk format version for the rebuildable indexer database.
pub const DATABASE_FORMAT_VERSION: u64 = 7;

/// Cloneable low-level database shared by all index domains.
#[derive(Clone)]
pub(crate) struct IndexerDatabase {
    db: Arc<DB>,
    _temporary_directory: Option<Arc<tempfile::TempDir>>,
}

impl IndexerDatabase {
    pub(crate) fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        Self::open_at_path(path, None)
    }

    pub(crate) fn open_ephemeral(prefix: &str) -> Result<Self, Error> {
        let temporary_directory = tempfile::Builder::new().prefix(prefix).tempdir()?;
        let path = temporary_directory.path().to_path_buf();
        Self::open_at_path(path, Some(Arc::new(temporary_directory)))
    }

    fn open_at_path(
        path: impl AsRef<Path>,
        temporary_directory: Option<Arc<tempfile::TempDir>>,
    ) -> Result<Self, Error> {
        let mut database_options = Options::default();
        database_options.create_if_missing(true);
        database_options.create_missing_column_families(true);
        database_options.set_compression_type(rocksdb::DBCompressionType::Lz4);

        let column_families = DatabaseColumn::ALL
            .into_iter()
            .map(|column| ColumnFamilyDescriptor::new(column.name(), database_options.clone()));
        let db = DB::open_cf_descriptors(&database_options, path, column_families)?;
        let database = Self {
            db: Arc::new(db),
            _temporary_directory: temporary_directory,
        };
        database.ensure_format_version()?;

        Ok(database)
    }

    pub(crate) fn get(
        &self,
        column: DatabaseColumn,
        key: impl AsRef<[u8]>,
    ) -> Result<Option<Vec<u8>>, Error> {
        Ok(self.db.get_cf(self.column_family(column), key)?)
    }

    /// Returns values for `keys` in input order using one RocksDB batched read.
    pub(crate) fn multi_get<K>(
        &self,
        column: DatabaseColumn,
        keys: &[K],
    ) -> Result<Vec<Option<Vec<u8>>>, Error>
    where
        K: AsRef<[u8]>,
    {
        if keys.is_empty() {
            return Ok(Vec::new());
        }

        self.db
            .batched_multi_get_cf(self.column_family(column), keys.iter(), false)
            .into_iter()
            .map(|result| Ok(result?.map(|bytes| bytes.to_vec())))
            .collect()
    }

    pub(crate) fn insert(
        &self,
        batch: &mut WriteBatch,
        column: DatabaseColumn,
        key: impl AsRef<[u8]>,
        value: impl AsRef<[u8]>,
    ) {
        batch.put_cf(self.column_family(column), key, value);
    }

    pub(crate) fn delete(
        &self,
        batch: &mut WriteBatch,
        column: DatabaseColumn,
        key: impl AsRef<[u8]>,
    ) {
        batch.delete_cf(self.column_family(column), key);
    }

    pub(crate) fn write(&self, batch: WriteBatch) -> Result<(), Error> {
        self.db.write(batch)?;
        Ok(())
    }

    /// Returns all entries whose keys start with `prefix`, in ascending order.
    pub(crate) fn scan_prefix(
        &self,
        column: DatabaseColumn,
        prefix: &[u8],
    ) -> Result<Vec<DatabaseEntry>, Error> {
        let mut entries = Vec::new();
        for entry in self.db.iterator_cf(
            self.column_family(column),
            IteratorMode::From(prefix, Direction::Forward),
        ) {
            let (key, value) = entry?;
            if !key.starts_with(prefix) {
                break;
            }
            entries.push((key.to_vec(), value.to_vec()));
        }
        Ok(entries)
    }

    /// Returns at most `limit` prefix-matching entries at or below `start`, newest first.
    pub(crate) fn scan_prefix_reverse_from(
        &self,
        column: DatabaseColumn,
        prefix: &[u8],
        start: &[u8],
        limit: usize,
    ) -> Result<Vec<DatabaseEntry>, Error> {
        let mut entries = Vec::with_capacity(limit);
        for entry in self.db.iterator_cf(
            self.column_family(column),
            IteratorMode::From(start, Direction::Reverse),
        ) {
            let (key, value) = entry?;
            if !key.starts_with(prefix) {
                break;
            }
            entries.push((key.to_vec(), value.to_vec()));
            if entries.len() == limit {
                break;
            }
        }
        Ok(entries)
    }

    /// Returns at most `limit` prefix-matching entries at or above `start`, oldest first.
    pub(crate) fn scan_prefix_forward_from(
        &self,
        column: DatabaseColumn,
        prefix: &[u8],
        start: &[u8],
        limit: usize,
    ) -> Result<Vec<DatabaseEntry>, Error> {
        let mut entries = Vec::with_capacity(limit);
        for entry in self.db.iterator_cf(
            self.column_family(column),
            IteratorMode::From(start, Direction::Forward),
        ) {
            let (key, value) = entry?;
            if !key.starts_with(prefix) {
                break;
            }
            entries.push((key.to_vec(), value.to_vec()));
            if entries.len() == limit {
                break;
            }
        }
        Ok(entries)
    }

    /// Returns at most `limit` entries in the inclusive key range, oldest first.
    pub(crate) fn scan_range_forward(
        &self,
        column: DatabaseColumn,
        start: &[u8],
        end: &[u8],
        limit: usize,
    ) -> Result<Vec<DatabaseEntry>, Error> {
        let mut entries = Vec::with_capacity(limit);
        for entry in self.db.iterator_cf(
            self.column_family(column),
            IteratorMode::From(start, Direction::Forward),
        ) {
            let (key, value) = entry?;
            if key.as_ref() > end {
                break;
            }
            entries.push((key.to_vec(), value.to_vec()));
            if entries.len() == limit {
                break;
            }
        }
        Ok(entries)
    }

    /// Returns at most `limit` entries at or above `start`, in key order.
    pub(crate) fn scan_forward_from(
        &self,
        column: DatabaseColumn,
        start: &[u8],
        limit: usize,
    ) -> Result<Vec<DatabaseEntry>, Error> {
        let mut entries = Vec::with_capacity(limit);
        for entry in self.db.iterator_cf(
            self.column_family(column),
            IteratorMode::From(start, Direction::Forward),
        ) {
            let (key, value) = entry?;
            entries.push((key.to_vec(), value.to_vec()));
            if entries.len() == limit {
                break;
            }
        }
        Ok(entries)
    }

    fn ensure_format_version(&self) -> Result<(), Error> {
        let key = MetadataKey::FormatVersion.as_bytes();
        let expected = DATABASE_FORMAT_VERSION.to_be_bytes();
        match self.get(DatabaseColumn::Metadata, key)? {
            Some(actual) if actual.as_slice() == expected => Ok(()),
            Some(actual) => Err(Error::DatabaseFormat {
                expected: DATABASE_FORMAT_VERSION,
                actual,
            }),
            None => {
                let mut batch = WriteBatch::default();
                self.insert(&mut batch, DatabaseColumn::Metadata, key, expected);
                self.write(batch)
            }
        }
    }

    fn column_family(&self, column: DatabaseColumn) -> &rocksdb::ColumnFamily {
        self.db
            .cf_handle(column.name())
            .expect("column family exists because IndexerDatabase created it when opening RocksDB")
    }

    #[cfg(test)]
    pub(crate) fn path(&self) -> &Path {
        self.db.path()
    }
}

#[cfg(test)]
mod tests {
    use rocksdb::WriteBatch;

    use super::{DatabaseColumn, IndexerDatabase};

    #[test]
    fn multi_get_preserves_input_order_and_missing_keys() {
        let database = IndexerDatabase::open_ephemeral("zakura-indexer-multi-get-test-")
            .expect("ephemeral indexer database should open");
        let mut batch = WriteBatch::default();
        database.insert(
            &mut batch,
            DatabaseColumn::TransactionRecords,
            b"alpha",
            b"first",
        );
        database.insert(
            &mut batch,
            DatabaseColumn::TransactionRecords,
            b"bravo",
            b"second",
        );
        database
            .write(batch)
            .expect("test records should be written");

        let keys: [&[u8]; 3] = [b"bravo", b"missing", b"alpha"];
        let values = database
            .multi_get(DatabaseColumn::TransactionRecords, &keys)
            .expect("batch lookup should succeed");

        assert_eq!(
            values,
            vec![Some(b"second".to_vec()), None, Some(b"first".to_vec())]
        );
    }

    #[test]
    fn ephemeral_database_directory_is_removed_after_close() {
        let path = {
            let database = IndexerDatabase::open_ephemeral("zakura-indexer-test-")
                .expect("ephemeral indexer database should open");
            database.path().to_path_buf()
        };

        assert!(!path.exists());
    }
}
