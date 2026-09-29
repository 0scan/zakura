//! Atomic block record indexing and canonical-chain rollback.

use std::{collections::HashMap, sync::Arc};

use rocksdb::WriteBatch;
use zakura_chain::{
    block::{Block, Height},
    transparent::{OutPoint, Utxo},
};

use super::{
    disk_format::{
        block_height_key, decode_transparent_output, encode_transparent_output,
        indexed_block_tip_value, transparent_outpoint_key,
    },
    miner_attribution::identify_miner,
};
use crate::{
    database::{DatabaseColumn, MetadataKey},
    types::BlockRecord,
    Error, Indexer,
};

impl Indexer {
    /// Atomically derives and stores records for a contiguous block batch.
    pub(crate) fn index_blocks(
        &self,
        blocks: Vec<(Height, Arc<Block>, usize)>,
    ) -> Result<(), Error> {
        let mut batch = WriteBatch::default();
        let mut pending_outputs = HashMap::new();

        for (height, block, serialized_size) in blocks {
            self.prepare_block(
                &mut batch,
                &mut pending_outputs,
                height,
                &block,
                serialized_size,
            )?;
        }
        self.database.write(batch)
    }

    fn prepare_block(
        &self,
        batch: &mut WriteBatch,
        pending_outputs: &mut HashMap<OutPoint, Utxo>,
        height: Height,
        block: &Block,
        serialized_size: usize,
    ) -> Result<(), Error> {
        let actual_height = block.coinbase_height().ok_or_else(|| {
            Error::Calculation("indexed block is missing a coinbase height".to_string())
        })?;
        if actual_height != height {
            return Err(Error::Calculation(format!(
                "state returned block height {} for requested height {}",
                actual_height.0, height.0
            )));
        }

        let hash = block.hash();
        let mut total_fees = 0_u64;

        for (transaction_index, transaction) in block.transactions.iter().enumerate() {
            let spent_utxos = self.spent_utxos(transaction, pending_outputs)?;
            let fee = self.prepare_transaction(
                batch,
                height,
                transaction_index,
                transaction,
                &spent_utxos,
            )?;
            if transaction_index > 0 {
                total_fees = total_fees
                    .checked_add(fee)
                    .ok_or_else(|| Error::Calculation("block fee total exceeds u64".to_string()))?;
            }

            let transaction_hash = transaction.hash();
            for (output_index, output) in transaction.outputs().iter().enumerate() {
                let outpoint = OutPoint::from_usize(transaction_hash, output_index);
                let utxo = Utxo::from_location(output.clone(), height, transaction_index);
                pending_outputs.insert(outpoint, utxo.clone());
                self.prepare_transparent_output(batch, outpoint, &utxo)?;
            }
        }

        let coinbase = block.transactions.first().ok_or_else(|| {
            Error::Calculation("indexed block must contain a coinbase transaction".to_string())
        })?;
        let (miner_address, miner_pool) = identify_miner(coinbase, &self.network);
        let transaction_count = u32::try_from(block.transactions.len())
            .map_err(|_| Error::Calculation("block transaction count exceeds u32".to_string()))?;
        let size = u32::try_from(serialized_size)
            .map_err(|_| Error::Calculation("serialized block size exceeds u32".to_string()))?;

        let record = BlockRecord {
            height: height.0.to_string(),
            hash: hash.to_string(),
            timestamp: block.header.time.timestamp().to_string(),
            transaction_count,
            size,
            difficulty: format!(
                "{:.6}",
                block
                    .header
                    .difficulty_threshold
                    .relative_to_network(&self.network)
            ),
            miner_address,
            total_fees: total_fees.to_string(),
            miner_pool,
        };

        self.database.insert(
            batch,
            DatabaseColumn::BlockRecords,
            hash.0,
            serde_json::to_vec(&record)?,
        );
        self.database.insert(
            batch,
            DatabaseColumn::CanonicalBlockHashes,
            block_height_key(height),
            hash.0,
        );
        self.database.insert(
            batch,
            DatabaseColumn::Metadata,
            MetadataKey::IndexedBlockTip.as_bytes(),
            indexed_block_tip_value(height, hash),
        );

        Ok(())
    }

    /// Removes canonical block positions above `ancestor` in one atomic batch.
    ///
    /// Hash-addressed records and historical transparent outputs remain as a
    /// rebuildable cache. Replayed canonical blocks overwrite their positions.
    pub(crate) fn rollback_blocks_to(&self, ancestor: Option<Height>) -> Result<(), Error> {
        let Some((indexed_height, _)) = self.indexed_block_tip()? else {
            return Ok(());
        };
        let first_removed_height = ancestor
            .and_then(|height| height.next().ok())
            .unwrap_or(Height::MIN);
        let mut batch = WriteBatch::default();

        for raw_height in first_removed_height.0..=indexed_height.0 {
            self.prepare_transaction_rollback(&mut batch, Height(raw_height))?;
            self.database.delete(
                &mut batch,
                DatabaseColumn::CanonicalBlockHashes,
                block_height_key(Height(raw_height)),
            );
        }

        match ancestor {
            Some(height) => {
                let hash = self.canonical_block_hash(height)?.ok_or_else(|| {
                    Error::CorruptData(
                        "common ancestor is missing from the block index".to_string(),
                    )
                })?;
                self.database.insert(
                    &mut batch,
                    DatabaseColumn::Metadata,
                    MetadataKey::IndexedBlockTip.as_bytes(),
                    indexed_block_tip_value(height, hash),
                );
            }
            None => self.database.delete(
                &mut batch,
                DatabaseColumn::Metadata,
                MetadataKey::IndexedBlockTip.as_bytes(),
            ),
        }

        self.database.write(batch)
    }

