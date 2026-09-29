//! Atomic block record indexing and canonical-chain rollback.

use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
};

use rocksdb::WriteBatch;
use zakura_chain::{
    amount::{Amount, NegativeAllowed, NonNegative},
    block::{Block, Height},
    parameters::subsidy::{
        block_subsidy, founders_reward, funding_stream_values, is_zip234_active, miner_subsidy,
        parent_nsm_value_balance,
    },
    transparent::{OutPoint, Utxo},
    value_balance::ValueBalance,
    work::difficulty::U256,
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
    stats::{day_key, day_number, BlockTransactionStats},
    Error, Indexer,
};

struct PendingBlockBatch {
    batch: WriteBatch,
    outputs: HashMap<OutPoint, Utxo>,
    addresses: PendingAddressRecords,
    chain_stats: crate::models::ChainStatsRecord,
    daily_stats: HashMap<u32, crate::models::DailyStatsRecord>,
    previous_block_timestamp: Option<i64>,
    previous_pool_nsm: i64,
}

impl Indexer {
    /// Atomically derives and stores records for a contiguous block batch.
    pub(crate) fn index_blocks(
        &self,
        blocks: Vec<(Height, Arc<Block>, usize, ValueBalance<NonNegative>)>,
    ) -> Result<(), Error> {
        let previous = match self.indexed_block_tip()? {
            Some((_, hash)) => self.indexed_block_record(hash)?,
            None => None,
        };
        let mut pending = PendingBlockBatch {
            batch: WriteBatch::default(),
            outputs: HashMap::new(),
            addresses: PendingAddressRecords::new(),
            chain_stats: self.chain_stats_record()?,
            daily_stats: HashMap::new(),
            previous_block_timestamp: previous.as_ref().map(|block| block.timestamp),
            previous_pool_nsm: previous.as_ref().map_or(0, |block| block.pool_nsm),
        };

        for (height, block, serialized_size, value_pools) in blocks {
            self.prepare_block(&mut pending, height, &block, serialized_size, value_pools)?;
        }
        self.prepare_chain_stats_write(&mut pending.batch, pending.chain_stats)?;
        for (day, stats) in pending.daily_stats {
            self.prepare_daily_stats_write(&mut pending.batch, day, &stats)?;
        }
        self.database.write(pending.batch)
    }

