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
    addresses::PendingAddressRecords,
    database::{DatabaseColumn, MetadataKey},
    models::{IndexedBlockRecord, TransactionPosition},
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
        let mut pending_address_records = PendingAddressRecords::new();

        for (height, block, serialized_size) in blocks {
            self.prepare_block(
                &mut batch,
                &mut pending_outputs,
                &mut pending_address_records,
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
        pending_address_records: &mut PendingAddressRecords,
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
            let transaction_index_u32 = u32::try_from(transaction_index)
                .map_err(|_| Error::Calculation("transaction index exceeds u32".to_string()))?;
            self.prepare_address_transaction(
                batch,
                pending_address_records,
                TransactionPosition {
                    height,
                    transaction_index: transaction_index_u32,
                },
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

        let model = IndexedBlockRecord {
            height,
            timestamp: block.header.time.timestamp(),
            transaction_count,
            serialized_size: size,
            difficulty: format!(
                "{:.6}",
                block
                    .header
                    .difficulty_threshold
                    .relative_to_network(&self.network)
            ),
            miner_address,
            total_fees_zat: total_fees,
            miner_pool,
        };

        self.database.insert(
            batch,
            DatabaseColumn::BlockRecords,
            hash.0,
            serde_json::to_vec(&model)?,
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
        let mut pending_address_records = PendingAddressRecords::new();

        for raw_height in first_removed_height.0..=indexed_height.0 {
            self.prepare_transaction_rollback(
                &mut batch,
                &mut pending_address_records,
                Height(raw_height),
                ancestor,
            )?;
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

    pub(crate) fn transparent_output(&self, outpoint: OutPoint) -> Result<Option<Utxo>, Error> {
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
    use std::sync::Arc;

    use tempfile::TempDir;
    use zakura_chain::{
        amount::{Amount, NonNegative},
        block::{genesis::regtest_genesis_block, Height},
        parameters::{testnet::RegtestParameters, Network, NetworkKind},
        serialization::ZcashSerialize,
        transaction::Transaction,
        transparent::{Address, Output},
    };

    use crate::{
        transactions::TransactionQuery,
        types::{PageDirection, TransactionKind},
        Indexer,
    };

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
        assert!(!response.pagination.has_next);

        let transactions = indexer
            .transactions_page(TransactionQuery::default(), None, None, PageDirection::Next)
            .await
            .expect("indexed genesis transaction should be queryable");
        assert_eq!(transactions.transactions.len(), 1);
        assert_eq!(transactions.transactions[0].kind, TransactionKind::Coinbase);
        assert_eq!(transactions.transactions[0].block_height, "0");
        assert_eq!(
            transactions.transactions[0].txid,
            block.transactions[0].hash().to_string()
        );
        assert!(!transactions.pagination.has_next);
    }

    #[tokio::test]
    async fn indexes_address_summary_and_history_without_copying_transactions() {
        let directory = TempDir::new().expect("temporary index directory should be created");
        let network = Network::new_regtest(RegtestParameters::default());
        let indexer =
            Indexer::open(directory.path(), network).expect("temporary index should open");
        let address = Address::from_pub_key_hash(NetworkKind::Testnet, [7; 20]);
        let value: Amount<NonNegative> = 123_456
            .try_into()
            .expect("test output value is within the valid monetary range");
        let mut block = (*regtest_genesis_block()).clone();
        let (inputs, lock_time) = match block.transactions[0].as_ref() {
            Transaction::V1 {
                inputs, lock_time, ..
            } => (inputs.clone(), *lock_time),
            _ => panic!("regtest genesis transaction should use version 1"),
        };
        let coinbase = Arc::new(Transaction::V1 {
            inputs,
            outputs: vec![Output::new(value, address.script())],
            lock_time,
        });
        block.transactions = vec![coinbase];
        let block = Arc::new(block);

        indexer
            .index_blocks(vec![(
                Height(0),
                block.clone(),
                block.zcash_serialized_size(),
            )])
            .expect("valid address funding block should be indexed");

        let summary = indexer.address_summary(address).await.unwrap();
        assert_eq!(summary.address, address.to_string());
        assert_eq!(summary.balance_zat, "123456");
        assert_eq!(summary.total_received_zat, "123456");
        assert_eq!(summary.total_sent_zat, "0");
        assert_eq!(summary.transaction_count, "1");
        assert!(summary.first_seen.is_some());
        assert!(summary.last_seen.is_some());
        assert!(summary.first_funding.as_ref().unwrap().is_coinbase);

        let page = indexer
            .address_transactions_page(address, None, None, PageDirection::Next)
            .await
            .unwrap();
        assert_eq!(page.transactions.len(), 1);
        assert_eq!(page.transactions[0].received_zat, "123456");
        assert_eq!(page.transactions[0].sent_zat, "0");
        assert_eq!(page.transactions[0].net_change_zat, "123456");

        indexer
            .rollback_blocks_to(None)
            .expect("address indexes should roll back atomically");
        let summary = indexer.address_summary(address).await.unwrap();
        assert_eq!(summary.transaction_count, "0");
        assert_eq!(summary.balance_zat, "0");
        let page = indexer
            .address_transactions_page(address, None, None, PageDirection::Next)
            .await
            .unwrap();
        assert!(page.transactions.is_empty());
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
            .transactions_page(TransactionQuery::default(), None, None, PageDirection::Next)
            .await
            .expect("rolled back transaction query should succeed");
        assert!(transactions.transactions.is_empty());
    }
}
