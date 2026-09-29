//! Public explorer query contracts backed by canonical state.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use zakura_chain::{block, transaction, transparent};

pub use crate::service::finalized_state::{
    ExplorerAddressRecord, ExplorerShieldedFlow, ExplorerShieldedPool, ExplorerTransactionKind,
    ExplorerTransactionRecord,
};
use crate::TransactionLocation;

/// Top-level transaction category selector.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ExplorerTransactionKindFilter {
    #[default]
    /// Match every transaction kind.
    All,
    /// Match transactions that touch at least one shielded pool.
    Shielded,
    /// Match non-coinbase transactions with only transparent components.
    Transparent,
    /// Match coinbase transactions.
    Coinbase,
}

/// Observable shielded-flow selector.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ExplorerShieldedFlowFilter {
    #[default]
    /// Match every shielded-flow direction.
    All,
    /// Match observable value entering a shielded pool.
    Shield,
    /// Match observable value leaving a shielded pool.
    Deshield,
    /// Match transactions with no transparent inputs or outputs.
    FullyShielded,
    /// Match shielded transactions without a directional public flow.
    Complex,
}

/// Shielded pool selector.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ExplorerShieldedPoolFilter {
    #[default]
    /// Match every shielded pool.
    All,
    /// Match Sprout-only transactions.
    Sprout,
    /// Match Sapling-only transactions.
    Sapling,
    /// Match Orchard-only transactions.
    Orchard,
    /// Match Ironwood-only transactions.
    Ironwood,
    /// Match transactions that touch multiple shielded pools.
    Mixed,
}

/// Indexed public-value threshold selector.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ExplorerAmountFilter {
    #[default]
    /// Do not impose a public-flow amount threshold.
    Any,
    /// Match public flows of at least one billion zatoshis.
    AtLeastOneBillion,
    /// Match public flows of at least ten billion zatoshis.
    AtLeastTenBillion,
    /// Match public flows of at least one hundred billion zatoshis.
    AtLeastOneHundredBillion,
}

/// Cursor traversal direction. Results are always returned newest first.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ExplorerPageDirection {
    #[default]
    /// Traverse toward older chain locations.
    Older,
    /// Traverse toward newer chain locations.
    Newer,
}

/// A validated transaction filter set.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ExplorerTransactionQuery {
    /// Primary transaction-kind selector.
    pub kind: ExplorerTransactionKindFilter,
    /// Observable shielded-flow selector.
    pub flow: ExplorerShieldedFlowFilter,
    /// Shielded-pool selector.
    pub pool: ExplorerShieldedPoolFilter,
    /// Public-flow amount threshold.
    pub amount: ExplorerAmountFilter,
}

impl ExplorerTransactionQuery {
    /// Returns whether selectors requiring shielded metadata are internally valid.
    pub fn is_valid(self) -> bool {
        let uses_shielded_selector = self.flow != ExplorerShieldedFlowFilter::All
            || self.pool != ExplorerShieldedPoolFilter::All
            || self.amount != ExplorerAmountFilter::Any;
        !uses_shielded_selector || self.kind == ExplorerTransactionKindFilter::Shielded
    }

    pub(crate) fn uses_exact_shielded_index(self) -> bool {
        self.kind == ExplorerTransactionKindFilter::Shielded
            && (self.flow != ExplorerShieldedFlowFilter::All
                || self.pool != ExplorerShieldedPoolFilter::All
                || self.amount != ExplorerAmountFilter::Any)
    }

