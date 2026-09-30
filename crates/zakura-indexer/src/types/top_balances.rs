//! Explorer-facing transparent top-balances contracts.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Cursor-paginated top-balances request.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct TopBalancesRequest {
    /// Maximum number of ranked addresses to return.
    pub limit: Option<u32>,

    /// Opaque cursor returned by the previous page.
    pub cursor: Option<String>,
}

/// One funded transparent address ordered by current indexed balance.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct TopBalanceEntry {
    /// One-based position in the current ranking.
    pub rank: u64,

    /// Network-encoded transparent address.
    pub address: String,

    /// Current indexed balance in zatoshis.
    pub balance_zat: String,
}

/// Aggregate values displayed above the top-balances table.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct TopBalancesSummary {
    /// Number of transparent addresses with a non-zero balance.
    pub funded_transparent_address_count: u64,

    /// Total value held in the transparent pool, in zatoshis.
    pub transparent_supply_zat: String,

    /// Combined balance of the ten richest transparent addresses.
    pub top_10_balance_zat: String,

    /// Top-ten share of transparent supply, formatted as a percentage.
    pub top_10_concentration_percent: String,

    /// Combined balance of the hundred richest transparent addresses.
    pub top_100_balance_zat: String,

    /// Top-hundred share of transparent supply, formatted as a percentage.
    pub top_100_concentration_percent: String,
}

/// Cursor metadata for a top-balances page.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct TopBalancesPagination {
    /// Maximum number of entries requested.
    pub limit: u32,

    /// Total number of funded transparent addresses.
    pub total: String,

    /// Whether another page exists in this ranking generation.
    pub has_next: bool,

    /// Opaque cursor for the next page.
    pub next_cursor: Option<String>,
}

/// A stable page from the transparent-address balance ranking.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct TopBalancesResponse {
    /// Ranked funded transparent addresses.
    pub entries: Vec<TopBalanceEntry>,

    /// Top-balances summary derived from the same indexed chain generation.
    pub summary: TopBalancesSummary,

    /// Page navigation metadata.
    pub pagination: TopBalancesPagination,

    /// Indexed chain height represented by this ranking.
    pub indexed_height: Option<String>,

    /// Indexed chain hash represented by this ranking.
    pub indexed_block_hash: Option<String>,
}
