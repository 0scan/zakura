//! Atomic all-time aggregate transitions.

use std::collections::HashMap;

use rocksdb::WriteBatch;
use zakura_chain::{
    block::Height,
    parameters::{Network, NetworkUpgrade},
    transaction::{zip317, Transaction},
    transparent::{OutPoint, Utxo},
};

use crate::{
    database::{DatabaseColumn, MetadataKey},
    models::{ChainStatsRecord, DailyStatsRecord, IndexedBlockRecord, TransactionRecord},
    stats::day_key,
    transactions::{shielded_flow, shielded_pool, transaction_kind},
    types::{ShieldedFlow, ShieldedPool, TransactionKind},
    Error, Indexer,
};

/// Transaction classifications accumulated while one block is prepared.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct BlockTransactionStats {
    pub(crate) transparent: u32,
    pub(crate) shielded: u32,
    pub(crate) coinbase: u32,
    pub(crate) fully_shielded: u32,
    pub(crate) mixed_pool: u32,
    pub(crate) interval: crate::models::IntervalStatsRecord,
    pub(crate) has_ironwood_transaction: bool,
}

impl BlockTransactionStats {
    pub(crate) fn record(
        &mut self,
        record: &TransactionRecord,
        transaction: &Transaction,
        spent_utxos: &HashMap<OutPoint, Utxo>,
        network: &Network,
    ) -> Result<(), Error> {
        let kind = transaction_kind(record);
        match kind {
            TransactionKind::Transparent => increment(&mut self.transparent, "transparent")?,
            TransactionKind::Shielded => increment(&mut self.shielded, "shielded")?,
            TransactionKind::Coinbase => increment(&mut self.coinbase, "coinbase")?,
        }
        let is_fully_shielded = shielded_flow(record)? == Some(ShieldedFlow::FullyShielded);
        if is_fully_shielded {
            increment(&mut self.fully_shielded, "fully shielded")?;
        }
        let is_mixed_pool = shielded_pool(record) == Some(ShieldedPool::Mixed);
        if is_mixed_pool {
            increment(&mut self.mixed_pool, "mixed-pool")?;
        }

        let interval = &mut self.interval;
        match kind {
            TransactionKind::Transparent => add_u64(
                &mut interval.transparent_transaction_count,
                1,
                "transparent transaction count",
            )?,
            TransactionKind::Shielded => add_u64(
                &mut interval.shielded_transaction_count,
                1,
                "shielded transaction count",
            )?,
            TransactionKind::Coinbase => add_u64(
                &mut interval.coinbase_transaction_count,
                1,
                "coinbase transaction count",
            )?,
        }
        if is_fully_shielded {
            add_u64(
                &mut interval.fully_shielded_transaction_count,
                1,
                "fully shielded transaction count",
            )?;
        }
        if is_mixed_pool {
            add_u64(
                &mut interval.mixed_pool_transaction_count,
                1,
                "mixed-pool transaction count",
            )?;
        }
        add_u64(
            &mut interval.sapling_spend_count,
            u64::from(record.sapling_spend_count),
            "Sapling spend count",
        )?;
        add_u64(
            &mut interval.sapling_output_count,
            u64::from(record.sapling_output_count),
            "Sapling output count",
        )?;
        let is_coinbase = transaction.is_coinbase();
        let spends_coinbase = spent_utxos.values().any(|utxo| utxo.from_coinbase);
        if !is_coinbase && spends_coinbase && transaction.has_shielded_outputs() {
            add_u64(
                &mut interval.shielded_coinbase_migration_tx_count,
                1,
                "shielded coinbase migration transaction count",
            )?;
        } else if record.ironwood_action_count > 0 {
            add_u64(
                &mut interval.ironwood_tx_count,
                1,
                "Ironwood transaction count",
            )?;
        } else if record.orchard_action_count > 0 {
            add_u64(
                &mut interval.orchard_tx_count,
                1,
                "Orchard transaction count",
            )?;
        } else if record.sapling_spend_count > 0 || record.sapling_output_count > 0 {
            add_u64(
                &mut interval.sapling_tx_count,
                1,
                "Sapling transaction count",
            )?;
        } else if record.joinsplit_count > 0 {
            add_u64(&mut interval.sprout_tx_count, 1, "Sprout transaction count")?;
        } else if is_coinbase {
            add_u64(
                &mut interval.transparent_coinbase_tx_count,
                1,
                "transparent coinbase transaction count",
            )?;
        } else if record.transparent_input_count > 0 || record.transparent_output_count > 0 {
            add_u64(
                &mut interval.transparent_tx_count,
                1,
                "transparent transaction count",
            )?;
        }

        add_transaction_flows(interval, record, transaction, spent_utxos)?;
        add_ironwood_observatory(interval, record, transaction, network)?;
        self.has_ironwood_transaction |= record.ironwood_action_count > 0;
        Ok(())
    }
}