    pub(crate) fn matches(
        self,
        location: TransactionLocation,
        record: ExplorerTransactionRecord,
    ) -> bool {
        let kind_matches = match self.kind {
            ExplorerTransactionKindFilter::All => true,
            ExplorerTransactionKindFilter::Shielded => {
                record.kind(location) == ExplorerTransactionKind::Shielded
            }
            ExplorerTransactionKindFilter::Transparent => {
                record.kind(location) == ExplorerTransactionKind::Transparent
            }
            ExplorerTransactionKindFilter::Coinbase => {
                record.kind(location) == ExplorerTransactionKind::Coinbase
            }
        };
        let flow_matches = match self.flow {
            ExplorerShieldedFlowFilter::All => true,
            ExplorerShieldedFlowFilter::Shield => {
                record.flow(location) == Some(ExplorerShieldedFlow::Shield)
            }
            ExplorerShieldedFlowFilter::Deshield => {
                record.flow(location) == Some(ExplorerShieldedFlow::Deshield)
            }
            ExplorerShieldedFlowFilter::FullyShielded => {
                record.flow(location) == Some(ExplorerShieldedFlow::FullyShielded)
            }
            ExplorerShieldedFlowFilter::Complex => {
                record.flow(location) == Some(ExplorerShieldedFlow::Complex)
            }
        };
        let pool_matches = match self.pool {
            ExplorerShieldedPoolFilter::All => true,
            ExplorerShieldedPoolFilter::Sprout => {
                record.pool() == Some(ExplorerShieldedPool::Sprout)
            }
            ExplorerShieldedPoolFilter::Sapling => {
                record.pool() == Some(ExplorerShieldedPool::Sapling)
            }
            ExplorerShieldedPoolFilter::Orchard => {
                record.pool() == Some(ExplorerShieldedPool::Orchard)
            }
            ExplorerShieldedPoolFilter::Ironwood => {
                record.pool() == Some(ExplorerShieldedPool::Ironwood)
            }
            ExplorerShieldedPoolFilter::Mixed => record.pool() == Some(ExplorerShieldedPool::Mixed),
        };
        let amount_matches = match self.amount {
            ExplorerAmountFilter::Any => true,
            ExplorerAmountFilter::AtLeastOneBillion => record
                .public_flow_amount(location)
                .is_some_and(|amount| amount >= 1_000_000_000),
            ExplorerAmountFilter::AtLeastTenBillion => record
                .public_flow_amount(location)
                .is_some_and(|amount| amount >= 10_000_000_000),
            ExplorerAmountFilter::AtLeastOneHundredBillion => record
                .public_flow_amount(location)
                .is_some_and(|amount| amount >= 100_000_000_000),
        };

        kind_matches && flow_matches && pool_matches && amount_matches
    }
}

/// One canonical transaction summary returned by the state service.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExplorerTransactionSummary {
    /// Canonical transaction location.
    pub location: TransactionLocation,
    /// Mined transaction identifier.
    pub txid: transaction::Hash,
    /// Containing canonical block hash.
    pub block_hash: block::Hash,
    /// Containing block timestamp as Unix seconds.
    pub block_time: i64,
    /// Compact transaction facts.
    pub record: ExplorerTransactionRecord,
}

/// A cursor-adjacent canonical transaction page.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExplorerTransactionPage {
    /// Current best-chain tip represented by the response.
    pub best_tip: Option<(block::Height, block::Hash)>,
    /// Current finalized tip represented by the response.
    pub finalized_tip: Option<(block::Height, block::Hash)>,
    /// Whether the supplied cursor still matches the requested filters.
    pub cursor_valid: bool,
    /// Matching transactions ordered newest first.
    pub transactions: Vec<ExplorerTransactionSummary>,
    /// Whether another page exists in the requested direction.
    pub has_more: bool,
}

/// Block data and compact explorer aggregates returned from one state snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExplorerBlockSummary {
    /// Canonical block body.
    pub block: Arc<block::Block>,
    /// Consensus-serialized block size in bytes.
    pub serialized_size: u32,
    /// Sum of non-coinbase transaction fees in zatoshis.
    pub total_fees_zat: u64,
}

/// Canonical transparent-address state and cursor-adjacent transactions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExplorerAddressPage {
    /// Current best-chain tip represented by the response.
    pub best_tip: Option<(block::Height, block::Hash)>,
    /// Whether the supplied cursor belongs to this address on the canonical chain.
    pub cursor_valid: bool,
    /// Current transparent balance in zatoshis.
    pub balance_zat: u64,
    /// Total recognizable transparent value received in zatoshis.
    pub received_zat: u64,
    /// Compact canonical activity metadata, when the address has activity.
    pub activity: Option<ExplorerAddressRecord>,
    /// Earliest canonical transaction involving the address.
    pub first_seen: Option<ExplorerTransactionSummary>,
    /// Latest canonical transaction involving the address.
    pub last_seen: Option<ExplorerTransactionSummary>,
    /// Earliest canonical transaction that paid the address.
    pub first_funding: Option<ExplorerTransactionSummary>,
    /// Matching transactions, always ordered newest first.
    pub transactions: Vec<ExplorerTransactionSummary>,
    /// Whether another page exists in the requested direction.
    pub has_more: bool,
}

/// Canonical-chain totals maintained atomically with explorer state.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct ExplorerChainStats {
    pub block_count: u64,
    pub transaction_count: u64,
    pub block_bytes: u64,
    pub total_fees_zat: u64,
    pub transparent_transaction_count: u64,
    pub shielded_transaction_count: u64,
    pub coinbase_transaction_count: u64,
    pub fully_shielded_transaction_count: u64,
    pub mixed_pool_transaction_count: u64,
    pub funded_transparent_address_count: u64,
}

