//! Explorer-facing transparent address summary types.

use schemars::JsonSchema;
use serde::Serialize;

/// Canonical transaction and block context for address activity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct AddressActivity {
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
}

/// Earliest indexed public funding event for a transparent address.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct AddressFirstFunding {
    /// Canonical transaction and block context.
    #[serde(flatten)]
    pub activity: AddressActivity,

    /// Total value received by the address in this transaction, in zatoshis.
    pub amount_zat: String,

    /// Largest recognizable transparent input address other than the recipient.
    pub funder_address: Option<String>,

    /// Whether the funding transaction creates a block reward.
    pub is_coinbase: bool,
}

/// General indexed information for one transparent address.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct AddressSummary {
    /// Transparent address in its network encoding.
    pub address: String,

    /// Current indexed transparent balance in zatoshis.
    pub balance_zat: String,

    /// Total indexed value received, including change, in zatoshis.
    pub total_received_zat: String,

    /// Total indexed value spent in zatoshis.
    pub total_sent_zat: String,

    /// Number of canonical transactions that sent from or received to the address.
    pub transaction_count: String,

    /// Earliest canonical transaction involving the address.
    pub first_seen: Option<AddressActivity>,

    /// Most recent canonical transaction involving the address.
    pub last_seen: Option<AddressActivity>,

    /// Earliest canonical transaction that paid the address.
    pub first_funding: Option<AddressFirstFunding>,

    /// Tip height covered by this summary.
    pub indexed_height: Option<String>,

    /// Tip block hash covered by this summary.
    pub indexed_block_hash: Option<String>,
}
