//! Compact authoritative facts stored for each canonical transaction.

use serde::{Deserialize, Serialize};
use zakura_chain::block::Height;

/// A transaction's canonical position.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub(crate) struct TransactionPosition {
    pub(crate) height: Height,
    pub(crate) transaction_index: u32,
}

/// Transaction facts used to derive explorer list fields and filter indexes.
///
/// Block hash and time remain authoritative in the block index. Classification
/// and amount buckets are derived from these facts, so they cannot drift from
/// the stored value balances and component counts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TransactionRecord {
    pub(crate) position: TransactionPosition,
    pub(crate) serialized_size: u32,
    pub(crate) fee_zat: u64,
    pub(crate) transparent_value_balance_zat: i64,
    pub(crate) sapling_value_balance_zat: i64,
    pub(crate) orchard_value_balance_zat: i64,
    pub(crate) ironwood_value_balance_zat: i64,
    pub(crate) transparent_input_count: u32,
    pub(crate) transparent_output_count: u32,
    pub(crate) joinsplit_count: u32,
    pub(crate) sapling_spend_count: u32,
    pub(crate) sapling_output_count: u32,
    pub(crate) orchard_action_count: u32,
    pub(crate) ironwood_action_count: u32,
}