/// Additive metrics for one explorer chart interval.
#[allow(missing_docs)]
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct ExplorerIntervalStats {
    pub block_count: u64,
    pub transaction_count: u64,
    pub empty_block_count: u64,
    pub accepted_work: u128,
    pub total_fees_zat: u128,
    pub total_block_size_bytes: u64,
    pub total_subsidy_zat: u128,
    pub miner_subsidy_zat: u128,
    pub founders_reward_zat: u128,
    pub funding_streams_zat: u128,
    pub deferred_subsidy_zat: u128,
    pub lockbox_disbursement_zat: u128,
    pub coinbase_output_transparent_zat: u128,
    pub coinbase_output_sapling_zat: u128,
    pub coinbase_output_orchard_zat: u128,
    pub coinbase_output_ironwood_zat: u128,
    pub coinbase_unclaimed_zat: u128,
    pub transparent_tx_count: u64,
    pub transparent_coinbase_tx_count: u64,
    pub shielded_coinbase_migration_tx_count: u64,
    pub sprout_tx_count: u64,
    pub sapling_tx_count: u64,
    pub orchard_tx_count: u64,
    pub ironwood_tx_count: u64,
    pub transparent_transaction_count: u64,
    pub shielded_transaction_count: u64,
    pub coinbase_transaction_count: u64,
    pub fully_shielded_transaction_count: u64,
    pub mixed_pool_transaction_count: u64,
    pub sapling_spend_count: u64,
    pub sapling_output_count: u64,
    pub transparent_inflow: u128,
    pub transparent_outflow: u128,
    pub sprout_inflow: u128,
    pub sprout_outflow: u128,
    pub sprout_inflow_transaction_count: u64,
    pub sprout_outflow_transaction_count: u64,
    pub sapling_inflow: u128,
    pub sapling_outflow: u128,
    pub sapling_inflow_transaction_count: u64,
    pub sapling_outflow_transaction_count: u64,
    pub orchard_inflow: u128,
    pub orchard_outflow: u128,
    pub orchard_inflow_transaction_count: u64,
    pub orchard_outflow_transaction_count: u64,
    pub ironwood_inflow: u128,
    pub ironwood_outflow: u128,
    pub ironwood_inflow_transaction_count: u64,
    pub ironwood_outflow_transaction_count: u64,
    pub v6_transaction_count: u64,
    pub ironwood_bundle_transaction_count: u64,
    pub orchard_bundle_transaction_count: u64,
    pub orchard_ironwood_transaction_count: u64,
    pub orchard_action_count: u64,
    pub ironwood_action_count: u64,
    pub ironwood_active_block_count: u64,
    pub observable_orchard_to_ironwood_transaction_count: u64,
    pub observable_orchard_to_ironwood_value_zat: u128,
    pub zip318_action_shape_transaction_count: u64,
    pub zip318_denomination_transaction_count: u64,
    pub zip318_fee_transaction_count: u64,
    pub zip318_schedule_transaction_count: u64,
    pub ironwood_canonical_denomination_counts: [u64; 19],
}

/// Explorer analytics derived for one canonical block.
#[allow(missing_docs)]
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct ExplorerBlockStats {
    pub height: u32,
    pub hash: block::Hash,
    pub timestamp: i64,
    pub transaction_count: u32,
    pub serialized_size: u32,
    pub difficulty: String,
    pub total_fees_zat: u64,
    pub transparent_transaction_count: u32,
    pub shielded_transaction_count: u32,
    pub coinbase_transaction_count: u32,
    pub fully_shielded_transaction_count: u32,
    pub mixed_pool_transaction_count: u32,
    pub funded_transparent_address_count: u64,
    pub pool_transparent: u64,
    pub pool_sprout: u64,
    pub pool_sapling: u64,
    pub pool_orchard: u64,
    pub pool_deferred: u64,
    pub pool_ironwood: u64,
    pub pool_nsm: i64,
    pub total_issuance: u64,
    pub interval: ExplorerIntervalStats,
}

/// One UTC day's reversible snapshot and interval facts.
#[allow(missing_docs)]
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct ExplorerDailyStats {
    pub day: u32,
    pub start_height: u32,
    pub end_height: u32,
    pub end_block_hash: block::Hash,
    pub interval_anchor_timestamp: i64,
    pub block_time_interval_count: u64,
    pub block_timestamp: i64,
    pub min_header_timestamp: i64,
    pub max_header_timestamp: i64,
    pub difficulty: String,
    pub funded_transparent_address_count: u64,
    pub pool_transparent: u64,
    pub pool_sprout: u64,
    pub pool_sapling: u64,
    pub pool_orchard: u64,
    pub pool_deferred: u64,
    pub pool_ironwood: u64,
    pub total_issuance: u64,
    pub end_block_subsidy_zat: u64,
    pub interval: ExplorerIntervalStats,
}