impl Indexer {
    pub(crate) fn chain_stats_record(&self) -> Result<ChainStatsRecord, Error> {
        self.database
            .get(DatabaseColumn::Metadata, MetadataKey::ChainStats.as_bytes())?
            .map(|bytes| serde_json::from_slice(&bytes).map_err(Error::from))
            .transpose()
            .map(|record| record.unwrap_or_default())
    }

    pub(crate) fn prepare_chain_stats_write(
        &self,
        batch: &mut WriteBatch,
        stats: ChainStatsRecord,
    ) -> Result<(), Error> {
        self.database.insert(
            batch,
            DatabaseColumn::Metadata,
            MetadataKey::ChainStats.as_bytes(),
            serde_json::to_vec(&stats)?,
        );
        Ok(())
    }

    pub(crate) fn daily_stats_record(&self, day: u32) -> Result<Option<DailyStatsRecord>, Error> {
        self.database
            .get(DatabaseColumn::DailyStats, day_key(day))?
            .map(|bytes| serde_json::from_slice(&bytes).map_err(Error::from))
            .transpose()
    }

    pub(crate) fn prepare_daily_stats_write(
        &self,
        batch: &mut WriteBatch,
        day: u32,
        stats: &DailyStatsRecord,
    ) -> Result<(), Error> {
        self.database.insert(
            batch,
            DatabaseColumn::DailyStats,
            day_key(day),
            serde_json::to_vec(stats)?,
        );
        Ok(())
    }

    pub(crate) fn add_block_to_daily_stats(
        &self,
        stats: &mut DailyStatsRecord,
        block: &IndexedBlockRecord,
        hash: zakura_chain::block::Hash,
        previous_timestamp: Option<i64>,
    ) -> Result<(), Error> {
        let first = stats.interval.block_count == 0;
        if first {
            stats.start_height = block.height.0;
            let anchor_timestamp = previous_timestamp.unwrap_or(block.timestamp);
            stats.interval_anchor_timestamp = anchor_timestamp;
            stats.min_header_timestamp = anchor_timestamp.min(block.timestamp);
            stats.max_header_timestamp = anchor_timestamp.max(block.timestamp);
        } else {
            stats.min_header_timestamp = stats.min_header_timestamp.min(block.timestamp);
            stats.max_header_timestamp = stats.max_header_timestamp.max(block.timestamp);
        }
        if previous_timestamp.is_some() {
            stats.block_time_interval_count = stats
                .block_time_interval_count
                .checked_add(1)
                .ok_or_else(|| {
                    Error::Calculation("daily block intervals exceed u64".to_string())
                })?;
        }

        stats.end_height = block.height.0;
        stats.end_block_hash = hash.to_string();
        stats.block_timestamp = block.timestamp;
        stats.difficulty.clone_from(&block.difficulty);
        stats.funded_transparent_address_count = block.funded_transparent_address_count;
        stats.pool_transparent = block.pool_transparent;
        stats.pool_sprout = block.pool_sprout;
        stats.pool_sapling = block.pool_sapling;
        stats.pool_orchard = block.pool_orchard;
        stats.pool_deferred = block.pool_deferred;
        stats.pool_ironwood = block.pool_ironwood;
        stats.total_issuance = block.total_issuance;
        stats.end_block_subsidy_zat = u64::try_from(block.interval.total_subsidy_zat)
            .map_err(|_| Error::Calculation("block subsidy exceeds u64".to_string()))?;
        add_interval(&mut stats.interval, &block.interval)?;
        Ok(())
    }

    pub(crate) fn add_block_to_chain_stats(
        &self,
        stats: &mut ChainStatsRecord,
        block: &IndexedBlockRecord,
    ) -> Result<(), Error> {
        apply_block(stats, block, AggregateDirection::Add)
    }

    pub(crate) fn remove_block_from_chain_stats(
        &self,
        stats: &mut ChainStatsRecord,
        block: &IndexedBlockRecord,
    ) -> Result<(), Error> {
        apply_block(stats, block, AggregateDirection::Remove)
    }
}