    fn prepare_block(
        &self,
        pending: &mut PendingBlockBatch,
        height: Height,
        block: &Block,
        serialized_size: usize,
        value_pools: ValueBalance<NonNegative>,
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
        let mut transaction_stats = BlockTransactionStats::default();

        for (transaction_index, transaction) in block.transactions.iter().enumerate() {
            let spent_utxos = self.spent_utxos(transaction, &pending.outputs)?;
            let transaction_record = self.prepare_transaction(
                &mut pending.batch,
                height,
                transaction_index,
                transaction,
                &spent_utxos,
            )?;
            transaction_stats.record(
                &transaction_record,
                transaction,
                &spent_utxos,
                &self.network,
            )?;
            let transaction_index_u32 = u32::try_from(transaction_index)
                .map_err(|_| Error::Calculation("transaction index exceeds u32".to_string()))?;
            self.prepare_address_transaction(
                &mut pending.batch,
                &mut pending.addresses,
                &mut pending.chain_stats.funded_transparent_address_count,
                TransactionPosition {
                    height,
                    transaction_index: transaction_index_u32,
                },
                transaction,
                &spent_utxos,
            )?;
            if transaction_index > 0 {
                total_fees = total_fees
                    .checked_add(transaction_record.fee_zat)
                    .ok_or_else(|| Error::Calculation("block fee total exceeds u64".to_string()))?;
            }

            let transaction_hash = transaction.hash();
            for (output_index, output) in transaction.outputs().iter().enumerate() {
                let outpoint = OutPoint::from_usize(transaction_hash, output_index);
                let utxo = Utxo::from_location(output.clone(), height, transaction_index);
                pending.outputs.insert(outpoint, utxo.clone());
                self.prepare_transparent_output(&mut pending.batch, outpoint, &utxo)?;
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
        let accepted_work = block
            .header
            .difficulty_threshold
            .to_work()
            .ok_or_else(|| Error::Calculation("indexed block has an invalid target".to_string()))?
            .as_u256();
        if accepted_work > U256::from(u128::MAX) {
            return Err(Error::Calculation(
                "indexed block work exceeds the chart accumulator width".to_string(),
            ));
        }

        let mut interval = transaction_stats.interval.clone();
        interval.block_count = 1;
        interval.transaction_count = u64::from(transaction_count);
        interval.empty_block_count = u64::from(transaction_count <= 1);
        interval.accepted_work = accepted_work.low_u128();
        interval.total_fees_zat = u128::from(total_fees);
        interval.total_block_size_bytes = u64::from(size);
        interval.ironwood_active_block_count =
            u64::from(transaction_stats.has_ironwood_transaction);
        add_mining_accounting(
            &mut interval,
            block,
            height,
            total_fees,
            pending.previous_pool_nsm,
            &self.network,
        )?;

        let pool_transparent =
            non_negative_amount(value_pools.transparent_amount(), "transparent")?;
        let pool_sprout = non_negative_amount(value_pools.sprout_amount(), "Sprout")?;
        let pool_sapling = non_negative_amount(value_pools.sapling_amount(), "Sapling")?;
        let pool_orchard = non_negative_amount(value_pools.orchard_amount(), "Orchard")?;
        let pool_deferred = non_negative_amount(value_pools.deferred_amount(), "deferred")?;
        let pool_ironwood = non_negative_amount(value_pools.ironwood_amount(), "Ironwood")?;
        let pool_nsm = value_pools.nsm_value_balance_amount().zatoshis();
        let total_issuance = non_negative_amount(value_pools.issued_supply(), "issued supply")?;

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
            accepted_work: accepted_work.low_u128(),
            miner_address,
            total_fees_zat: total_fees,
            miner_pool,
            transparent_transaction_count: transaction_stats.transparent,
            shielded_transaction_count: transaction_stats.shielded,
            coinbase_transaction_count: transaction_stats.coinbase,
            fully_shielded_transaction_count: transaction_stats.fully_shielded,
            mixed_pool_transaction_count: transaction_stats.mixed_pool,
            funded_transparent_address_count: pending.chain_stats.funded_transparent_address_count,
            pool_transparent,
            pool_sprout,
            pool_sapling,
            pool_orchard,
            pool_deferred,
            pool_ironwood,
            pool_nsm,
            total_issuance,
            interval,
        };

        self.add_block_to_chain_stats(&mut pending.chain_stats, &model)?;
        let day = day_number(model.timestamp)?;
        let daily_stats = match pending.daily_stats.entry(day) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(self.daily_stats_record(day)?.unwrap_or_default())
            }
        };
        self.add_block_to_daily_stats(daily_stats, &model, hash, pending.previous_block_timestamp)?;
        pending.previous_block_timestamp = Some(model.timestamp);
        pending.previous_pool_nsm = model.pool_nsm;

