//! Cursor-paginated transparent-address UTXO responses.

use schemars::JsonSchema;
use serde::Serialize;

/// One transparent output that is unspent in the current best chain.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct AddressUtxoSummary {
    /// Transaction that created the output.
    pub txid: String,

    /// Output index within the transaction.
    pub output_index: u32,

    /// Height of the block that created the output.
    pub block_height: String,

    /// Transaction index within the block.
    pub tx_index: u32,

    /// Exact output value in zatoshis.
    pub value_zat: String,

    /// Hex-encoded transparent locking script, without a length prefix.
    pub script_hex: String,

    /// Whether the creating transaction is the block's coinbase transaction.
    pub coinbase: bool,

    /// Canonical block hash at `block_height`.
    pub block_hash: String,

    /// Creating block's Unix timestamp.
    pub block_time: String,

    /// Whether the creating block is at or below this node's finalized tip.
    pub finalized: bool,
}

/// Pagination metadata for [`AddressUtxosResponse`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct AddressUtxosPagination {
    /// Maximum number of requested records.
    pub limit: u32,

    /// Whether an older unspent output is available.
    pub has_next: bool,

    /// Whether a newer unspent output is available.
    pub has_prev: bool,

    /// Opaque position to pass as `cursor` with `direction=next`.
    pub next_cursor: Option<String>,

    /// Opaque position to pass as `cursor` with `direction=prev`.
    pub prev_cursor: Option<String>,
}

/// A newest-first page of current transparent UTXOs for one address.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct AddressUtxosResponse {
    /// Requested transparent address.
    pub address: String,

    /// Current unspent outputs, newest first.
    pub utxos: Vec<AddressUtxoSummary>,

    /// Page metadata.
    pub pagination: AddressUtxosPagination,
}
