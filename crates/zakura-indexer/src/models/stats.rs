//! Persisted all-time aggregates for the canonical indexed chain.

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
}
