//! Deterministic explorer aggregates derived from verified canonical blocks.

#![cfg(feature = "indexer")]

use std::{collections::HashMap, sync::Arc};

use zakura_chain::{
    amount::{Amount, NegativeAllowed, NonNegative},
    block::{Block, Height},
    parameters::{
        subsidy::{
            block_subsidy, founders_reward, funding_stream_values, is_zip234_active, miner_subsidy,
            parent_nsm_value_balance,
        },
        Network, NetworkUpgrade,
    },
    transaction::{zip317, Transaction},
    transparent::{OutPoint, Utxo},
    value_balance::ValueBalance,
    work::difficulty::U256,
};

use crate::{
    service::finalized_state::explorer_transaction_record_with_utxos, ExplorerBlockStats,
    ExplorerChainStats, ExplorerDailyStats, ExplorerIntervalStats, ExplorerShieldedFlow,
    ExplorerShieldedPool, ExplorerTransactionKind, TransactionLocation,
};

const SECONDS_PER_DAY: i64 = 86_400;
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

#[derive(Default)]
struct BlockTransactionStats {
    transparent: u32,
    shielded: u32,
    coinbase: u32,
    fully_shielded: u32,
    mixed_pool: u32,
    interval: ExplorerIntervalStats,
    has_ironwood_transaction: bool,
}

/// Derives the compact block record shared by durable analytics and live suffix reads.
#[allow(clippy::too_many_arguments)]
pub(super) fn derive_block_stats(
    block: &Arc<Block>,
    height: Height,
    serialized_size: u32,
    value_pools: ValueBalance<NonNegative>,
    spent_utxos: &HashMap<OutPoint, Utxo>,
    previous_pool_nsm: i64,
    funded_transparent_address_count: u64,
    network: &Network,
) -> ExplorerBlockStats {
    let mut transaction_stats = BlockTransactionStats::default();
    let mut total_fees_zat = 0_u64;

    for (transaction_index, transaction) in block.transactions.iter().enumerate() {
        let transaction_utxos = transaction
            .inputs()
            .iter()
            .filter_map(|input| input.outpoint())
            .map(|outpoint| {
                let utxo = spent_utxos
                    .get(&outpoint)
                    .expect("verified block inputs have resolved transparent outputs")
                    .clone();
                (outpoint, utxo)
            })
            .collect::<HashMap<_, _>>();
        let record = explorer_transaction_record_with_utxos(
            transaction,
            transaction_index,
            &transaction_utxos,
        );
        transaction_stats.record(
            height,
            transaction_index,
            record,
            transaction,
            &transaction_utxos,
            network,
        );
        if transaction_index > 0 {
            total_fees_zat = total_fees_zat
                .checked_add(record.fee_zat)
                .expect("verified block fees fit in u64");
        }
    }

    let transaction_count = u32::try_from(block.transactions.len())
        .expect("verified block transaction count fits in u32");
    let accepted_work = block
        .header
        .difficulty_threshold
        .to_work()
        .expect("verified block target converts to work")
        .as_u256();
    assert!(
        accepted_work <= U256::from(u128::MAX),
        "verified block work fits in explorer accumulator"
    );

    let mut interval = transaction_stats.interval;
    interval.block_count = 1;
    interval.transaction_count = u64::from(transaction_count);
    interval.empty_block_count = u64::from(transaction_count <= 1);
    interval.accepted_work = accepted_work.low_u128();
    interval.total_fees_zat = u128::from(total_fees_zat);
    interval.total_block_size_bytes = u64::from(serialized_size);
    interval.ironwood_active_block_count = u64::from(transaction_stats.has_ironwood_transaction);
    add_mining_accounting(
        &mut interval,
        block,
        height,
        total_fees_zat,
        previous_pool_nsm,
        network,
    );

    ExplorerBlockStats {
        height: height.0,
        hash: block.hash(),
        timestamp: block.header.time.timestamp(),
        transaction_count,
        serialized_size,
        difficulty: format!(
            "{:.6}",
            block
                .header
                .difficulty_threshold
                .relative_to_network(network)
        ),
        total_fees_zat,
        transparent_transaction_count: transaction_stats.transparent,
        shielded_transaction_count: transaction_stats.shielded,
        coinbase_transaction_count: transaction_stats.coinbase,
        fully_shielded_transaction_count: transaction_stats.fully_shielded,
        mixed_pool_transaction_count: transaction_stats.mixed_pool,
        funded_transparent_address_count,
        pool_transparent: non_negative_amount(value_pools.transparent_amount()),
        pool_sprout: non_negative_amount(value_pools.sprout_amount()),
        pool_sapling: non_negative_amount(value_pools.sapling_amount()),
        pool_orchard: non_negative_amount(value_pools.orchard_amount()),
        pool_deferred: non_negative_amount(value_pools.deferred_amount()),
        pool_ironwood: non_negative_amount(value_pools.ironwood_amount()),
        pool_nsm: value_pools.nsm_value_balance_amount().zatoshis(),
        total_issuance: non_negative_amount(value_pools.issued_supply()),
        interval,
    }
}

