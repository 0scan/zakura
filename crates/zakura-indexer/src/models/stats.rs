//! Persisted canonical-chain aggregates used by explorer statistics and charts.

use serde::{Deserialize, Serialize};

/// Canonical-chain totals updated atomically with block indexing and rollback.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub(crate) struct ChainStatsRecord {
    pub(crate) block_count: u64,
    pub(crate) transaction_count: u64,
    pub(crate) block_bytes: u64,
    pub(crate) total_fees_zat: u64,
    pub(crate) transparent_transaction_count: u64,
    pub(crate) shielded_transaction_count: u64,
    pub(crate) coinbase_transaction_count: u64,
    pub(crate) fully_shielded_transaction_count: u64,
    pub(crate) mixed_pool_transaction_count: u64,
    pub(crate) funded_transparent_address_count: u64,
}

/// Additive metrics for one UTC chart interval.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub(crate) struct IntervalStatsRecord {
    pub(crate) block_count: u64,
    pub(crate) transaction_count: u64,
    pub(crate) empty_block_count: u64,
    #[serde(with = "decimal_u128")]
    pub(crate) accepted_work: u128,
    #[serde(with = "decimal_u128")]
    pub(crate) total_fees_zat: u128,
    pub(crate) total_block_size_bytes: u64,
    #[serde(with = "decimal_u128")]
    pub(crate) total_subsidy_zat: u128,
    #[serde(with = "decimal_u128")]
    pub(crate) miner_subsidy_zat: u128,
    #[serde(with = "decimal_u128")]
    pub(crate) founders_reward_zat: u128,
    #[serde(with = "decimal_u128")]
    pub(crate) funding_streams_zat: u128,
    #[serde(with = "decimal_u128")]
    pub(crate) deferred_subsidy_zat: u128,
    #[serde(with = "decimal_u128")]
    pub(crate) lockbox_disbursement_zat: u128,
    #[serde(with = "decimal_u128")]
    pub(crate) coinbase_output_transparent_zat: u128,
    #[serde(with = "decimal_u128")]
    pub(crate) coinbase_output_sapling_zat: u128,
    #[serde(with = "decimal_u128")]
    pub(crate) coinbase_output_orchard_zat: u128,
    #[serde(with = "decimal_u128")]
    pub(crate) coinbase_output_ironwood_zat: u128,
    #[serde(with = "decimal_u128")]
    pub(crate) coinbase_unclaimed_zat: u128,
    pub(crate) transparent_tx_count: u64,
    pub(crate) transparent_coinbase_tx_count: u64,
    pub(crate) shielded_coinbase_migration_tx_count: u64,
    pub(crate) sprout_tx_count: u64,
    pub(crate) sapling_tx_count: u64,
    pub(crate) orchard_tx_count: u64,
    pub(crate) ironwood_tx_count: u64,
    pub(crate) transparent_transaction_count: u64,
    pub(crate) shielded_transaction_count: u64,
    pub(crate) coinbase_transaction_count: u64,
    pub(crate) fully_shielded_transaction_count: u64,
    pub(crate) mixed_pool_transaction_count: u64,
    pub(crate) sapling_spend_count: u64,
    pub(crate) sapling_output_count: u64,
    #[serde(with = "decimal_u128")]
    pub(crate) transparent_inflow: u128,
    #[serde(with = "decimal_u128")]
    pub(crate) transparent_outflow: u128,
    #[serde(with = "decimal_u128")]
    pub(crate) sprout_inflow: u128,
    #[serde(with = "decimal_u128")]
    pub(crate) sprout_outflow: u128,
    pub(crate) sprout_inflow_transaction_count: u64,
    pub(crate) sprout_outflow_transaction_count: u64,
    #[serde(with = "decimal_u128")]
    pub(crate) sapling_inflow: u128,
    #[serde(with = "decimal_u128")]
    pub(crate) sapling_outflow: u128,
    pub(crate) sapling_inflow_transaction_count: u64,
    pub(crate) sapling_outflow_transaction_count: u64,
    #[serde(with = "decimal_u128")]
    pub(crate) orchard_inflow: u128,
    #[serde(with = "decimal_u128")]
    pub(crate) orchard_outflow: u128,
    pub(crate) orchard_inflow_transaction_count: u64,
    pub(crate) orchard_outflow_transaction_count: u64,
    #[serde(with = "decimal_u128")]
    pub(crate) ironwood_inflow: u128,
    #[serde(with = "decimal_u128")]
    pub(crate) ironwood_outflow: u128,
    pub(crate) ironwood_inflow_transaction_count: u64,
    pub(crate) ironwood_outflow_transaction_count: u64,
    pub(crate) v6_transaction_count: u64,
    pub(crate) ironwood_bundle_transaction_count: u64,
    pub(crate) orchard_bundle_transaction_count: u64,
    pub(crate) orchard_ironwood_transaction_count: u64,
    pub(crate) orchard_action_count: u64,
    pub(crate) ironwood_action_count: u64,
    pub(crate) ironwood_active_block_count: u64,
    pub(crate) observable_orchard_to_ironwood_transaction_count: u64,
    #[serde(with = "decimal_u128")]
    pub(crate) observable_orchard_to_ironwood_value_zat: u128,
    pub(crate) zip318_action_shape_transaction_count: u64,
    pub(crate) zip318_denomination_transaction_count: u64,
    pub(crate) zip318_fee_transaction_count: u64,
    pub(crate) zip318_schedule_transaction_count: u64,
    pub(crate) ironwood_canonical_denomination_counts: [u64; 19],
}

/// One UTC day's reversible snapshot and interval facts.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub(crate) struct DailyStatsRecord {
    pub(crate) start_height: u32,
    pub(crate) end_height: u32,
    pub(crate) end_block_hash: String,
    pub(crate) interval_anchor_timestamp: i64,
    pub(crate) block_time_interval_count: u64,
    pub(crate) block_timestamp: i64,
    pub(crate) min_header_timestamp: i64,
    pub(crate) max_header_timestamp: i64,
    pub(crate) difficulty: String,
    pub(crate) funded_transparent_address_count: u64,
    pub(crate) pool_transparent: u64,
    pub(crate) pool_sprout: u64,
    pub(crate) pool_sapling: u64,
    pub(crate) pool_orchard: u64,
    pub(crate) pool_deferred: u64,
    pub(crate) pool_ironwood: u64,
    pub(crate) total_issuance: u64,
    pub(crate) end_block_subsidy_zat: u64,
    pub(crate) interval: IntervalStatsRecord,
}

pub(crate) mod decimal_u128 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub(crate) fn serialize<S>(value: &u128, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&value.to_string())
    }

    pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<u128, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::{DailyStatsRecord, IntervalStatsRecord};

    #[test]
    fn daily_stats_round_trip_exact_u128_values_as_decimal_strings() {
        let record = DailyStatsRecord {
            interval: IntervalStatsRecord {
                accepted_work: u128::MAX,
                observable_orchard_to_ironwood_value_zat: u128::MAX - 1,
                ..Default::default()
            },
            ..Default::default()
        };
        let json = serde_json::to_string(&record).expect("daily stats should serialize");
        assert!(json.contains(&format!("\"accepted_work\":\"{}\"", u128::MAX)));
        assert_eq!(
            serde_json::from_str::<DailyStatsRecord>(&json)
                .expect("daily stats should deserialize"),
            record
        );
    }
}
