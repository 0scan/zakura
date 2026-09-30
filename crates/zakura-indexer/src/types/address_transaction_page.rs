//! Cursor-paginated address transaction responses.

use schemars::JsonSchema;
use serde::Serialize;

use super::AddressTransactionListItem;

/// Pagination metadata for [`AddressTransactionsResponse`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct AddressTransactionsPagination {
    /// Maximum number of requested records.
    pub limit: u32,

    /// Whether an older matching transaction is available.
    pub has_next: bool,

    /// Whether a newer matching transaction is available.
    pub has_prev: bool,

    /// Opaque position to pass with `direction=next`.
    pub next_cursor: Option<String>,

    /// Opaque position to pass with `direction=prev`.
    pub prev_cursor: Option<String>,
}

/// A newest-first page of canonical transactions involving one address.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct AddressTransactionsResponse {
    /// Requested transparent address.
    pub address: String,

    /// Matching canonical transactions ordered newest first.
    pub transactions: Vec<AddressTransactionListItem>,

    /// Page metadata.
    pub pagination: AddressTransactionsPagination,
}