impl BlockTransactionStats {
    fn record(
        &mut self,
        height: Height,
        transaction_index: usize,
        record: crate::ExplorerTransactionRecord,
        transaction: &Transaction,
        spent_utxos: &HashMap<OutPoint, Utxo>,
        network: &Network,
    ) {
        let location = TransactionLocation::from_usize(height, transaction_index);
        let kind = record.kind(location);
        match kind {
            ExplorerTransactionKind::Transparent => increment(&mut self.transparent),
            ExplorerTransactionKind::Shielded => increment(&mut self.shielded),
            ExplorerTransactionKind::Coinbase => increment(&mut self.coinbase),
        }
        let is_fully_shielded = record.flow(location) == Some(ExplorerShieldedFlow::FullyShielded);
        if is_fully_shielded {
            increment(&mut self.fully_shielded);
        }
        let is_mixed_pool = record.pool() == Some(ExplorerShieldedPool::Mixed);
        if is_mixed_pool {
            increment(&mut self.mixed_pool);
        }

        let interval = &mut self.interval;
        match kind {
            ExplorerTransactionKind::Transparent => {
                add_u64(&mut interval.transparent_transaction_count, 1)
            }
            ExplorerTransactionKind::Shielded => {
                add_u64(&mut interval.shielded_transaction_count, 1)
            }
            ExplorerTransactionKind::Coinbase => {
                add_u64(&mut interval.coinbase_transaction_count, 1)
            }
        }
        if is_fully_shielded {
            add_u64(&mut interval.fully_shielded_transaction_count, 1);
        }
        if is_mixed_pool {
            add_u64(&mut interval.mixed_pool_transaction_count, 1);
        }
        add_u64(
            &mut interval.sapling_spend_count,
            u64::from(record.sapling_spend_count),
        );
        add_u64(
            &mut interval.sapling_output_count,
            u64::from(record.sapling_output_count),
        );

        let is_coinbase = transaction.is_coinbase();
        let spends_coinbase = spent_utxos.values().any(|utxo| utxo.from_coinbase);
        if !is_coinbase && spends_coinbase && transaction.has_shielded_outputs() {
            add_u64(&mut interval.shielded_coinbase_migration_tx_count, 1);
        } else if record.ironwood_action_count > 0 {
            add_u64(&mut interval.ironwood_tx_count, 1);
        } else if record.orchard_action_count > 0 {
            add_u64(&mut interval.orchard_tx_count, 1);
        } else if record.sapling_spend_count > 0 || record.sapling_output_count > 0 {
            add_u64(&mut interval.sapling_tx_count, 1);
        } else if record.joinsplit_count > 0 {
            add_u64(&mut interval.sprout_tx_count, 1);
        } else if is_coinbase {
            add_u64(&mut interval.transparent_coinbase_tx_count, 1);
        } else if record.transparent_input_count > 0 || record.transparent_output_count > 0 {
            add_u64(&mut interval.transparent_tx_count, 1);
        }

        add_transaction_flows(interval, &record, transaction, spent_utxos);
        add_ironwood_observatory(interval, height, &record, transaction, network);
        self.has_ironwood_transaction |= record.ironwood_action_count > 0;
    }
}