#[derive(Clone, Copy)]
enum AggregateDirection {
    Add,
    Remove,
}

fn apply_block(
    stats: &mut ChainStatsRecord,
    block: &IndexedBlockRecord,
    direction: AggregateDirection,
) -> Result<(), Error> {
    update(&mut stats.block_count, 1, "block count", direction)?;
    update(
        &mut stats.transaction_count,
        u64::from(block.transaction_count),
        "transaction count",
        direction,
    )?;
    update(
        &mut stats.block_bytes,
        u64::from(block.serialized_size),
        "block bytes",
        direction,
    )?;
    update(
        &mut stats.total_fees_zat,
        block.total_fees_zat,
        "total fees",
        direction,
    )?;
    update(
        &mut stats.transparent_transaction_count,
        u64::from(block.transparent_transaction_count),
        "transparent transaction count",
        direction,
    )?;
    update(
        &mut stats.shielded_transaction_count,
        u64::from(block.shielded_transaction_count),
        "shielded transaction count",
        direction,
    )?;
    update(
        &mut stats.coinbase_transaction_count,
        u64::from(block.coinbase_transaction_count),
        "coinbase transaction count",
        direction,
    )?;
    update(
        &mut stats.fully_shielded_transaction_count,
        u64::from(block.fully_shielded_transaction_count),
        "fully shielded transaction count",
        direction,
    )?;
    update(
        &mut stats.mixed_pool_transaction_count,
        u64::from(block.mixed_pool_transaction_count),
        "mixed-pool transaction count",
        direction,
    )
}

fn update(
    current: &mut u64,
    value: u64,
    field: &str,
    direction: AggregateDirection,
) -> Result<(), Error> {
    *current = match direction {
        AggregateDirection::Add => current
            .checked_add(value)
            .ok_or_else(|| Error::Calculation(format!("chain stats {field} exceeds u64")))?,
        AggregateDirection::Remove => current.checked_sub(value).ok_or_else(|| {
            Error::CorruptData(format!("chain stats {field} underflowed during rollback"))
        })?,
    };
    Ok(())
}

fn increment(value: &mut u32, category: &str) -> Result<(), Error> {
    *value = value.checked_add(1).ok_or_else(|| {
        Error::Calculation(format!("block {category} transaction count exceeds u32"))
    })?;
    Ok(())
}

const ZIP318_EXPIRY_MODULUS: u32 = 34_560;
const ZIP318_ANCHOR_MODULUS: u32 = 144;
const IRONWOOD_CANONICAL_DENOMINATIONS_ZAT: [u64; 19] = [
    1_000_000,
    2_000_000,
    5_000_000,
    10_000_000,
    20_000_000,
    50_000_000,
    100_000_000,
    200_000_000,
    500_000_000,
    1_000_000_000,
    2_000_000_000,
    5_000_000_000,
    10_000_000_000,
    20_000_000_000,
    50_000_000_000,
    100_000_000_000,
    200_000_000_000,
    500_000_000_000,
    1_000_000_000_000,
];

