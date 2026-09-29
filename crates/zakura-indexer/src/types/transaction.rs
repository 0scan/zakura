//! Explorer-facing transaction list items.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The primary public-value domain used by a transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TransactionKind {
    /// A transaction containing at least one shielded component.
    Shielded,
    /// A non-coinbase transaction containing only transparent components.
    Transparent,
    /// The first transaction in a block, which creates the block reward.
    Coinbase,
}

/// The observable direction of value crossing the transparent/shielded boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ShieldedFlow {
    /// Public value enters one or more shielded pools.
    Shield,
    /// Shielded value leaves one or more shielded pools.
    Deshield,
    /// The transaction has shielded components and no transparent inputs or outputs.
    FullyShielded,
    /// Shielded and transparent components are present, but there is no net boundary flow.
    Complex,
}

/// The shielded pool or set of pools touched by a transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ShieldedPool {
    /// The legacy Sprout pool.
    Sprout,
    /// The Sapling pool.
    Sapling,
    /// The Orchard pool.
    Orchard,
    /// The Ironwood pool.
    Ironwood,
    /// More than one shielded pool.
    Mixed,
}

/// A compact canonical transaction summary for explorer list views.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct TransactionListItem {
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

    /// Public boundary-flow amount in zatoshis, or `None` when the amount is private.
    pub amount_zat: Option<String>,

    /// Exact transaction fee in zatoshis.
    pub fee: String,

    /// Number of non-coinbase transparent inputs.
    pub vin_count: u32,

    /// Number of transparent outputs.
    pub vout_count: u32,

    /// Combined shielded transaction value balance in zatoshis.
    pub shielded_value_balance: String,

    /// Sapling value balance in zatoshis.
    pub value_balance_sapling: String,

    /// Orchard value balance in zatoshis.
    pub value_balance_orchard: String,

    /// Ironwood value balance in zatoshis.
    pub value_balance_ironwood: String,

    /// Number of Sprout JoinSplits.
    pub joinsplit_count: u32,

    /// Number of Sapling spends.
    pub sapling_spend_count: u32,

    /// Number of Sapling outputs.
    pub sapling_output_count: u32,

    /// Number of Orchard actions.
    pub orchard_actions: u32,

    /// Number of Ironwood actions.
    pub ironwood_actions: u32,
}