        self.database.insert(
            &mut pending.batch,
            DatabaseColumn::BlockRecords,
            hash.0,
            serde_json::to_vec(&model)?,
        );
        self.database.insert(
            &mut pending.batch,
            DatabaseColumn::CanonicalBlockHashes,
            block_height_key(height),
            hash.0,
        );
        self.database.insert(
            &mut pending.batch,
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
        let mut chain_stats = self.chain_stats_record()?;
        let mut affected_days = BTreeSet::new();

        for raw_height in first_removed_height.0..=indexed_height.0 {
            let height = Height(raw_height);
            let hash = self.canonical_block_hash(height)?.ok_or_else(|| {
                Error::CorruptData(format!(
                    "missing canonical block hash while rolling back height {raw_height}"
                ))
            })?;
            let block = self.indexed_block_record(hash)?.ok_or_else(|| {
                Error::CorruptData(format!(
                    "missing block record while rolling back canonical hash {hash}"
                ))
            })?;
            affected_days.insert(day_number(block.timestamp)?);
            self.remove_block_from_chain_stats(&mut chain_stats, &block)?;
            self.prepare_transaction_rollback(
                &mut batch,
                &mut pending_address_records,
                &mut chain_stats.funded_transparent_address_count,
                height,
                ancestor,
            )?;
            self.database.delete(
                &mut batch,
                DatabaseColumn::CanonicalBlockHashes,
                block_height_key(Height(raw_height)),
            );
        }
        self.prepare_chain_stats_write(&mut batch, chain_stats)?;

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
        self.prepare_daily_stats_rollback(&mut batch, &affected_days, ancestor)?;

        self.database.write(batch)
    }

