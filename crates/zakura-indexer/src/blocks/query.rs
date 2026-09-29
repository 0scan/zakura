//! Canonical block reads and stable cursor pagination.

use zakura_chain::block::{Hash, Height};

use super::{
    cursor::BlockCursor,
    disk_format::{block_height_key, decode_indexed_block_tip},
};
use crate::{
    database::{DatabaseColumn, MetadataKey},
    models::IndexedBlockRecord,
    types::{BlockRecord, BlocksPagination, BlocksResponse, PageDirection},
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
        self.blocks_page(limit, cursor, PageDirection::Next).await
    }

    /// Returns a canonical block page in either direction from `cursor`.
    pub async fn blocks_page(
        &self,
        limit: Option<u32>,
        cursor: Option<String>,
        direction: PageDirection,
    ) -> Result<BlocksResponse, Error> {
        let indexer = self.clone();
        tokio::task::spawn_blocking(move || indexer.blocks_page_blocking(limit, cursor, direction))
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

    /// Materializes the explorer response record stored for `hash`.
    pub(crate) fn block_record(&self, hash: Hash) -> Result<Option<BlockRecord>, Error> {
        Ok(self
            .indexed_block_record(hash)?
            .map(|model| block_record(hash, model)))
    }

    pub(crate) fn indexed_block_record(
        &self,
        hash: Hash,
    ) -> Result<Option<IndexedBlockRecord>, Error> {
        self.database
            .get(DatabaseColumn::BlockRecords, hash.0)?
            .map(|value| serde_json::from_slice(&value).map_err(Error::from))
            .transpose()
    }

    fn blocks_page_blocking(
        &self,
        limit: Option<u32>,
        cursor: Option<String>,
        direction: PageDirection,
    ) -> Result<BlocksResponse, Error> {
        let limit = limit
            .unwrap_or(DEFAULT_QUERY_LIMIT)
            .clamp(1, MAX_QUERY_LIMIT);
        let cursor = cursor
            .map(|encoded| BlockCursor::decode(&encoded))
            .transpose()?;
        if direction == PageDirection::Previous && cursor.is_none() {
            return Err(Error::InvalidCursor(
                "direction=prev requires a cursor".to_string(),
            ));
        }
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
        if let Some(cursor) = cursor {
            if self.canonical_block_hash(cursor.height)? != Some(cursor.hash) {
                return Err(Error::InvalidCursor(
                    "cursor block is no longer on the indexed canonical chain".to_string(),
                ));
            }
        }
        let start_height = match (direction, cursor) {
            (PageDirection::Next, Some(cursor)) => cursor.height.previous().ok(),
            (PageDirection::Next, None) => Some(tip_height),
            (PageDirection::Previous, Some(cursor)) => cursor
                .height
                .next()
                .ok()
                .filter(|height| *height <= tip_height),
            (PageDirection::Previous, None) => unreachable!("previous pages require a cursor"),
        };
        let Some(mut height) = start_height else {
            return Ok(empty_response(limit, total));
        };
        let mut records = Vec::with_capacity(capacity);

        for _ in 0..limit {
            let hash = self.canonical_block_hash(height)?.ok_or_else(|| {
                Error::CorruptData(format!(
                    "missing canonical block hash at height {}",
                    height.0
                ))
            })?;
            let model = self.indexed_block_record(hash)?.ok_or_else(|| {
                Error::CorruptData(format!("missing block record for canonical hash {hash}"))
            })?;
            if model.height != height {
                return Err(Error::CorruptData(format!(
                    "block record {hash} has height {}, expected {}",
                    model.height.0, height.0
                )));
            }
            records.push((BlockCursor::new(height, hash), block_record(hash, model)));

            height = match direction {
                PageDirection::Next => match height.previous() {
                    Ok(previous_height) => previous_height,
                    Err(_) => break,
                },
                PageDirection::Previous => match height.next() {
                    Ok(next_height) if next_height <= tip_height => next_height,
                    Ok(_) | Err(_) => break,
                },
            };
        }

        if direction == PageDirection::Previous {
            records.reverse();
        }
        let first_position = records.first().map(|(position, _)| *position);
        let last_position = records.last().map(|(position, _)| *position);
        let has_prev = first_position.is_some_and(|position| position.height < tip_height);
        let has_next = last_position.is_some_and(|position| position.height > Height::MIN);
        let prev_cursor = first_position.filter(|_| has_prev).map(BlockCursor::encode);
        let next_cursor = last_position.filter(|_| has_next).map(BlockCursor::encode);
        let blocks = records.into_iter().map(|(_, record)| record).collect();

        Ok(BlocksResponse {
            blocks,
            pagination: BlocksPagination {
                limit,
                total: total.to_string(),
                has_next,
                has_prev,
                next_cursor,
                prev_cursor,
            },
        })
    }
}

