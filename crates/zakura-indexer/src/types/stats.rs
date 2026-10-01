//! Explorer-facing canonical-chain statistics.

use schemars::JsonSchema;
use serde::Serialize;

/// All-time aggregates for the canonical portion covered by the indexer.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct ChainTotals {
    /// Number of indexed canonical blocks.
    pub block_count: String,
    /// Number of transactions, including coinbase transactions.
    pub transaction_count: String,
    /// Sum of consensus-serialized canonical block sizes.
    pub block_bytes: String,
    /// Sum of non-coinbase transaction fees in zatoshis.
    pub total_fees_zat: String,
    /// Number of non-coinbase transactions with no shielded components.
    pub transparent_transaction_count: String,
    /// Number of non-coinbase transactions containing a shielded component.
    pub shielded_transaction_count: String,
    /// Number of coinbase transactions.
    pub coinbase_transaction_count: String,
    /// Number of shielded transactions with no transparent inputs or outputs.
    pub fully_shielded_transaction_count: String,
    /// Number of transactions touching more than one shielded pool.
    pub mixed_pool_transaction_count: String,
}

/// Activity in the trailing 24 hours ending at the indexed tip timestamp.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct RollingDayStats {
    /// False when the safety scan limit was reached before covering 24 hours.
    pub complete: bool,
    /// Inclusive lower timestamp bound in Unix seconds.
    pub window_start: Option<String>,
    /// Indexed-tip timestamp ending the window, in Unix seconds.
    pub window_end: Option<String>,
    /// Number of canonical blocks in the window.
    pub block_count: String,
    /// Number of transactions in the window, including coinbase transactions.
    pub transaction_count: String,
    /// Number of non-coinbase transactions with no shielded components.
    pub transparent_transaction_count: String,
    /// Number of non-coinbase transactions containing a shielded component.
    pub shielded_transaction_count: String,
    /// Number of coinbase transactions.
    pub coinbase_transaction_count: String,
    /// Sum of consensus-serialized block sizes in the window.
    pub block_bytes: String,
    /// Sum of non-coinbase transaction fees in the window.
    pub total_fees_zat: String,
    /// Sum of consensus block subsidies scheduled in the window.
    pub scheduled_subsidy_zat: String,
    /// Sum of allowed coinbase value not claimed by block producers in the window.
    pub coinbase_unclaimed_zat: String,
    /// Signed change in consensus `IssuedSupply` across the window.
    ///
    /// This is absent when the bounded scan did not reach the block immediately
    /// before the window, so an exact supply delta cannot be calculated.
    pub issued_supply_change_zat: Option<String>,
    /// Mean time between sampled blocks, formatted with one decimal place.
    pub average_block_time_seconds: Option<String>,
    /// Mean consensus-serialized block size, formatted with one decimal place.
    pub average_block_size_bytes: Option<String>,
    /// Mean transactions per block, formatted with one decimal place.
    pub average_transactions_per_block: Option<String>,
}

/// Explorer statistics at one canonical finalized tip.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct IndexerStats {
    /// Highest indexed canonical block height.
    pub indexed_height: Option<String>,
    /// Hash at `indexed_height`.
    pub indexed_block_hash: Option<String>,
    /// All-time canonical aggregates covered by the indexer.
    pub totals: ChainTotals,
    /// Trailing activity ending at the indexed tip timestamp.
    pub trailing_24h: RollingDayStats,
}

#[cfg(test)]
mod tests {
    use super::RollingDayStats;

    #[test]
    fn rolling_day_stats_serializes_issuance_accounting() {
        let stats = RollingDayStats {
            complete: true,
            window_start: Some("100".to_string()),
            window_end: Some("200".to_string()),
            block_count: "2".to_string(),
            transaction_count: "3".to_string(),
            transparent_transaction_count: "1".to_string(),
            shielded_transaction_count: "0".to_string(),
            coinbase_transaction_count: "2".to_string(),
            block_bytes: "400".to_string(),
            total_fees_zat: "5".to_string(),
            scheduled_subsidy_zat: "50".to_string(),
            coinbase_unclaimed_zat: "10".to_string(),
            issued_supply_change_zat: Some("40".to_string()),
            average_block_time_seconds: Some("100.0".to_string()),
            average_block_size_bytes: Some("200.0".to_string()),
            average_transactions_per_block: Some("1.5".to_string()),
        };

        let json = serde_json::to_value(stats).expect("rolling stats serialize to JSON");
        assert_eq!(json["scheduled_subsidy_zat"], "50");
        assert_eq!(json["coinbase_unclaimed_zat"], "10");
        assert_eq!(json["issued_supply_change_zat"], "40");
    }
}