impl Default for ExplorerDailyStats {
    fn default() -> Self {
        Self {
            day: 0,
            start_height: 0,
            end_height: 0,
            end_block_hash: block::Hash([0; 32]),
            interval_anchor_timestamp: 0,
            block_time_interval_count: 0,
            block_timestamp: 0,
            min_header_timestamp: 0,
            max_header_timestamp: 0,
            difficulty: String::new(),
            funded_transparent_address_count: 0,
            pool_transparent: 0,
            pool_sprout: 0,
            pool_sapling: 0,
            pool_orchard: 0,
            pool_deferred: 0,
            pool_ironwood: 0,
            total_issuance: 0,
            end_block_subsidy_zat: 0,
            interval: ExplorerIntervalStats::default(),
        }
    }
}

/// Raw trailing-window totals ending at the canonical tip.
#[allow(missing_docs)]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExplorerRollingStats {
    pub complete: bool,
    pub window_start: Option<i64>,
    pub window_end: Option<i64>,
    pub oldest_timestamp: Option<i64>,
    pub totals: ExplorerChainStats,
}

/// One internally consistent canonical analytics snapshot.
#[allow(missing_docs)]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExplorerStatsSnapshot {
    pub best_tip: Option<(block::Height, block::Hash)>,
    pub totals: ExplorerChainStats,
    pub trailing_24h: ExplorerRollingStats,
}

/// Balance-ordered transparent address returned by canonical state.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExplorerBalanceRankEntry {
    pub address: transparent::Address,
    pub balance_zat: u64,
}

/// Stable state cursor for transparent balance ranking.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExplorerBalanceRankCursor {
    pub address: transparent::Address,
    pub balance_zat: u64,
    pub rank: u64,
    pub block_hash: block::Hash,
}

/// One canonical transparent balance ranking page and its summary totals.
#[allow(missing_docs)]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExplorerBalanceRankPage {
    pub best_tip: Option<(block::Height, block::Hash)>,
    pub cursor_valid: bool,
    pub entries: Vec<ExplorerBalanceRankEntry>,
    pub funded_transparent_address_count: u64,
    pub transparent_supply_zat: u64,
    pub top_10_balance_zat: u64,
    pub top_100_balance_zat: u64,
    pub has_more: bool,
}

impl ExplorerTransactionRecord {
    /// Returns the primary explorer category at `location`.
    pub fn kind(self, location: TransactionLocation) -> ExplorerTransactionKind {
        if location.index.as_usize() == 0 {
            ExplorerTransactionKind::Coinbase
        } else if self.pool().is_some() {
            ExplorerTransactionKind::Shielded
        } else {
            ExplorerTransactionKind::Transparent
        }
    }

    /// Returns the single shielded pool or mixed-pool classification.
    pub fn pool(self) -> Option<ExplorerShieldedPool> {
        let pools = [
            (self.joinsplit_count > 0, ExplorerShieldedPool::Sprout),
            (
                self.sapling_spend_count > 0 || self.sapling_output_count > 0,
                ExplorerShieldedPool::Sapling,
            ),
            (self.orchard_action_count > 0, ExplorerShieldedPool::Orchard),
            (
                self.ironwood_action_count > 0,
                ExplorerShieldedPool::Ironwood,
            ),
        ];
        let mut active = pools
            .into_iter()
            .filter_map(|(active, pool)| active.then_some(pool));
        let first = active.next()?;
        Some(if active.next().is_some() {
            ExplorerShieldedPool::Mixed
        } else {
            first
        })
    }

    /// Returns the signed observable shielded-side value balance.
    pub fn shielded_value_balance(self, location: TransactionLocation) -> i64 {
        let balance = if self.kind(location) == ExplorerTransactionKind::Coinbase {
            i128::from(self.sapling_value_balance_zat)
                + i128::from(self.orchard_value_balance_zat)
                + i128::from(self.ironwood_value_balance_zat)
        } else {
            i128::from(self.fee_zat) - i128::from(self.transparent_value_balance_zat)
        };
        i64::try_from(balance).expect("verified transaction value balances fit in the money range")
    }

    /// Returns the observable shielded-flow classification.
    pub fn flow(self, location: TransactionLocation) -> Option<ExplorerShieldedFlow> {
        if self.kind(location) != ExplorerTransactionKind::Shielded {
            return None;
        }
        if self.transparent_input_count == 0 && self.transparent_output_count == 0 {
            return Some(ExplorerShieldedFlow::FullyShielded);
        }
        Some(match self.shielded_value_balance(location) {
            value if value < 0 => ExplorerShieldedFlow::Shield,
            value if value > 0 => ExplorerShieldedFlow::Deshield,
            _ => ExplorerShieldedFlow::Complex,
        })
    }

    /// Returns the absolute public flow used by amount filters.
    pub fn public_flow_amount(self, location: TransactionLocation) -> Option<u64> {
        matches!(
            self.flow(location),
            Some(ExplorerShieldedFlow::Shield | ExplorerShieldedFlow::Deshield)
        )
        .then_some(self.shielded_value_balance(location).unsigned_abs())
    }
}