pub(super) fn day_number(timestamp: i64) -> u32 {
    u32::try_from(timestamp.div_euclid(SECONDS_PER_DAY))
        .expect("canonical block timestamps produce nonnegative UTC day numbers")
}

pub(super) fn add_block_to_daily_stats(
    stats: &mut ExplorerDailyStats,
    block: &ExplorerBlockStats,
    previous_timestamp: Option<i64>,
) {
    let first = stats.interval.block_count == 0;
    if first {
        stats.day = day_number(block.timestamp);
        stats.start_height = block.height;
        let anchor = previous_timestamp.unwrap_or(block.timestamp);
        stats.interval_anchor_timestamp = anchor;
        stats.min_header_timestamp = anchor.min(block.timestamp);
        stats.max_header_timestamp = anchor.max(block.timestamp);
    } else {
        stats.min_header_timestamp = stats.min_header_timestamp.min(block.timestamp);
        stats.max_header_timestamp = stats.max_header_timestamp.max(block.timestamp);
    }
    if previous_timestamp.is_some() {
        add_u64(&mut stats.block_time_interval_count, 1);
    }
    stats.end_height = block.height;
    stats.end_block_hash = block.hash;
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
    stats.end_block_subsidy_zat =
        u64::try_from(block.interval.total_subsidy_zat).expect("block subsidy fits in u64");
    add_interval(&mut stats.interval, &block.interval);
}

pub(super) fn add_block_to_chain_stats(stats: &mut ExplorerChainStats, block: &ExplorerBlockStats) {
    add_u64(&mut stats.block_count, 1);
    add_u64(
        &mut stats.transaction_count,
        u64::from(block.transaction_count),
    );
    add_u64(&mut stats.block_bytes, u64::from(block.serialized_size));
    add_u64(&mut stats.total_fees_zat, block.total_fees_zat);
    add_u64(
        &mut stats.transparent_transaction_count,
        u64::from(block.transparent_transaction_count),
    );
    add_u64(
        &mut stats.shielded_transaction_count,
        u64::from(block.shielded_transaction_count),
    );
    add_u64(
        &mut stats.coinbase_transaction_count,
        u64::from(block.coinbase_transaction_count),
    );
    add_u64(
        &mut stats.fully_shielded_transaction_count,
        u64::from(block.fully_shielded_transaction_count),
    );
    add_u64(
        &mut stats.mixed_pool_transaction_count,
        u64::from(block.mixed_pool_transaction_count),
    );
    stats.funded_transparent_address_count = block.funded_transparent_address_count;
}

pub(super) fn remove_block_from_chain_stats(
    stats: &mut ExplorerChainStats,
    block: &ExplorerBlockStats,
) {
    sub_u64(&mut stats.block_count, 1);
    sub_u64(
        &mut stats.transaction_count,
        u64::from(block.transaction_count),
    );
    sub_u64(&mut stats.block_bytes, u64::from(block.serialized_size));
    sub_u64(&mut stats.total_fees_zat, block.total_fees_zat);
    sub_u64(
        &mut stats.transparent_transaction_count,
        u64::from(block.transparent_transaction_count),
    );
    sub_u64(
        &mut stats.shielded_transaction_count,
        u64::from(block.shielded_transaction_count),
    );
    sub_u64(
        &mut stats.coinbase_transaction_count,
        u64::from(block.coinbase_transaction_count),
    );
    sub_u64(
        &mut stats.fully_shielded_transaction_count,
        u64::from(block.fully_shielded_transaction_count),
    );
    sub_u64(
        &mut stats.mixed_pool_transaction_count,
        u64::from(block.mixed_pool_transaction_count),
    );
}

