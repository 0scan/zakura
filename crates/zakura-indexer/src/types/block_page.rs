//! Paginated block query responses.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::BlockRecord;

/// Pagination metadata for [`BlocksResponse`].
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BlocksPagination {
    /// Maximum number of requested records.
    pub limit: u32,

    /// Number of indexed canonical blocks, encoded as a decimal string.
    pub total: String,

    /// Whether an older page is available.
    pub has_more: bool,

    /// Opaque position to pass as `cursor` when requesting the next page.
    pub next_cursor: Option<String>,
}

/// A newest-first page of indexed canonical blocks.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize, JsonSchema)]
pub struct BlocksResponse {
    /// Canonical blocks ordered from newest to oldest.
    pub blocks: Vec<BlockRecord>,

    /// Page metadata.
    pub pagination: BlocksPagination,
}
