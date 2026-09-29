//! Persisted explorer-specific facts for one indexed block.

use serde::{Deserialize, Serialize};
use zakura_chain::block::Height;

use super::IntervalStatsRecord;

/// Block facts that are not read from the canonical state database at list-query time.
///
/// The block hash is the RocksDB key and is intentionally not duplicated in this value.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub(crate) struct IndexedBlockRecord {
    pub(crate) height: Height,
    pub(crate) timestamp: i64,
    pub(crate) transaction_count: u32,
    pub(crate) serialized_size: u32,
    pub(crate) difficulty: String,
    #[serde(with = "super::stats::decimal_u128")]
    pub(crate) accepted_work: u128,
    pub(crate) miner_address: Option<String>,
    pub(crate) total_fees_zat: u64,
    pub(crate) miner_pool: String,
    pub(crate) transparent_transaction_count: u32,
    pub(crate) shielded_transaction_count: u32,
    pub(crate) coinbase_transaction_count: u32,
    pub(crate) fully_shielded_transaction_count: u32,
    pub(crate) mixed_pool_transaction_count: u32,
    pub(crate) funded_transparent_address_count: u64,
    pub(crate) pool_transparent: u64,
    pub(crate) pool_sprout: u64,
    pub(crate) pool_sapling: u64,
    pub(crate) pool_orchard: u64,
    pub(crate) pool_deferred: u64,
    pub(crate) pool_ironwood: u64,
    pub(crate) pool_nsm: i64,
    pub(crate) total_issuance: u64,
    pub(crate) interval: IntervalStatsRecord,
}
