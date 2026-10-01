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

    /// Minimum public flow amount in zatoshis: 0, 1B, 10B, or 100B.
    pub min_zat: u64,
}

/// Cursor pagination parameters for `getaddresstransactions`.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetAddressTransactionsRequest {
    /// Transparent address whose canonical transaction history is requested.
    pub address: String,

    /// Maximum records to return. The indexer clamps this value to 1–100.
    #[serde(default)]
    pub limit: Option<u32>,

    /// Opaque cursor returned by an earlier response for this address.
    #[serde(default)]
    pub cursor: Option<String>,

    /// Direction to move relative to `cursor`.
    #[serde(default)]
    pub direction: PageDirection,
}

/// Cursor pagination parameters for `getaddressutxospage`.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetAddressUtxosPageRequest {
    /// Transparent address whose current unspent outputs are requested.
    pub address: String,

    /// Maximum records to return. The indexer clamps this value to 1–100.
    #[serde(default)]
    pub limit: Option<u32>,

    /// Opaque cursor returned by an earlier response for this address.
    #[serde(default)]
    pub cursor: Option<String>,

    /// Direction to move relative to `cursor`.
    #[serde(default)]
    pub direction: PageDirection,
}

impl GetTransactionsRequest {
    /// Converts transport parameters into the indexer's validated query type.
    pub(crate) fn transaction_query(&self) -> Result<TransactionQuery, String> {
        TransactionQuery::new(
            self.kind,
            self.flow,
            self.pool,
            AmountFilter::from_minimum_zat(self.min_zat)?,
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
            "min_zat": 10_000_000_000_u64
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
            min_zat: 42,
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
        assert!(
            serde_json::from_value::<GetAddressTransactionsRequest>(serde_json::json!({
                "address": "t1example",
                "unknown": true
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<GetAddressUtxosPageRequest>(serde_json::json!({
                "address": "t1example",
                "unknown": true
            }))
            .is_err()
        );
    }

    #[test]
    fn address_transaction_request_defaults_to_newest_page() {
        let request: GetAddressTransactionsRequest = serde_json::from_value(serde_json::json!({
            "address": "t1example"
        }))
        .expect("pagination fields should be optional");

        assert_eq!(request.address, "t1example");
        assert_eq!(request.limit, None);
        assert_eq!(request.cursor, None);
        assert_eq!(request.direction, PageDirection::Next);
    }

    #[test]
    fn address_utxo_request_defaults_to_newest_page() {
        let request: GetAddressUtxosPageRequest = serde_json::from_value(serde_json::json!({
            "address": "t1example"
        }))
        .expect("pagination fields should be optional");

        assert_eq!(request.address, "t1example");
        assert_eq!(request.limit, None);
        assert_eq!(request.cursor, None);
        assert_eq!(request.direction, PageDirection::Next);
    }
}