    fn prepare_daily_stats_rollback(
        &self,
        batch: &mut WriteBatch,
        affected_days: &BTreeSet<u32>,
        ancestor: Option<Height>,
    ) -> Result<(), Error> {
        for &day in affected_days {
            let existing = self.daily_stats_record(day)?.ok_or_else(|| {
                Error::CorruptData(format!("missing daily stats for UTC day {day}"))
            })?;
            let Some(end_height) = ancestor
                .map(|height| height.0.min(existing.end_height))
                .filter(|height| *height >= existing.start_height)
            else {
                self.database
                    .delete(batch, DatabaseColumn::DailyStats, day_key(day));
                continue;
            };

            let mut rebuilt = crate::models::DailyStatsRecord::default();
            let mut previous_timestamp = match existing.start_height.checked_sub(1) {
                Some(raw_height) => {
                    let hash = self
                        .canonical_block_hash(Height(raw_height))?
                        .ok_or_else(|| {
                            Error::CorruptData(format!(
                                "missing canonical block before daily stats at height {raw_height}"
                            ))
                        })?;
                    self.indexed_block_record(hash)?
                        .map(|block| block.timestamp)
                }
                None => None,
            };
            for raw_height in existing.start_height..=end_height {
                let hash = self
                    .canonical_block_hash(Height(raw_height))?
                    .ok_or_else(|| {
                        Error::CorruptData(format!(
                            "missing retained canonical block at height {raw_height}"
                        ))
                    })?;
                let block = self.indexed_block_record(hash)?.ok_or_else(|| {
                    Error::CorruptData(format!("missing retained block record for {hash}"))
                })?;
                if day_number(block.timestamp)? == day {
                    self.add_block_to_daily_stats(&mut rebuilt, &block, hash, previous_timestamp)?;
                }
                previous_timestamp = Some(block.timestamp);
            }
            self.prepare_daily_stats_write(batch, day, &rebuilt)?;
        }
        Ok(())
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

    pub(crate) fn transparent_outputs(
        &self,
        outpoints: &[OutPoint],
    ) -> Result<Vec<Option<Utxo>>, Error> {
        let keys = outpoints
            .iter()
            .copied()
            .map(transparent_outpoint_key)
            .collect::<Vec<_>>();

        self.database
            .multi_get(DatabaseColumn::TransparentOutputs, &keys)?
            .into_iter()
            .map(|bytes| {
                bytes
                    .map(|bytes| decode_transparent_output(&bytes))
                    .transpose()
            })
            .collect()
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

fn add_mining_accounting(
    interval: &mut crate::models::IntervalStatsRecord,
    block: &Block,
    height: Height,
    total_fees_zat: u64,
    previous_pool_nsm: i64,
    network: &zakura_chain::parameters::Network,
) -> Result<(), Error> {
    let parent_nsm = if is_zip234_active(network, height) {
        let amount = Amount::<NegativeAllowed>::try_from(previous_pool_nsm)
            .map_err(|error| Error::Calculation(error.to_string()))?;
        Some(
            parent_nsm_value_balance(amount)
                .map_err(|error| Error::Calculation(error.to_string()))?,
        )
    } else {
        None
    };
    let subsidy = block_subsidy(height, network, parent_nsm)
        .map_err(|error| Error::Calculation(error.to_string()))?;
    let founder_reward = founders_reward(network, height);
    let funding = funding_stream_values(height, network, subsidy)
        .map_err(|error| Error::Calculation(error.to_string()))?;
    let mut direct_funding_streams_zat = 0_u128;
    let mut deferred_subsidy_zat = 0_u128;
    for (receiver, value) in funding {
        let value = u128::from(non_negative_amount(value, "funding stream")?);
        if receiver.is_deferred() {
            deferred_subsidy_zat =
                checked_add_u128(deferred_subsidy_zat, value, "deferred subsidy")?;
        } else {
            direct_funding_streams_zat =
                checked_add_u128(direct_funding_streams_zat, value, "direct funding streams")?;
        }
    }
    let miner_subsidy = miner_subsidy(height, network, subsidy)
        .map_err(|error| Error::Calculation(error.to_string()))?;

    let coinbase = block.transactions.first().ok_or_else(|| {
        Error::Calculation("indexed block must contain a coinbase transaction".to_string())
    })?;
    let transparent_output = coinbase
        .outputs()
        .iter()
        .try_fold(0_u128, |total, output| {
            checked_add_u128(
                total,
                u128::from(output.value().zatoshis().unsigned_abs()),
                "coinbase transparent outputs",
            )
        })?;
    let shielded_output = |value: i64, pool: &str| -> Result<u128, Error> {
        if value > 0 {
            return Err(Error::Calculation(format!(
                "verified coinbase withdraws {value} zatoshis from {pool}"
            )));
        }
        Ok(u128::from(value.unsigned_abs()))
    };
    let sapling_output = shielded_output(
        coinbase.sapling_value_balance().sapling_amount().zatoshis(),
        "Sapling",
    )?;
    let orchard_output = shielded_output(
        coinbase.orchard_value_balance().orchard_amount().zatoshis(),
        "Orchard",
    )?;
    let ironwood_output = shielded_output(
        coinbase
            .ironwood_value_balance()
            .ironwood_amount()
            .zatoshis(),
        "Ironwood",
    )?;
    let observed_outputs = [
        transparent_output,
        sapling_output,
        orchard_output,
        ironwood_output,
    ]
    .into_iter()
    .try_fold(0_u128, |total, value| {
        checked_add_u128(total, value, "observed coinbase outputs")
    })?;
    let subsidy_zat = u128::from(non_negative_amount(subsidy, "block subsidy")?);
    let lockbox_disbursement_zat = u128::from(non_negative_amount(
        network.lockbox_disbursement_total_amount(height),
        "lockbox disbursement",
    )?);
    let allowed_outputs = subsidy_zat
        .checked_sub(deferred_subsidy_zat)
        .and_then(|value| value.checked_add(u128::from(total_fees_zat)))
        .and_then(|value| value.checked_add(lockbox_disbursement_zat))
        .ok_or_else(|| Error::Calculation("allowed coinbase output exceeds u128".to_string()))?;
    let unclaimed = allowed_outputs
        .checked_sub(observed_outputs)
        .ok_or_else(|| {
            Error::Calculation(format!(
                "observed coinbase output {observed_outputs} exceeds allowed output {allowed_outputs}"
            ))
        })?;

    interval.total_subsidy_zat = subsidy_zat;
    interval.miner_subsidy_zat = u128::from(non_negative_amount(miner_subsidy, "miner subsidy")?);
    interval.founders_reward_zat =
        u128::from(non_negative_amount(founder_reward, "founders reward")?);
    interval.funding_streams_zat = direct_funding_streams_zat;
    interval.deferred_subsidy_zat = deferred_subsidy_zat;
    interval.lockbox_disbursement_zat = lockbox_disbursement_zat;
    interval.coinbase_output_transparent_zat = transparent_output;
    interval.coinbase_output_sapling_zat = sapling_output;
    interval.coinbase_output_orchard_zat = orchard_output;
    interval.coinbase_output_ironwood_zat = ironwood_output;
    interval.coinbase_unclaimed_zat = unclaimed;
    Ok(())
}

fn non_negative_amount(amount: Amount<NonNegative>, field: &str) -> Result<u64, Error> {
    u64::try_from(amount.zatoshis())
        .map_err(|_| Error::Calculation(format!("{field} must be non-negative")))
}

fn checked_add_u128(current: u128, value: u128, field: &str) -> Result<u128, Error> {
    current
        .checked_add(value)
        .ok_or_else(|| Error::Calculation(format!("{field} exceeds u128")))
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
        transparent::{Address, OutPoint, Output},
    };

    use crate::{
        transactions::TransactionQuery,
        types::{ChartDataRequest, PageDirection, TopBalancesRequest, TransactionKind},
        Indexer,
    };

    #[test]
    fn loads_transparent_outputs_in_one_ordered_batch() {
        let network = Network::new_regtest(RegtestParameters::default());
        let indexer = Indexer::open_ephemeral(network).expect("ephemeral index should open");
        let block = regtest_genesis_block();
        let transaction_hash = block.transactions[0].hash();
        indexer
            .index_blocks(vec![(
                Height(0),
                block.clone(),
                block.zcash_serialized_size(),
                Default::default(),
            )])
            .expect("valid genesis block should be indexed");
        let existing = OutPoint {
            hash: transaction_hash,
            index: 0,
        };
        let missing = OutPoint {
            hash: transaction_hash,
            index: u32::MAX,
        };

        let outputs = indexer
            .transparent_outputs(&[existing, missing, existing])
            .expect("batch output lookup should succeed");

        assert!(outputs[0].is_some());
        assert!(outputs[1].is_none());
        assert_eq!(outputs[0], outputs[2]);
    }

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
                Default::default(),
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

        let stats = indexer
            .stats()
            .await
            .expect("indexed stats should be queryable");
        assert_eq!(stats.indexed_height.as_deref(), Some("0"));
        assert_eq!(stats.totals.block_count, "1");
        assert_eq!(stats.totals.transaction_count, "1");
        assert_eq!(stats.totals.coinbase_transaction_count, "1");
        assert_eq!(
            stats.totals.block_bytes,
            block.zcash_serialized_size().to_string()
        );
        assert_eq!(stats.trailing_24h.block_count, "1");
        assert_eq!(stats.trailing_24h.transaction_count, "1");
        assert_eq!(stats.trailing_24h.transparent_transaction_count, "0");
        assert_eq!(stats.trailing_24h.shielded_transaction_count, "0");
        assert_eq!(stats.trailing_24h.coinbase_transaction_count, "1");
        assert!(stats.trailing_24h.complete);

        let chart = indexer
            .chart_data(ChartDataRequest::default())
            .await
            .expect("indexed chart snapshot should be queryable");
        assert_eq!(chart.entries.len(), 1);
        assert_eq!(chart.entries[0].height, 0);
        assert_eq!(chart.entries[0].interval_block_count, 1);
        assert_eq!(chart.entries[0].interval_transaction_count, 1);
        assert_eq!(chart.entries[0].transparent_coinbase_tx_count, 1);
        assert_eq!(chart.entries[0].interval_total_fees_zat, "0");
        assert!(chart.entries[0]
            .interval_accepted_work
            .parse::<u128>()
            .is_ok_and(|work| work > 0));
        assert_eq!(chart.next_start_date, None);
    }

    #[tokio::test]
    async fn indexes_address_summary_and_history_without_copying_transactions() {
        let directory = TempDir::new().expect("temporary index directory should be created");
        let network = Network::new_regtest(RegtestParameters::default());
        let indexer =
            Indexer::open(directory.path(), network).expect("temporary index should open");
        let address = Address::from_pub_key_hash(NetworkKind::Testnet, [7; 20]);
        let richer_address = Address::from_pub_key_hash(NetworkKind::Testnet, [8; 20]);
        let value: Amount<NonNegative> = 123_456
            .try_into()
            .expect("test output value is within the valid monetary range");
        let richer_value: Amount<NonNegative> = 234_567
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
            outputs: vec![
                Output::new(value, address.script()),
                Output::new(richer_value, richer_address.script()),
            ],
            lock_time,
        });
        block.transactions = vec![coinbase];
        let block = Arc::new(block);

