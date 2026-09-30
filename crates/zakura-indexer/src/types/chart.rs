//! Explorer chart snapshots and date-range pagination.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Inclusive UTC date range for chart snapshots.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct ChartDataRequest {
    /// First date to return, formatted as `YYYY-MM-DD`.
    pub start_date: Option<String>,
    /// Last date to return, formatted as `YYYY-MM-DD`.
    pub end_date: Option<String>,
    /// Maximum number of entries to return.
    pub limit: Option<usize>,
}

/// One daily snapshot, with interval fields measured since the previous snapshot.
#[allow(missing_docs)]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct ChartDataEntry {
    pub date_key: String,
    pub height: u32,
    pub funded_transparent_address_count: u64,
    pub pool_transparent: String,
    pub pool_sprout: String,
    pub pool_sapling: String,
    pub pool_orchard: String,
    pub pool_deferred: String,
    pub pool_ironwood: String,
    pub difficulty: String,
    pub total_issuance: String,
    pub inflation_rate_percent: String,
    pub block_timestamp: i64,
    pub transparent_tx_count: u32,
    pub transparent_coinbase_tx_count: u32,
    pub shielded_coinbase_migration_tx_count: u32,
    pub sprout_tx_count: u32,
    pub sapling_tx_count: u32,
    pub orchard_tx_count: u32,
    pub ironwood_tx_count: u32,
    pub interval_transparent_transaction_count: u64,
    pub interval_shielded_transaction_count: u64,
    pub interval_coinbase_transaction_count: u64,
    pub interval_fully_shielded_transaction_count: u64,
    pub interval_mixed_pool_transaction_count: u64,
    pub interval_sapling_spend_count: u64,
    pub interval_sapling_output_count: u64,
    pub transparent_inflow: String,
    pub transparent_outflow: String,
    pub sprout_inflow: String,
    pub sprout_outflow: String,
    pub sprout_inflow_transaction_count: u64,
    pub sprout_outflow_transaction_count: u64,
    pub sapling_inflow: String,
    pub sapling_outflow: String,
    pub sapling_inflow_transaction_count: u64,
    pub sapling_outflow_transaction_count: u64,
    pub orchard_inflow: String,
    pub orchard_outflow: String,
    pub orchard_inflow_transaction_count: u64,
    pub orchard_outflow_transaction_count: u64,
    pub ironwood_inflow: String,
    pub ironwood_outflow: String,
    pub ironwood_inflow_transaction_count: u64,
    pub ironwood_outflow_transaction_count: u64,
    pub average_block_time: String,
    pub average_block_fee_zat: String,
    pub average_block_size: u32,
    pub interval_block_count: u64,
    pub interval_transaction_count: u64,
    pub interval_empty_block_count: u64,
    pub interval_min_header_timestamp: i64,
    pub interval_max_header_timestamp: i64,
    pub interval_elapsed_header_time_seconds: u64,
    pub interval_accepted_work: String,
    pub interval_total_fees_zat: String,
    pub interval_total_block_size_bytes: u64,
    pub interval_total_subsidy_zat: String,
    pub interval_miner_subsidy_zat: String,
    pub interval_founders_reward_zat: String,
    pub interval_funding_streams_zat: String,
    pub interval_deferred_subsidy_zat: String,
    pub interval_lockbox_disbursement_zat: String,
    pub coinbase_output_transparent_zat: String,
    pub coinbase_output_sapling_zat: String,
    pub coinbase_output_orchard_zat: String,
    pub coinbase_output_ironwood_zat: String,
    pub coinbase_unclaimed_zat: String,
    pub interval_v6_transaction_count: u64,
    pub interval_ironwood_bundle_transaction_count: u64,
    pub interval_orchard_bundle_transaction_count: u64,
    pub interval_orchard_ironwood_transaction_count: u64,
    pub interval_orchard_action_count: u64,
    pub interval_ironwood_action_count: u64,
    pub interval_ironwood_active_block_count: u64,
    pub interval_observable_orchard_to_ironwood_transaction_count: u64,
    pub interval_observable_orchard_to_ironwood_value_zat: String,
    pub interval_zip318_action_shape_transaction_count: u64,
    pub interval_zip318_denomination_transaction_count: u64,
    pub interval_zip318_fee_transaction_count: u64,
    pub interval_zip318_schedule_transaction_count: u64,
    pub interval_ironwood_canonical_denomination_counts: Vec<u64>,
}

/// A page of chart snapshots sorted from oldest to newest.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct ChartDataResponse {
    /// Daily entries in ascending date order.
    pub entries: Vec<ChartDataEntry>,
    /// Inclusive start date for the next page.
    pub next_start_date: Option<String>,
}