pub(super) fn add_interval(total: &mut ExplorerIntervalStats, value: &ExplorerIntervalStats) {
    macro_rules! add_u64_fields {
        ($($field:ident),+ $(,)?) => {$(add_u64(&mut total.$field, value.$field);)+};
    }
    macro_rules! add_u128_fields {
        ($($field:ident),+ $(,)?) => {$(add_u128(&mut total.$field, value.$field);)+};
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
        add_u64(total, value);
    }
}

fn add_transaction_flows(
    interval: &mut ExplorerIntervalStats,
    record: &crate::ExplorerTransactionRecord,
    transaction: &Transaction,
    spent_utxos: &HashMap<OutPoint, Utxo>,
) {
    let transparent_inflow = transaction.outputs().iter().fold(0_u128, |total, output| {
        total
            .checked_add(u128::from(output.value().zatoshis().unsigned_abs()))
            .expect("one block transparent inflow fits in u128")
    });
    let transparent_outflow = spent_utxos.values().fold(0_u128, |total, utxo| {
        total
            .checked_add(u128::from(utxo.output.value().zatoshis().unsigned_abs()))
            .expect("one block transparent outflow fits in u128")
    });
    add_u128(&mut interval.transparent_inflow, transparent_inflow);
    add_u128(&mut interval.transparent_outflow, transparent_outflow);

    let sprout_inflow = transaction
        .output_values_to_sprout()
        .fold(0_u128, |total, value| {
            total
                .checked_add(u128::from(value.zatoshis().unsigned_abs()))
                .expect("one block Sprout inflow fits in u128")
        });
    let sprout_outflow = transaction
        .input_values_from_sprout()
        .fold(0_u128, |total, value| {
            total
                .checked_add(u128::from(value.zatoshis().unsigned_abs()))
                .expect("one block Sprout outflow fits in u128")
        });
    add_flow(
        sprout_inflow,
        &mut interval.sprout_inflow,
        &mut interval.sprout_inflow_transaction_count,
    );
    add_flow(
        sprout_outflow,
        &mut interval.sprout_outflow,
        &mut interval.sprout_outflow_transaction_count,
    );
    add_signed_flow(
        record.sapling_value_balance_zat,
        &mut interval.sapling_inflow,
        &mut interval.sapling_outflow,
        &mut interval.sapling_inflow_transaction_count,
        &mut interval.sapling_outflow_transaction_count,
    );
    add_signed_flow(
        record.orchard_value_balance_zat,
        &mut interval.orchard_inflow,
        &mut interval.orchard_outflow,
        &mut interval.orchard_inflow_transaction_count,
        &mut interval.orchard_outflow_transaction_count,
    );
    add_signed_flow(
        record.ironwood_value_balance_zat,
        &mut interval.ironwood_inflow,
        &mut interval.ironwood_outflow,
        &mut interval.ironwood_inflow_transaction_count,
        &mut interval.ironwood_outflow_transaction_count,
    );
}