fn add_transaction_flows(
    interval: &mut crate::models::IntervalStatsRecord,
    record: &TransactionRecord,
    transaction: &Transaction,
    spent_utxos: &HashMap<OutPoint, Utxo>,
) -> Result<(), Error> {
    let transparent_inflow = transaction
        .outputs()
        .iter()
        .try_fold(0_u128, |total, output| {
            total
                .checked_add(u128::from(output.value().zatoshis().unsigned_abs()))
                .ok_or_else(|| Error::Calculation("transparent inflow exceeds u128".to_string()))
        })?;
    let transparent_outflow = spent_utxos.values().try_fold(0_u128, |total, utxo| {
        total
            .checked_add(u128::from(utxo.output.value().zatoshis().unsigned_abs()))
            .ok_or_else(|| Error::Calculation("transparent outflow exceeds u128".to_string()))
    })?;
    add_u128(
        &mut interval.transparent_inflow,
        transparent_inflow,
        "transparent inflow",
    )?;
    add_u128(
        &mut interval.transparent_outflow,
        transparent_outflow,
        "transparent outflow",
    )?;

    let sprout_inflow =
        transaction
            .output_values_to_sprout()
            .try_fold(0_u128, |total, value| {
                total
                    .checked_add(u128::from(value.zatoshis().unsigned_abs()))
                    .ok_or_else(|| Error::Calculation("Sprout inflow exceeds u128".to_string()))
            })?;
    let sprout_outflow =
        transaction
            .input_values_from_sprout()
            .try_fold(0_u128, |total, value| {
                total
                    .checked_add(u128::from(value.zatoshis().unsigned_abs()))
                    .ok_or_else(|| Error::Calculation("Sprout outflow exceeds u128".to_string()))
            })?;
    add_flow(
        sprout_inflow,
        &mut interval.sprout_inflow,
        &mut interval.sprout_inflow_transaction_count,
        "Sprout inflow",
    )?;
    add_flow(
        sprout_outflow,
        &mut interval.sprout_outflow,
        &mut interval.sprout_outflow_transaction_count,
        "Sprout outflow",
    )?;
    add_signed_flow(
        record.sapling_value_balance_zat,
        &mut interval.sapling_inflow,
        &mut interval.sapling_outflow,
        &mut interval.sapling_inflow_transaction_count,
        &mut interval.sapling_outflow_transaction_count,
        "Sapling",
    )?;
    add_signed_flow(
        record.orchard_value_balance_zat,
        &mut interval.orchard_inflow,
        &mut interval.orchard_outflow,
        &mut interval.orchard_inflow_transaction_count,
        &mut interval.orchard_outflow_transaction_count,
        "Orchard",
    )?;
    add_signed_flow(
        record.ironwood_value_balance_zat,
        &mut interval.ironwood_inflow,
        &mut interval.ironwood_outflow,
        &mut interval.ironwood_inflow_transaction_count,
        &mut interval.ironwood_outflow_transaction_count,
        "Ironwood",
    )
}

fn add_ironwood_observatory(
    interval: &mut crate::models::IntervalStatsRecord,
    record: &TransactionRecord,
    transaction: &Transaction,
    network: &Network,
) -> Result<(), Error> {
    if transaction.version() == 6 {
        add_u64(
            &mut interval.v6_transaction_count,
            1,
            "v6 transaction count",
        )?;
    }
    let has_orchard = record.orchard_action_count > 0;
    let has_ironwood = record.ironwood_action_count > 0;
    if has_orchard {
        add_u64(
            &mut interval.orchard_bundle_transaction_count,
            1,
            "Orchard bundle transaction count",
        )?;
    }
    if has_ironwood {
        add_u64(
            &mut interval.ironwood_bundle_transaction_count,
            1,
            "Ironwood bundle transaction count",
        )?;
    }
    if has_orchard && has_ironwood {
        add_u64(
            &mut interval.orchard_ironwood_transaction_count,
            1,
            "Orchard-Ironwood transaction count",
        )?;
    }
    add_u64(
        &mut interval.orchard_action_count,
        u64::from(record.orchard_action_count),
        "Orchard action count",
    )?;
    add_u64(
        &mut interval.ironwood_action_count,
        u64::from(record.ironwood_action_count),
        "Ironwood action count",
    )?;

    let direct_crossing = !transaction.is_coinbase()
        && has_orchard
        && has_ironwood
        && record.orchard_value_balance_zat > 0
        && record.ironwood_value_balance_zat < 0
        && record.transparent_input_count == 0
        && record.transparent_output_count == 0
        && record.joinsplit_count == 0
        && record.sapling_spend_count == 0
        && record.sapling_output_count == 0;
    if !direct_crossing {
        return Ok(());
    }

    add_u64(
        &mut interval.observable_orchard_to_ironwood_transaction_count,
        1,
        "observable Orchard-to-Ironwood transaction count",
    )?;
    let credit = record.ironwood_value_balance_zat.unsigned_abs();
    add_u128(
        &mut interval.observable_orchard_to_ironwood_value_zat,
        u128::from(credit),
        "observable Orchard-to-Ironwood value",
    )?;

    let orchard_flags = transaction.orchard_flags();
    let ironwood_flags = transaction.ironwood_flags();
    let action_shape = transaction.version() == 6
        && record.orchard_action_count == 2
        && record.ironwood_action_count == 1
        && orchard_flags.is_some_and(|flags| {
            flags.contains(zakura_chain::orchard::Flags::ENABLE_SPENDS)
                && flags.contains(zakura_chain::orchard::Flags::ENABLE_OUTPUTS)
        })
        && ironwood_flags.is_some_and(|flags| {
            !flags.contains(zakura_chain::ironwood::Flags::ENABLE_SPENDS)
                && flags.contains(zakura_chain::ironwood::Flags::ENABLE_OUTPUTS)
        });
    if !action_shape {
        return Ok(());
    }
    add_u64(
        &mut interval.zip318_action_shape_transaction_count,
        1,
        "ZIP-318 action-shape transaction count",
    )?;

    let Some(denomination_index) = IRONWOOD_CANONICAL_DENOMINATIONS_ZAT
        .iter()
        .position(|denomination| *denomination == credit)
    else {
        return Ok(());
    };
    add_u64(
        &mut interval.zip318_denomination_transaction_count,
        1,
        "ZIP-318 denomination transaction count",
    )?;
    add_u64(
        &mut interval.ironwood_canonical_denomination_counts[denomination_index],
        1,
        "Ironwood denomination count",
    )?;

    let conventional_fee = zip317::conventional_fee(transaction).zatoshis();
    if i128::from(record.fee_zat) != i128::from(conventional_fee) {
        return Ok(());
    }
    add_u64(
        &mut interval.zip318_fee_transaction_count,
        1,
        "ZIP-318 fee transaction count",
    )?;
    if has_zip318_schedule_shape(
        network,
        record.position.height,
        transaction.raw_lock_time(),
        transaction.expiry_height().map(|height| height.0),
    ) {
        add_u64(
            &mut interval.zip318_schedule_transaction_count,
            1,
            "ZIP-318 schedule transaction count",
        )?;
    }
    Ok(())
}

