//! Compact authoritative facts stored for each canonical transaction.

use zakura_chain::block::Height;

/// A transaction's canonical position.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct TransactionPosition {
    pub(super) height: Height,
    pub(super) transaction_index: u32,
}

/// Transaction-specific facts used to derive explorer list fields and indexes.
///
/// Block hash and time remain authoritative in the block index. Classification
/// and amount buckets are derived from these facts, so they cannot drift from
/// the stored value balances and component counts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct TransactionRecord {
    pub(super) position: TransactionPosition,
    pub(super) serialized_size: u32,
    pub(super) fee_zat: u64,
    pub(super) transparent_value_balance_zat: i64,
    pub(super) sapling_value_balance_zat: i64,
    pub(super) orchard_value_balance_zat: i64,
    pub(super) ironwood_value_balance_zat: i64,
    pub(super) transparent_input_count: u32,
    pub(super) transparent_output_count: u32,
    pub(super) joinsplit_count: u32,
    pub(super) sapling_spend_count: u32,
    pub(super) sapling_output_count: u32,
    pub(super) orchard_action_count: u32,
    pub(super) ironwood_action_count: u32,
}