fn add_ironwood_observatory(
    interval: &mut ExplorerIntervalStats,
    height: Height,
    record: &crate::ExplorerTransactionRecord,
    transaction: &Transaction,
    network: &Network,
) {
    if transaction.version() == 6 {
        add_u64(&mut interval.v6_transaction_count, 1);
    }
    let has_orchard = record.orchard_action_count > 0;
    let has_ironwood = record.ironwood_action_count > 0;
    if has_orchard {
        add_u64(&mut interval.orchard_bundle_transaction_count, 1);
    }
    if has_ironwood {
        add_u64(&mut interval.ironwood_bundle_transaction_count, 1);
    }
    if has_orchard && has_ironwood {
        add_u64(&mut interval.orchard_ironwood_transaction_count, 1);
    }
    add_u64(
        &mut interval.orchard_action_count,
        u64::from(record.orchard_action_count),
    );
    add_u64(
        &mut interval.ironwood_action_count,
        u64::from(record.ironwood_action_count),
    );

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
        return;
    }

    add_u64(
        &mut interval.observable_orchard_to_ironwood_transaction_count,
        1,
    );
    let credit = record.ironwood_value_balance_zat.unsigned_abs();
    add_u128(
        &mut interval.observable_orchard_to_ironwood_value_zat,
        u128::from(credit),
    );
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
        return;
    }
    add_u64(&mut interval.zip318_action_shape_transaction_count, 1);
    let Some(denomination_index) = IRONWOOD_CANONICAL_DENOMINATIONS_ZAT
        .iter()
        .position(|denomination| *denomination == credit)
    else {
        return;
    };
    add_u64(&mut interval.zip318_denomination_transaction_count, 1);
    add_u64(
        &mut interval.ironwood_canonical_denomination_counts[denomination_index],
        1,
    );
    if i128::from(record.fee_zat) != i128::from(zip317::conventional_fee(transaction).zatoshis()) {
        return;
    }
    add_u64(&mut interval.zip318_fee_transaction_count, 1);
    if has_zip318_schedule_shape(
        network,
        height,
        transaction.raw_lock_time(),
        transaction.expiry_height().map(|height| height.0),
    ) {
        add_u64(&mut interval.zip318_schedule_transaction_count, 1);
    }
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
    let earliest = (activation_height.0 / ZIP318_ANCHOR_MODULUS)
        .checked_add(2)
        .and_then(|bucket| bucket.checked_mul(ZIP318_ANCHOR_MODULUS));
    let range = expiry_height
        .checked_sub(2 * ZIP318_EXPIRY_MODULUS)
        .and_then(|start| {
            expiry_height
                .checked_sub(ZIP318_EXPIRY_MODULUS + 1)
                .map(|end| (start, end))
        });
    raw_lock_time == 0
        && expiry_height % ZIP318_EXPIRY_MODULUS == 0
        && expiry_height >= inclusion_height.0
        && earliest.zip(range).is_some_and(|(earliest, (start, end))| {
            inclusion_height.0 >= earliest && end >= earliest && start <= inclusion_height.0
        })
}

