//! Explorer-facing transaction rows scoped to one transparent address.

use schemars::JsonSchema;
use serde::Serialize;

use super::{ShieldedFlow, ShieldedPool, TransactionKind};

/// One canonical transaction's public effect on a requested transparent address.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct AddressTransactionListItem {
    /// Transaction identifier in display byte order.
    pub txid: String,

    /// Containing block height.
    pub block_height: String,

    /// Containing canonical block hash.
    pub block_hash: String,

    /// Containing block timestamp as Unix seconds.
    pub block_time: String,

    /// Zero-based transaction position within the block.
    #[serde(rename = "tx_index")]
    pub transaction_index: u32,

    /// Consensus-serialized transaction size in bytes.
    pub size: u32,

    /// Primary transaction kind.
    pub kind: TransactionKind,

    /// Shielded pool classification, when the transaction touches a shielded pool.
    pub pool: Option<ShieldedPool>,

    /// Observable transparent/shielded boundary flow, when applicable.
    pub flow: Option<ShieldedFlow>,

    /// Whether the transaction contains Sprout data.
    pub has_sprout: bool,

    /// Whether the transaction contains Sapling data.
    pub has_sapling: bool,

    /// Whether the transaction contains Orchard data.
    pub has_orchard: bool,

    /// Whether the transaction contains Ironwood data.
    pub has_ironwood: bool,

    /// Value received by this address in zatoshis.
    pub received_zat: String,

    /// Value spent from this address in zatoshis.
    pub sent_zat: String,

    /// Signed balance change for this address in zatoshis.
    pub net_change_zat: String,

    /// Deterministic largest-value address on the opposite side of the transaction.
    pub primary_counterparty: Option<String>,

    /// Number of other recognizable transparent input addresses.
    pub sender_count: u32,

    /// Number of other recognizable transparent output addresses.
    pub recipient_count: u32,
}