        indexer
            .index_blocks(vec![(
                Height(0),
                block.clone(),
                block.zcash_serialized_size(),
                Default::default(),
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

        let top_balances = indexer
            .top_balances(TopBalancesRequest {
                limit: Some(1),
                cursor: None,
            })
            .await
            .expect("funded address should be ranked");
        assert_eq!(top_balances.entries.len(), 1);
        assert_eq!(top_balances.entries[0].rank, 1);
        assert_eq!(top_balances.entries[0].address, richer_address.to_string());
        assert_eq!(top_balances.entries[0].balance_zat, "234567");
        assert_eq!(top_balances.summary.funded_transparent_address_count, 2);
        assert_eq!(top_balances.summary.top_10_balance_zat, "358023");
        assert_eq!(top_balances.pagination.total, "2");
        assert!(top_balances.pagination.has_next);
        let second_page = indexer
            .top_balances(TopBalancesRequest {
                limit: Some(1),
                cursor: top_balances.pagination.next_cursor,
            })
            .await
            .expect("top-balances cursor should return the next address");
        assert_eq!(second_page.entries.len(), 1);
        assert_eq!(second_page.entries[0].rank, 2);
        assert_eq!(second_page.entries[0].address, address.to_string());
        assert_eq!(second_page.entries[0].balance_zat, "123456");
        assert!(!second_page.pagination.has_next);

        let page = indexer
            .address_transactions_page(address, None, None, PageDirection::Next)
            .await
            .unwrap();
        assert_eq!(page.transactions.len(), 1);
        assert_eq!(page.transactions[0].received_zat, "123456");
        assert_eq!(page.transactions[0].sent_zat, "0");
        assert_eq!(page.transactions[0].net_change_zat, "123456");

        let chart = indexer
            .chart_data(ChartDataRequest::default())
            .await
            .expect("funded-address chart snapshot should be queryable");
        assert_eq!(chart.entries[0].funded_transparent_address_count, 2);

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
        let top_balances = indexer
            .top_balances(TopBalancesRequest::default())
            .await
            .expect("rolled-back top balances should be queryable");
        assert!(top_balances.entries.is_empty());
        assert_eq!(top_balances.summary.funded_transparent_address_count, 0);
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
                Default::default(),
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

        let stats = indexer
            .stats()
            .await
            .expect("empty stats should be queryable");
        assert_eq!(stats.indexed_height, None);
        assert_eq!(stats.totals.block_count, "0");
        assert_eq!(stats.totals.transaction_count, "0");
        assert_eq!(stats.totals.coinbase_transaction_count, "0");
        assert_eq!(stats.trailing_24h.block_count, "0");
        assert_eq!(stats.trailing_24h.transaction_count, "0");
        assert_eq!(stats.trailing_24h.transparent_transaction_count, "0");
        assert_eq!(stats.trailing_24h.shielded_transaction_count, "0");
        assert_eq!(stats.trailing_24h.coinbase_transaction_count, "0");
        assert!(stats.trailing_24h.complete);

        let chart = indexer
            .chart_data(ChartDataRequest::default())
            .await
            .expect("rolled-back chart query should succeed");
        assert!(chart.entries.is_empty());
    }
}
