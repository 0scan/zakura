//! Canonical block reads and stable cursor pagination.

use zakura_chain::block::{Hash, Height};

use super::{
    cursor::BlockCursor,
    disk_format::{block_height_key, decode_indexed_block_tip},
};
use crate::{
    database::{DatabaseColumn, MetadataKey},
    types::{BlocksPagination, BlocksResponse},
    Error, Indexer,
};

const DEFAULT_QUERY_LIMIT: u32 = 5;
const MAX_QUERY_LIMIT: u32 = 100;

impl Indexer {
    /// Returns canonical block summaries from newest to oldest.
    ///
    /// A cursor identifies the final canonical block from the previous page.
    /// If a reorganization removes that block, this method rejects the cursor so
    /// callers can restart from the current tip without silently skipping data.
    pub async fn recent_blocks(
        &self,
        limit: Option<u32>,
        cursor: Option<String>,
    ) -> Result<BlocksResponse, Error> {
        let indexer = self.clone();
        tokio::task::spawn_blocking(move || indexer.recent_blocks_blocking(limit, cursor))
            .await
            .map_err(|error| Error::Task(error.to_string()))?
    }

    pub(crate) fn indexed_block_tip(&self) -> Result<Option<(Height, Hash)>, Error> {
        self.database
            .get(
                DatabaseColumn::Metadata,
                MetadataKey::IndexedBlockTip.as_bytes(),
            )?
            .map(|bytes| decode_indexed_block_tip(&bytes))
            .transpose()
    }

    pub(crate) fn canonical_block_hash(&self, height: Height) -> Result<Option<Hash>, Error> {
        let Some(bytes) = self.database.get(
            DatabaseColumn::CanonicalBlockHashes,
            block_height_key(height),
        )?
        else {
            return Ok(None);
        };

        let hash = Hash(bytes.as_slice().try_into().map_err(|_| {
            Error::CorruptData("stored canonical block hash must be 32 bytes".to_string())
        })?);
        Ok(Some(hash))
    }

    fn recent_blocks_blocking(
        &self,
        limit: Option<u32>,
        cursor: Option<String>,
    ) -> Result<BlocksResponse, Error> {
        let limit = limit
            .unwrap_or(DEFAULT_QUERY_LIMIT)
            .clamp(1, MAX_QUERY_LIMIT);
        let cursor = cursor
            .map(|encoded| BlockCursor::decode(&encoded))
            .transpose()?;
        let Some((tip_height, _)) = self.indexed_block_tip()? else {
            if cursor.is_some() {
                return Err(Error::InvalidCursor(
                    "cursor cannot reference an empty indexed chain".to_string(),
                ));
            }
            return Ok(empty_response(limit, 0));
        };

        let total = u64::from(tip_height.0) + 1;
        let capacity = usize::try_from(limit).map_err(|_| {
            Error::Calculation("query limit does not fit this platform's usize".to_string())
        })?;
        let start_height = match cursor {
            Some(cursor) => {
                if self.canonical_block_hash(cursor.height)? != Some(cursor.hash) {
                    return Err(Error::InvalidCursor(
                        "cursor block is no longer on the indexed canonical chain".to_string(),
                    ));
                }
                cursor.height.previous().ok()
            }
            None => Some(tip_height),
        };
        let Some(mut height) = start_height else {
            return Ok(empty_response(limit, total));
        };
        let mut blocks = Vec::with_capacity(capacity);
        let mut last_position = None;

        for _ in 0..limit {
            let hash = self.canonical_block_hash(height)?.ok_or_else(|| {
                Error::CorruptData(format!(
                    "missing canonical block hash at height {}",
                    height.0
                ))
            })?;
            let value = self
                .database
                .get(DatabaseColumn::BlockRecords, hash.0)?
                .ok_or_else(|| {
                    Error::CorruptData(format!("missing block record for canonical hash {hash}"))
                })?;
            blocks.push(serde_json::from_slice(&value)?);
            last_position = Some(BlockCursor::new(height, hash));

            let Ok(previous_height) = height.previous() else {
                break;
            };
            height = previous_height;
        }

        let has_more = last_position.is_some_and(|position| position.height > Height::MIN);
        let next_cursor = last_position
            .filter(|position| has_more && position.height > Height::MIN)
            .map(BlockCursor::encode);
        Ok(BlocksResponse {
            blocks,
            pagination: BlocksPagination {
                limit,
                total: total.to_string(),
                has_more,
                next_cursor,
            },
        })
    }
}

