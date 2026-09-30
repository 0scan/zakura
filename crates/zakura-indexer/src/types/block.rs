//! Explorer-facing block records.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Explorer-facing summary of one canonical block.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize, JsonSchema)]
pub struct BlockRecord {
    /// Block height, encoded as a decimal string for explorer compatibility.
    pub height: String,

    /// Block hash in display byte order.
    pub hash: String,

    /// Block timestamp as Unix seconds, encoded as a decimal string.
    pub timestamp: String,

    /// Number of transactions in the block.
    pub transaction_count: u32,

    /// Consensus-serialized block size in bytes.
    pub size: u32,

    /// Work difficulty relative to the network minimum, with six decimal places.
    pub difficulty: String,

    /// Best-effort transparent miner payout address.
    pub miner_address: Option<String>,

    /// Sum of non-coinbase transaction fees in zatoshis.
    pub total_fees: String,

    /// Best-effort mining pool attribution, or `Unknown`.
    pub miner_pool: String,
}
