//! Low-level ownership and atomic operations for the indexer RocksDB.

mod column;

use std::{path::Path, sync::Arc};

use rocksdb::{ColumnFamilyDescriptor, Options, WriteBatch, DB};

pub(crate) use column::{DatabaseColumn, MetadataKey};

use crate::Error;

/// On-disk format version for the rebuildable indexer database.
pub const DATABASE_FORMAT_VERSION: u64 = 1;

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
    use super::IndexerDatabase;

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
