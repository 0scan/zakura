//! Explorer-facing transaction summaries embedded in block details.

use schemars::JsonSchema;
use serde::Serialize;

/// A transaction with the public accounting data needed by a block explorer.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct BlockTransaction {
    /// Transaction identifier in display byte order.
    pub txid: String,

    /// Consensus-serialized transaction bytes.
    pub hex: String,

    /// Containing block height.
    pub block_height: String,

    /// Containing block hash.
    pub block_hash: String,

    /// Containing block timestamp as Unix seconds.
    pub block_time: String,

    /// Consensus-serialized transaction size in bytes.
    pub size: u32,

    /// Transaction version.
    pub version: u32,

    /// Version group ID when present.
    pub version_group_id: Option<String>,

    /// Raw transaction lock time.
    pub lock_time: String,

    /// Expiry height when present.
    pub expiry_height: Option<String>,

    /// ZIP-244 authorizing data digest for version 5 and later transactions.
    pub auth_digest: Option<String>,

    /// Whether the transaction uses an Overwinter or later format.
    pub overwintered: bool,

    /// Number of transparent inputs.
    pub vin_count: u32,

    /// Number of transparent outputs.
    pub vout_count: u32,

    /// Combined Sapling, Orchard, and Ironwood value balance in zatoshis.
    pub value_balance: String,

    /// Sapling value balance in zatoshis.
    pub value_balance_sapling: String,

    /// Orchard value balance in zatoshis.
    pub value_balance_orchard: String,

    /// Ironwood value balance in zatoshis.
    pub value_balance_ironwood: String,

    /// Whether the transaction contains Sapling data.
    pub has_sapling: bool,

    /// Whether the transaction contains Orchard data.
    pub has_orchard: bool,

    /// Whether the transaction contains Ironwood data.
    pub has_ironwood: bool,

    /// Whether the transaction contains Sprout JoinSplits.
    pub has_sprout: bool,

    /// Number of Sapling spends.
    pub sapling_spend_count: u32,

    /// Number of Sapling outputs.
    pub sapling_output_count: u32,

    /// Number of Orchard actions.
    pub orchard_actions: u32,

    /// Number of Ironwood actions.
    pub ironwood_actions: u32,

    /// Exact transaction fee in zatoshis.
    pub fee: String,

    /// Sum of transparent input values in zatoshis.
    pub total_input: String,

    /// Sum of transparent output values in zatoshis.
    pub total_output: String,

    /// Whether this is the block's coinbase transaction.
    pub is_coinbase: bool,

    /// Zero-based transaction position within the block.
    #[serde(rename = "tx_index")]
    pub transaction_index: u32,

    /// Public transparent inputs. Coinbase input data is exposed at block level.
    pub inputs: Vec<BlockTransactionInput>,

    /// Public transparent outputs.
    pub outputs: Vec<BlockTransactionOutput>,
}

/// One public transparent transaction input.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct BlockTransactionInput {
    /// Transaction containing the spent output.
    #[serde(rename = "prev_txid")]
    pub previous_transaction_id: String,

    /// Index of the spent output.
    #[serde(rename = "prev_vout")]
    pub previous_output_index: u32,

    /// Transparent address of the spent output, when recognizable.
    pub address: Option<String>,

    /// Exact value of the spent output in zatoshis.
    pub value: String,

    /// Unlocking script bytes.
    pub script_sig: String,

    /// Input sequence number.
    pub sequence: u32,
}

/// One public transparent transaction output.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct BlockTransactionOutput {
    /// Transaction containing this output.
    #[serde(rename = "txid")]
    pub transaction_id: String,

    /// Transparent address, when recognizable.
    pub address: Option<String>,

    /// Exact output value in zatoshis.
    pub value: String,

    /// Zero-based output position within the transaction.
    #[serde(rename = "vout_index")]
    pub output_index: u32,

    /// Locking script bytes.
    #[serde(rename = "script_pubkey")]
    pub script_pub_key: String,

    /// Whether this output is spent in the current best chain.
    pub spent: bool,
}