fn has_zip318_schedule_shape(
    network: &Network,
    inclusion_height: Height,
    raw_lock_time: u32,
    expiry_height: Option<u32>,
) -> bool {
    let (Some(expiry_height), Some(activation_height)) = (
        expiry_height,
        NetworkUpgrade::Nu6_3.activation_height(network),
    ) else {
        return false;
    };
    let earliest_schedule_height = (activation_height.0 / ZIP318_ANCHOR_MODULUS)
        .checked_add(2)
        .and_then(|bucket| bucket.checked_mul(ZIP318_ANCHOR_MODULUS));
    let scheduled_height_range = expiry_height
        .checked_sub(2 * ZIP318_EXPIRY_MODULUS)
        .and_then(|start| {
            expiry_height
                .checked_sub(ZIP318_EXPIRY_MODULUS + 1)
                .map(|end| (start, end))
        });

    raw_lock_time == 0
        && expiry_height % ZIP318_EXPIRY_MODULUS == 0
        && expiry_height >= inclusion_height.0
        && earliest_schedule_height
            .zip(scheduled_height_range)
            .is_some_and(|(earliest, (start, end))| {
                inclusion_height.0 >= earliest && end >= earliest && start <= inclusion_height.0
            })
}

fn add_interval(
    total: &mut crate::models::IntervalStatsRecord,
    value: &crate::models::IntervalStatsRecord,
) -> Result<(), Error> {
    macro_rules! add_u64_fields {
        ($($field:ident),+ $(,)?) => {$({
            add_u64(&mut total.$field, value.$field, stringify!($field))?;
        })+};
    }
    macro_rules! add_u128_fields {
        ($($field:ident),+ $(,)?) => {$({
            add_u128(&mut total.$field, value.$field, stringify!($field))?;
        })+};
    }
    add_u64_fields!(
        block_count,
        transaction_count,
        empty_block_count,
        total_block_size_bytes,
        transparent_tx_count,
        transparent_coinbase_tx_count,
        shielded_coinbase_migration_tx_count,
        sprout_tx_count,
        sapling_tx_count,
        orchard_tx_count,
        ironwood_tx_count,
        transparent_transaction_count,
        shielded_transaction_count,
        coinbase_transaction_count,
        fully_shielded_transaction_count,
        mixed_pool_transaction_count,
        sapling_spend_count,
        sapling_output_count,
        sprout_inflow_transaction_count,
        sprout_outflow_transaction_count,
        sapling_inflow_transaction_count,
        sapling_outflow_transaction_count,
        orchard_inflow_transaction_count,
        orchard_outflow_transaction_count,
        ironwood_inflow_transaction_count,
        ironwood_outflow_transaction_count,
        v6_transaction_count,
        ironwood_bundle_transaction_count,
        orchard_bundle_transaction_count,
        orchard_ironwood_transaction_count,
        orchard_action_count,
        ironwood_action_count,
        ironwood_active_block_count,
        observable_orchard_to_ironwood_transaction_count,
        zip318_action_shape_transaction_count,
        zip318_denomination_transaction_count,
        zip318_fee_transaction_count,
        zip318_schedule_transaction_count,
    );
    add_u128_fields!(
        accepted_work,
        total_fees_zat,
        total_subsidy_zat,
        miner_subsidy_zat,
        founders_reward_zat,
        funding_streams_zat,
        deferred_subsidy_zat,
        lockbox_disbursement_zat,
        coinbase_output_transparent_zat,
        coinbase_output_sapling_zat,
        coinbase_output_orchard_zat,
        coinbase_output_ironwood_zat,
        coinbase_unclaimed_zat,
        transparent_inflow,
        transparent_outflow,
        sprout_inflow,
        sprout_outflow,
        sapling_inflow,
        sapling_outflow,
        orchard_inflow,
        orchard_outflow,
        ironwood_inflow,
        ironwood_outflow,
        observable_orchard_to_ironwood_value_zat,
    );
    for (total, value) in total
        .ironwood_canonical_denomination_counts
        .iter_mut()
        .zip(value.ironwood_canonical_denomination_counts)
    {
        add_u64(total, value, "Ironwood denomination count")?;
    }
    Ok(())
}

