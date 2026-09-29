//! Request types for explorer index JSON-RPC methods.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use zakura_indexer::{
    AmountFilter, PageDirection, ShieldedFlowFilter, ShieldedPoolFilter, TransactionKindFilter,
    TransactionQuery,
};

/// Cursor pagination parameters for `getblocks`.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct GetBlocksRequest {
    /// Maximum records to return. The indexer clamps this value to 1–100.
    pub limit: Option<u32>,

    /// Opaque cursor returned by an earlier `getblocks` response.
    pub cursor: Option<String>,

    /// Direction to move relative to `cursor`.
    pub direction: PageDirection,
}

/// Cursor pagination and filter parameters for `gettransactions`.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct GetTransactionsRequest {
    /// Maximum records to return. The indexer clamps this value to 1–100.
    pub limit: Option<u32>,

    /// Opaque cursor returned by an earlier `gettransactions` response.
    pub cursor: Option<String>,

    /// Direction to move relative to `cursor`.
    pub direction: PageDirection,

    /// Top-level transaction kind filter.
    pub kind: TransactionKindFilter,

    /// Observable transparent/shielded boundary-flow filter.
    pub flow: ShieldedFlowFilter,

    /// Shielded pool filter.
    pub pool: ShieldedPoolFilter,

    /// Minimum public flow amount in ZEC: 0, 10, 100, or 1000.
    pub min_zec: u32,
}

impl GetTransactionsRequest {
    /// Converts transport parameters into the indexer's validated query type.
    pub(crate) fn transaction_query(&self) -> Result<TransactionQuery, String> {
        TransactionQuery::new(
            self.kind,
            self.flow,
            self.pool,
            AmountFilter::from_minimum_zec(self.min_zec)?,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transaction_request_uses_snake_case_filters_and_short_page_directions() {
        let request: GetTransactionsRequest = serde_json::from_value(serde_json::json!({
            "limit": 25,
            "direction": "prev",
            "kind": "shielded",
            "flow": "fully_shielded",
            "pool": "ironwood",
            "min_zec": 100
        }))
        .expect("valid explorer transaction request should deserialize");

        assert_eq!(request.direction, PageDirection::Previous);
        assert_eq!(request.kind, TransactionKindFilter::Shielded);
        assert_eq!(request.flow, ShieldedFlowFilter::FullyShielded);
        assert_eq!(request.pool, ShieldedPoolFilter::Ironwood);
        request
            .transaction_query()
            .expect("compatible shielded filters should validate");
    }

    #[test]
    fn transaction_request_rejects_unsupported_filter_combinations() {
        let unsupported_amount = GetTransactionsRequest {
            min_zec: 42,
            ..Default::default()
        };
        assert!(unsupported_amount.transaction_query().is_err());

        let incompatible_filter = GetTransactionsRequest {
            kind: TransactionKindFilter::Transparent,
            pool: ShieldedPoolFilter::Ironwood,
            ..Default::default()
        };
        assert!(incompatible_filter.transaction_query().is_err());
    }

    #[test]
    fn explorer_requests_reject_unknown_fields() {
        assert!(
            serde_json::from_value::<GetBlocksRequest>(serde_json::json!({
                "unknown": true
            }))
            .is_err()
        );
    }
}
