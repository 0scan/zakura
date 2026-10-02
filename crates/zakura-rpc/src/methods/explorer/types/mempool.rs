//! Explorer-facing mempool list and pending transaction detail responses.

use schemars::JsonSchema;
use serde::Serialize;
use zakura_indexer::{
    BlockTransactionInput, BlockTransactionOutput, ShieldedFlow, ShieldedPool, TransactionData,
    TransactionDetails, TransactionKind, TransactionStatus, TransactionsPagination,
};

/// A compact pending transaction summary for mempool list views.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct MempoolTransactionListItem {
    /// Transaction identifier in display byte order.
    pub txid: String,
    /// Pending transaction status.
    pub status: TransactionStatus,
    /// Local Unix time when the transaction entered the mempool.
    pub first_seen: Option<String>,
    /// Best-chain height observed when the transaction entered the mempool.
    pub entry_height: Option<String>,
    /// Consensus-serialized transaction size in bytes.
    pub size: u32,
    /// Primary transaction kind.
    pub kind: TransactionKind,
    /// Shielded pool classification, when applicable.
    pub pool: Option<ShieldedPool>,
    /// Observable transparent/shielded boundary flow, when applicable.
    pub flow: Option<ShieldedFlow>,
    /// Observable transparent/shielded boundary-flow amount in zatoshis.
    pub flow_amount_zat: Option<String>,
    /// Exact transaction fee in zatoshis.
    pub fee: String,
    /// Number of transparent inputs.
    pub vin_count: u32,
    /// Number of transparent outputs.
    pub vout_count: u32,
    /// Sum of transparent input values in zatoshis.
    pub total_input: String,
    /// Sum of transparent output values in zatoshis.
    pub total_output: String,
    /// Transparent value balance in zatoshis.
    pub value_balance_transparent: String,
    /// Resolved public transparent inputs.
    pub inputs: Vec<BlockTransactionInput>,
    /// Public transparent outputs.
    pub outputs: Vec<BlockTransactionOutput>,
    /// Combined shielded value balance in zatoshis.
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
    /// Direct unconfirmed parent transaction IDs.
    pub depends: Vec<String>,
}

/// Transaction-kind counts for the complete live mempool snapshot.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, JsonSchema)]
pub struct MempoolTransactionSummary {
    /// Number of pending transactions.
    pub total: u64,
    /// Number of pending transactions containing a shielded component.
    pub shielded: u64,
    /// Number of pending transactions containing only transparent components.
    pub transparent: u64,
}

/// A cursor-paginated newest-first mempool snapshot.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct MempoolTransactionsResponse {
    /// Transaction-kind counts for the unfiltered mempool snapshot.
    pub summary: MempoolTransactionSummary,
    /// Matching pending transactions.
    pub transactions: Vec<MempoolTransactionListItem>,
    /// Best-effort pagination metadata for the volatile mempool snapshot.
    pub pagination: TransactionsPagination,
}

/// Mempool placement metadata for one pending transaction.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct MempoolTransactionMetadata {
    /// Local Unix time when the transaction entered the mempool.
    pub first_seen: Option<String>,
    /// Best-chain height observed when the transaction entered the mempool.
    pub entry_height: Option<String>,
    /// Direct unconfirmed parent transaction IDs.
    pub depends: Vec<String>,
}

/// Full explorer detail for a transaction currently in the mempool.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct PendingTransactionDetails {
    /// Block-independent transaction data.
    #[serde(flatten)]
    pub transaction: TransactionData,
    /// Pending transaction status.
    pub status: TransactionStatus,
    /// Pending transactions have no containing block height.
    pub block_height: Option<String>,
    /// Pending transactions have no containing block hash.
    pub block_hash: Option<String>,
    /// Pending transactions have no containing block timestamp.
    pub block_time: Option<String>,
    /// Pending transactions have no position inside a block.
    #[serde(rename = "tx_index")]
    pub transaction_index: Option<u32>,
    /// Pending transactions have zero confirmations.
    pub confirmations: u32,
    /// Primary transaction kind.
    pub kind: TransactionKind,
    /// Shielded pool classification, when applicable.
    pub pool: Option<ShieldedPool>,
    /// Observable transparent/shielded boundary flow, when applicable.
    pub flow: Option<ShieldedFlow>,
    /// Observable transparent/shielded boundary-flow amount in zatoshis.
    pub flow_amount_zat: Option<String>,
    /// Number of Sprout JoinSplits.
    pub joinsplit_count: u32,
    /// Coinbase data is always absent because coinbase transactions cannot enter the mempool.
    pub coinbase_hex: Option<String>,
    /// Current placement in this node's mempool.
    pub mempool: MempoolTransactionMetadata,
}

/// Detail response returned by the shared transaction-detail RPC.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum TransactionDetailsResponse {
    /// A transaction mined in the canonical chain.
    Mined(TransactionDetails),
    /// A transaction in the node's mempool.
    Pending(PendingTransactionDetails),
}
