//! Complete explorer response for one canonical transaction.

use schemars::JsonSchema;
use serde::Serialize;

use super::{BlockTransaction, ShieldedFlow, ShieldedPool, TransactionKind, TransactionStatus};

/// Transaction details assembled from canonical state and indexed explorer data.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct TransactionDetails {
    /// Raw transaction fields, public accounting data, and transparent inputs and outputs.
    #[serde(flatten)]
    pub transaction: BlockTransaction,

    /// Canonical confirmation status.
    pub status: TransactionStatus,

    /// Number of confirmations in the current best chain.
    pub confirmations: u32,

    /// Primary transaction kind.
    pub kind: TransactionKind,

    /// Shielded pool classification, when the transaction touches a shielded pool.
    pub pool: Option<ShieldedPool>,

    /// Observable transparent/shielded boundary flow, when applicable.
    pub flow: Option<ShieldedFlow>,

    /// Observable transparent/shielded boundary-flow amount in zatoshis.
    pub flow_amount_zat: Option<String>,

    /// Number of Sprout JoinSplits.
    pub joinsplit_count: u32,

    /// Complete coinbase input script as hexadecimal bytes, when this is a coinbase transaction.
    pub coinbase_hex: Option<String>,
}