fn add_mining_accounting(
    interval: &mut ExplorerIntervalStats,
    block: &Block,
    height: Height,
    total_fees_zat: u64,
    previous_pool_nsm: i64,
    network: &Network,
) {
    let parent_nsm = if is_zip234_active(network, height) {
        Some(
            parent_nsm_value_balance(
                Amount::<NegativeAllowed>::try_from(previous_pool_nsm)
                    .expect("verified parent NSM is in the valid money range"),
            )
            .expect("verified parent NSM converts to a subsidy input"),
        )
    } else {
        None
    };
    let subsidy = block_subsidy(height, network, parent_nsm)
        .expect("verified block height has a valid subsidy");
    let founder_reward = founders_reward(network, height);
    let funding = funding_stream_values(height, network, subsidy)
        .expect("verified block subsidy has valid funding streams");
    let mut direct_funding_streams_zat = 0_u128;
    let mut deferred_subsidy_zat = 0_u128;
    for (receiver, value) in funding {
        let value = u128::from(non_negative_amount(value));
        if receiver.is_deferred() {
            add_u128(&mut deferred_subsidy_zat, value);
        } else {
            add_u128(&mut direct_funding_streams_zat, value);
        }
    }
    let miner = miner_subsidy(height, network, subsidy)
        .expect("verified block subsidy has a valid miner share");
    let coinbase = block
        .transactions
        .first()
        .expect("verified blocks contain a coinbase transaction");
    let transparent_output = coinbase.outputs().iter().fold(0_u128, |total, output| {
        total
            .checked_add(u128::from(output.value().zatoshis().unsigned_abs()))
            .expect("coinbase outputs fit in u128")
    });
    // Direct state-write tests can construct blocks that bypass consensus validation.
    // Count only shielded coinbase deposits here so explorer accounting never makes the
    // canonical state write path stricter than consensus verification.
    let shielded_output = |value: i64| u128::from(value.min(0).unsigned_abs());
    let sapling_output =
        shielded_output(coinbase.sapling_value_balance().sapling_amount().zatoshis());
    let orchard_output =
        shielded_output(coinbase.orchard_value_balance().orchard_amount().zatoshis());
    let ironwood_output = shielded_output(
        coinbase
            .ironwood_value_balance()
            .ironwood_amount()
            .zatoshis(),
    );
    let observed_outputs = transparent_output
        .checked_add(sapling_output)
        .and_then(|value| value.checked_add(orchard_output))
        .and_then(|value| value.checked_add(ironwood_output))
        .expect("coinbase outputs fit in u128");
    let subsidy_zat = u128::from(non_negative_amount(subsidy));
    let lockbox_disbursement_zat = u128::from(non_negative_amount(
        network.lockbox_disbursement_total_amount(height),
    ));
    let allowed_outputs = subsidy_zat
        .checked_sub(deferred_subsidy_zat)
        .and_then(|value| value.checked_add(u128::from(total_fees_zat)))
        .and_then(|value| value.checked_add(lockbox_disbursement_zat))
        .expect("verified coinbase allowed output fits in u128");
    let unclaimed = allowed_outputs.saturating_sub(observed_outputs);

    interval.total_subsidy_zat = subsidy_zat;
    interval.miner_subsidy_zat = u128::from(non_negative_amount(miner));
    interval.founders_reward_zat = u128::from(non_negative_amount(founder_reward));
    interval.funding_streams_zat = direct_funding_streams_zat;
    interval.deferred_subsidy_zat = deferred_subsidy_zat;
    interval.lockbox_disbursement_zat = lockbox_disbursement_zat;
    interval.coinbase_output_transparent_zat = transparent_output;
    interval.coinbase_output_sapling_zat = sapling_output;
    interval.coinbase_output_orchard_zat = orchard_output;
    interval.coinbase_output_ironwood_zat = ironwood_output;
    interval.coinbase_unclaimed_zat = unclaimed;
}

fn add_signed_flow(
    value: i64,
    inflow: &mut u128,
    outflow: &mut u128,
    inflow_count: &mut u64,
    outflow_count: &mut u64,
) {
    if value < 0 {
        add_flow(u128::from(value.unsigned_abs()), inflow, inflow_count);
    } else if value > 0 {
        add_flow(u128::from(value.unsigned_abs()), outflow, outflow_count);
    }
}

fn add_flow(value: u128, total: &mut u128, count: &mut u64) {
    add_u128(total, value);
    if value > 0 {
        add_u64(count, 1);
    }
}

fn non_negative_amount(amount: Amount<NonNegative>) -> u64 {
    u64::try_from(amount.zatoshis()).expect("nonnegative Zcash amount fits in u64")
}

fn increment(value: &mut u32) {
    *value = value
        .checked_add(1)
        .expect("block component count fits in u32");
}

fn add_u64(current: &mut u64, value: u64) {
    *current = current
        .checked_add(value)
        .expect("explorer u64 aggregate does not overflow");
}

fn sub_u64(current: &mut u64, value: u64) {
    *current = current
        .checked_sub(value)
        .expect("explorer rollback only subtracts committed aggregates");
}

fn add_u128(current: &mut u128, value: u128) {
    *current = current
        .checked_add(value)
        .expect("explorer u128 aggregate does not overflow");
}