fn block_record(hash: Hash, model: IndexedBlockRecord) -> BlockRecord {
    BlockRecord {
        height: model.height.0.to_string(),
        hash: hash.to_string(),
        timestamp: model.timestamp.to_string(),
        transaction_count: model.transaction_count,
        size: model.serialized_size,
        difficulty: model.difficulty,
        miner_address: model.miner_address,
        total_fees: model.total_fees_zat.to_string(),
        miner_pool: model.miner_pool,
    }
}

fn empty_response(limit: u32, total: u64) -> BlocksResponse {
    BlocksResponse {
        blocks: Vec::new(),
        pagination: BlocksPagination {
            limit,
            total: total.to_string(),
            has_next: false,
            has_prev: false,
            next_cursor: None,
            prev_cursor: None,
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
        models::IndexedBlockRecord,
        types::{BlockRecord, PageDirection},
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
        assert!(first_page.pagination.has_next);
        assert!(!first_page.pagination.has_prev);
        let cursor = first_page.pagination.next_cursor.clone().unwrap();

        put_test_record(&indexer, 3);

        let second_page = indexer.recent_blocks(Some(2), Some(cursor)).await.unwrap();
        assert_eq!(block_heights(&second_page.blocks), ["0"]);
        assert_eq!(second_page.pagination.total, "4");
        assert!(!second_page.pagination.has_next);
        assert!(second_page.pagination.has_prev);
        assert_eq!(second_page.pagination.next_cursor, None);

        let previous_page = indexer
            .blocks_page(
                Some(2),
                second_page.pagination.prev_cursor,
                PageDirection::Previous,
            )
            .await
            .expect("newer block page should load");
        assert_eq!(block_heights(&previous_page.blocks), ["2", "1"]);
        assert!(previous_page.pagination.has_prev);
        assert!(previous_page.pagination.has_next);

        let json = serde_json::to_value(&first_page).unwrap();
        assert_eq!(json["pagination"]["has_next"], true);
        assert_eq!(json["pagination"]["has_prev"], false);
        assert!(json["pagination"]["next_cursor"].is_string());
        assert!(json["pagination"]["prev_cursor"].is_null());
        assert!(json["pagination"].get("hasNext").is_none());
        assert!(json["pagination"].get("nextCursor").is_none());
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
        let model = IndexedBlockRecord {
            height: Height(height),
            timestamp: i64::from(height),
            transaction_count: 1,
            serialized_size: 100,
            difficulty: "1.000000".to_string(),
            miner_address: None,
            total_fees_zat: 0,
            miner_pool: "Unknown".to_string(),
            transparent_transaction_count: 0,
            shielded_transaction_count: 0,
            coinbase_transaction_count: 1,
            fully_shielded_transaction_count: 0,
            mixed_pool_transaction_count: 0,
        };
        let mut batch = WriteBatch::default();
        indexer.database.insert(
            &mut batch,
            DatabaseColumn::BlockRecords,
            hash.0,
            serde_json::to_vec(&model).unwrap(),
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