    fn spent_utxos(
        &self,
        transaction: &zakura_chain::transaction::Transaction,
        pending_outputs: &HashMap<OutPoint, Utxo>,
    ) -> Result<HashMap<OutPoint, Utxo>, Error> {
        transaction
            .inputs()
            .iter()
            .filter_map(|input| input.outpoint())
            .map(|outpoint| {
                let utxo = if let Some(utxo) = pending_outputs.get(&outpoint) {
                    utxo.clone()
                } else {
                    self.transparent_output(outpoint)?.ok_or_else(|| {
                        Error::Calculation(format!(
                            "missing transparent output for spent outpoint {outpoint:?}"
                        ))
                    })?
                };
                Ok((outpoint, utxo))
            })
            .collect()
    }

    pub(super) fn transparent_output(&self, outpoint: OutPoint) -> Result<Option<Utxo>, Error> {
        self.database
            .get(
                DatabaseColumn::TransparentOutputs,
                transparent_outpoint_key(outpoint),
            )?
            .map(|bytes| decode_transparent_output(&bytes))
            .transpose()
    }

    fn prepare_transparent_output(
        &self,
        batch: &mut WriteBatch,
        outpoint: OutPoint,
        utxo: &Utxo,
    ) -> Result<(), Error> {
        self.database.insert(
            batch,
            DatabaseColumn::TransparentOutputs,
            transparent_outpoint_key(outpoint),
            encode_transparent_output(utxo)?,
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;
    use zakura_chain::{
        block::{genesis::regtest_genesis_block, Height},
        parameters::{testnet::RegtestParameters, Network},
        serialization::ZcashSerialize,
    };

    use crate::{transactions::TransactionQuery, types::TransactionKind, Indexer};

    #[tokio::test]
    async fn indexes_a_real_block_into_the_explorer_response() {
        let directory = TempDir::new().expect("temporary index directory should be created");
        let network = Network::new_regtest(RegtestParameters::default());
        let indexer =
            Indexer::open(directory.path(), network).expect("temporary index should open");
        let block = regtest_genesis_block();

        indexer
            .index_blocks(vec![(
                Height(0),
                block.clone(),
                block.zcash_serialized_size(),
            )])
            .expect("valid genesis block should be indexed");

        let response = indexer.recent_blocks(None, None).await.unwrap();
        assert_eq!(response.blocks.len(), 1);
        assert_eq!(response.blocks[0].height, "0");
        assert_eq!(response.blocks[0].hash, block.hash().to_string());
        assert_eq!(response.blocks[0].transaction_count, 1);
        assert_eq!(response.blocks[0].total_fees, "0");
        assert_eq!(response.pagination.total, "1");
        assert!(!response.pagination.has_more);

        let transactions = indexer
            .recent_transactions(TransactionQuery::default(), None, None)
            .await
            .expect("indexed genesis transaction should be queryable");
        assert_eq!(transactions.transactions.len(), 1);
        assert_eq!(transactions.transactions[0].kind, TransactionKind::Coinbase);
        assert_eq!(transactions.transactions[0].block_height, "0");
        assert_eq!(
            transactions.transactions[0].txid,
            block.transactions[0].hash().to_string()
        );
        assert!(!transactions.pagination.has_more);
    }

    #[tokio::test]
    async fn rollback_without_an_ancestor_clears_canonical_blocks_and_transactions() {
        let directory = TempDir::new().expect("temporary index directory should be created");
        let network = Network::new_regtest(RegtestParameters::default());
        let indexer =
            Indexer::open(directory.path(), network).expect("temporary index should open");
        let block = regtest_genesis_block();

        indexer
            .index_blocks(vec![(
                Height(0),
                block.clone(),
                block.zcash_serialized_size(),
            )])
            .expect("valid genesis block should be indexed");
        indexer
            .rollback_blocks_to(None)
            .expect("canonical block positions should roll back");

        assert_eq!(indexer.indexed_block_tip().unwrap(), None);
        assert_eq!(indexer.canonical_block_hash(Height(0)).unwrap(), None);
        let transactions = indexer
            .recent_transactions(TransactionQuery::default(), None, None)
            .await
            .expect("rolled back transaction query should succeed");
        assert!(transactions.transactions.is_empty());
    }
}