fn empty_response(limit: u32, total: u64) -> BlocksResponse {
    BlocksResponse {
        blocks: Vec::new(),
        pagination: BlocksPagination {
            limit,
            total: total.to_string(),
            has_more: false,
            next_cursor: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use rocksdb::WriteBatch;
    use tempfile::TempDir;
    use zakura_chain::{
        block::{Hash, Height},
        parameters::Network,
    };

    use super::super::disk_format::{block_height_key, indexed_block_tip_value};
    use crate::{
        database::{DatabaseColumn, MetadataKey},
        types::BlockRecord,
        Indexer,
    };

    #[tokio::test]
    async fn block_cursor_is_stable_when_the_tip_advances() {
        let directory = TempDir::new().expect("temporary index directory should be created");
        let indexer =
            Indexer::open(directory.path(), Network::Mainnet).expect("temporary index should open");

        for height in 0..3 {
            put_test_record(&indexer, height);
        }

        let first_page = indexer.recent_blocks(Some(2), None).await.unwrap();
        assert_eq!(block_heights(&first_page.blocks), ["2", "1"]);
        assert!(first_page.pagination.has_more);
        let cursor = first_page.pagination.next_cursor.clone().unwrap();

        put_test_record(&indexer, 3);

        let second_page = indexer.recent_blocks(Some(2), Some(cursor)).await.unwrap();
        assert_eq!(block_heights(&second_page.blocks), ["0"]);
        assert_eq!(second_page.pagination.total, "4");
        assert!(!second_page.pagination.has_more);
        assert_eq!(second_page.pagination.next_cursor, None);

        let json = serde_json::to_value(&first_page).unwrap();
        assert_eq!(json["pagination"]["hasMore"], true);
        assert!(json["pagination"]["nextCursor"].is_string());
        assert!(json["pagination"].get("has_more").is_none());
        assert!(json["pagination"].get("next_cursor").is_none());
    }

    #[tokio::test]
    async fn block_cursor_is_rejected_after_its_block_is_reorganized() {
        let directory = TempDir::new().expect("temporary index directory should be created");
        let indexer =
            Indexer::open(directory.path(), Network::Mainnet).expect("temporary index should open");

        for height in 0..3 {
            put_test_record(&indexer, height);
        }

        let first_page = indexer.recent_blocks(Some(2), None).await.unwrap();
        let cursor = first_page.pagination.next_cursor.unwrap();

        put_test_record_with_hash(&indexer, 1, Hash([9; 32]));

        let error = indexer
            .recent_blocks(Some(2), Some(cursor))
            .await
            .unwrap_err();
        assert!(matches!(error, crate::Error::InvalidCursor(_)));
    }

    fn put_test_record(indexer: &Indexer, height: u32) {
        let hash_byte = u8::try_from(height).expect("test heights fit in u8");
        put_test_record_with_hash(indexer, height, Hash([hash_byte; 32]));
    }

    fn put_test_record_with_hash(indexer: &Indexer, height: u32, hash: Hash) {
        let record = BlockRecord {
            height: height.to_string(),
            hash: hash.to_string(),
            timestamp: height.to_string(),
            transaction_count: 1,
            size: 100,
            difficulty: "1.000000".to_string(),
            miner_address: None,
            total_fees: "0".to_string(),
            miner_pool: "Unknown".to_string(),
        };
        let mut batch = WriteBatch::default();
        indexer.database.insert(
            &mut batch,
            DatabaseColumn::BlockRecords,
            hash.0,
            serde_json::to_vec(&record).unwrap(),
        );
        indexer.database.insert(
            &mut batch,
            DatabaseColumn::CanonicalBlockHashes,
            block_height_key(Height(height)),
            hash.0,
        );
        indexer.database.insert(
            &mut batch,
            DatabaseColumn::Metadata,
            MetadataKey::IndexedBlockTip.as_bytes(),
            indexed_block_tip_value(Height(height), hash),
        );
        indexer.database.write(batch).unwrap();
    }

    fn block_heights(blocks: &[BlockRecord]) -> Vec<&str> {
        blocks.iter().map(|block| block.height.as_str()).collect()
    }
}
