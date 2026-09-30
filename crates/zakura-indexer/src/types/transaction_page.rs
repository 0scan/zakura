//! Cursor-paginated transaction list responses.

use schemars::JsonSchema;
use serde::Serialize;

use super::TransactionListItem;

/// Pagination metadata for [`TransactionsResponse`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct TransactionsPagination {
    /// Maximum number of requested records.
    pub limit: u32,

    /// Whether an older matching transaction is available.
    pub has_next: bool,

    /// Whether a newer matching transaction is available.
    pub has_prev: bool,

    /// Opaque position to pass as `cursor` with `direction=next`.
    pub next_cursor: Option<String>,

    /// Opaque position to pass as `cursor` with `direction=prev`.
    pub prev_cursor: Option<String>,
}

/// A newest-first page of canonical transactions.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct TransactionsResponse {
    /// Matching canonical transactions ordered newest first.
    pub transactions: Vec<TransactionListItem>,

    /// Page metadata.
    pub pagination: TransactionsPagination,
}