fn add_signed_flow(
    value: i64,
    inflow: &mut u128,
    outflow: &mut u128,
    inflow_transaction_count: &mut u64,
    outflow_transaction_count: &mut u64,
    pool: &str,
) -> Result<(), Error> {
    if value < 0 {
        add_flow(
            u128::from(value.unsigned_abs()),
            inflow,
            inflow_transaction_count,
            &format!("{pool} inflow"),
        )
    } else if value > 0 {
        add_flow(
            u128::from(value.unsigned_abs()),
            outflow,
            outflow_transaction_count,
            &format!("{pool} outflow"),
        )
    } else {
        Ok(())
    }
}

fn add_flow(
    value: u128,
    total: &mut u128,
    transaction_count: &mut u64,
    field: &str,
) -> Result<(), Error> {
    add_u128(total, value, field)?;
    if value > 0 {
        add_u64(transaction_count, 1, &format!("{field} transaction count"))?;
    }
    Ok(())
}

fn add_u64(current: &mut u64, value: u64, field: &str) -> Result<(), Error> {
    *current = current
        .checked_add(value)
        .ok_or_else(|| Error::Calculation(format!("{field} exceeds u64")))?;
    Ok(())
}

fn add_u128(current: &mut u128, value: u128, field: &str) -> Result<(), Error> {
    *current = current
        .checked_add(value)
        .ok_or_else(|| Error::Calculation(format!("{field} exceeds u128")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daily_interval_adds_privacy_and_sapling_protocol_counts() {
        let mut total = crate::models::IntervalStatsRecord {
            fully_shielded_transaction_count: 2,
            mixed_pool_transaction_count: 3,
            transparent_transaction_count: 4,
            shielded_transaction_count: 6,
            coinbase_transaction_count: 8,
            sapling_spend_count: 5,
            sapling_output_count: 7,
            sapling_inflow_transaction_count: 11,
            sapling_outflow_transaction_count: 13,
            ..Default::default()
        };
        let value = crate::models::IntervalStatsRecord {
            fully_shielded_transaction_count: 11,
            mixed_pool_transaction_count: 13,
            transparent_transaction_count: 17,
            shielded_transaction_count: 19,
            coinbase_transaction_count: 23,
            sapling_spend_count: 17,
            sapling_output_count: 19,
            sapling_inflow_transaction_count: 23,
            sapling_outflow_transaction_count: 29,
            ..Default::default()
        };

        add_interval(&mut total, &value).expect("small interval counts should add");

        assert_eq!(total.fully_shielded_transaction_count, 13);
        assert_eq!(total.mixed_pool_transaction_count, 16);
        assert_eq!(total.transparent_transaction_count, 21);
        assert_eq!(total.shielded_transaction_count, 25);
        assert_eq!(total.coinbase_transaction_count, 31);
        assert_eq!(total.sapling_spend_count, 22);
        assert_eq!(total.sapling_output_count, 26);
        assert_eq!(total.sapling_inflow_transaction_count, 34);
        assert_eq!(total.sapling_outflow_transaction_count, 42);
    }
}
