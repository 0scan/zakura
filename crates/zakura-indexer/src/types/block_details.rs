//! Complete explorer response for one canonical block.

use schemars::JsonSchema;
use serde::Serialize;

use super::{BlockRecord, BlockTransaction};

/// Block details assembled from canonical state and indexed explorer data.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct BlockDetails {
    /// Fields shared with the recent-block list.
    #[serde(flatten)]
    pub summary: BlockRecord,

    /// Number of confirmations in the current best chain.
    pub confirmations: u32,

    /// Whether this hash is on the current best chain.
    pub canonical: bool,

    /// Whether this block is at or below the durable finalized tip.
    pub finalized: bool,

    /// Whether this response describes an archived orphaned block.
    pub is_orphaned: bool,

    /// Consensus block version.
    pub version: u32,

    /// Transaction Merkle root in display byte order.
    pub merkle_root: String,

    /// Height-dependent block commitment in display byte order.
    pub block_commitments: String,

    /// Sapling note commitment tree root after this block.
    pub final_sapling_root: String,

    /// Orchard note commitment tree root after this block.
    pub final_orchard_root: Option<String>,

    /// Ironwood note commitment tree root after this block.
    pub final_ironwood_root: Option<String>,

    /// Compact difficulty target.
    pub bits: String,

    /// Mining nonce in RPC display byte order.
    pub nonce: String,

    /// Equihash solution.
    pub solution: String,

    /// Previous canonical block hash.
    pub previous_block_hash: String,

    /// Next canonical block hash, if one currently exists.
    pub next_block_hash: Option<String>,

    /// Total chain supply after this block.
    pub chain_supply: Option<ValuePoolBalance>,

    /// Chain value-pool balances and per-block deltas.
    pub value_pools: Vec<ValuePoolBalance>,

    /// Note commitment tree sizes after this block.
    pub trees: BlockTrees,

    /// Complete coinbase input script as raw hexadecimal bytes.
    pub coinbase_hex: Option<String>,

    /// Transactions in their canonical block order.
    pub transactions: Vec<BlockTransaction>,
}

/// Note commitment tree sizes after a block.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct BlockTrees {
    /// Sapling note commitment tree information.
    pub sapling: TreeSize,

    /// Orchard note commitment tree information.
    pub orchard: TreeSize,

    /// Ironwood note commitment tree information after NU6.3 activation.
    pub ironwood: Option<TreeSize>,
}

/// The number of notes in a note commitment tree.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct TreeSize {
    /// Number of notes in the tree.
    pub size: u64,
}

/// One chain value pool balance, compatible with `getblock` accounting data.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct ValuePoolBalance {
    /// Pool name, omitted for the aggregate chain supply.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    /// Pool balance in ZEC.
    pub chain_value: f64,

    /// Exact pool balance in zatoshis.
    pub chain_value_zat: String,

    /// Whether this pool currently has a non-zero balance.
    pub monitored: bool,

    /// Change produced by this block in ZEC.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_delta: Option<f64>,

    /// Exact change produced by this block in zatoshis.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_delta_zat: Option<String>,
}
