//! JSON Schemas for the explorer index request types.

use serde_json::{json, Value};

use zakura_indexer::{
    PageDirection, ShieldedFlowFilter, ShieldedPoolFilter, TransactionKindFilter,
};

use super::{described, string_enum, Generator, RpcSchema};
use crate::methods::{
    ChartDataRequest, GetAddressTransactionsRequest, GetAddressUtxosPageRequest, GetBlocksRequest,
    GetMempoolTransactionsRequest, GetTransactionsRequest, TopBalancesRequest,
};

const LIMIT_DESC: &str = "Maximum records to return. The indexer clamps this value to 1–100.";
const DIRECTION_DESC: &str = "Direction to move relative to `cursor`.";
const KIND_DESC: &str = "Top-level transaction kind filter.";
const FLOW_DESC: &str = "Observable transparent/shielded boundary-flow filter.";
const POOL_DESC: &str = "Shielded pool filter.";
const FROM_HEIGHT_DESC: &str = "Inclusive lower block-height bound.";
const TO_HEIGHT_DESC: &str = "Inclusive upper block-height bound.";

impl RpcSchema for PageDirection {
    const COMPONENT: Option<&'static str> = Some("PageDirection");

    fn schema(_: &mut Generator) -> Value {
        string_enum(
            "Direction to move from an opaque page cursor.",
            &[
                (
                    "next",
                    "Return records older than the cursor, or the newest records without a cursor.",
                ),
                (
                    "prev",
                    "Return the immediately newer page preceding the cursor.",
                ),
            ],
        )
    }
}

impl RpcSchema for TransactionKindFilter {
    const COMPONENT: Option<&'static str> = Some("TransactionKindFilter");

    fn schema(_: &mut Generator) -> Value {
        string_enum(
            "Top-level transaction kind selector.",
            &[
                ("all", "Include every transaction kind."),
                (
                    "shielded",
                    "Include transactions that touch at least one shielded pool.",
                ),
                (
                    "transparent",
                    "Include non-coinbase transactions with transparent components only.",
                ),
                ("coinbase", "Include coinbase transactions."),
            ],
        )
    }
}

impl RpcSchema for ShieldedFlowFilter {
    const COMPONENT: Option<&'static str> = Some("ShieldedFlowFilter");

    fn schema(_: &mut Generator) -> Value {
        string_enum(
            "Observable shielded-flow selector.",
            &[
                ("all", "Include every observable shielded flow."),
                ("shield", "Include public value entering a shielded pool."),
                ("deshield", "Include shielded value becoming public."),
                (
                    "fully_shielded",
                    "Include transactions with shielded components and no transparent inputs or outputs.",
                ),
                (
                    "complex",
                    "Include transactions whose public boundary flow cannot be represented by one direction.",
                ),
            ],
        )
    }
}

impl RpcSchema for ShieldedPoolFilter {
    const COMPONENT: Option<&'static str> = Some("ShieldedPoolFilter");

    fn schema(_: &mut Generator) -> Value {
        string_enum(
            "Shielded-pool selector.",
            &[
                ("all", "Include every shielded pool."),
                ("sprout", "Include Sprout transactions."),
                ("sapling", "Include Sapling transactions."),
                ("orchard", "Include Orchard transactions."),
                ("ironwood", "Include Ironwood transactions."),
                (
                    "mixed",
                    "Include transactions that touch multiple shielded pools.",
                ),
            ],
        )
    }
}

impl RpcSchema for GetBlocksRequest {
    const COMPONENT: Option<&'static str> = Some("GetBlocksRequest");

    fn schema(generator: &mut Generator) -> Value {
        json!({
            "description": "Cursor pagination parameters for `getblocks`.",
            "type": "object",
            "properties": {
                "limit": described::<Option<u32>>(generator, LIMIT_DESC),
                "cursor": described::<Option<String>>(
                    generator,
                    "Opaque cursor returned by an earlier `getblocks` response.",
                ),
                "direction": described::<PageDirection>(generator, DIRECTION_DESC),
            },
            "additionalProperties": false,
        })
    }
}

impl RpcSchema for GetTransactionsRequest {
    const COMPONENT: Option<&'static str> = Some("GetTransactionsRequest");

    fn schema(generator: &mut Generator) -> Value {
        json!({
            "description": "Cursor pagination and filter parameters for `gettransactions`.",
            "type": "object",
            "properties": {
                "limit": described::<Option<u32>>(generator, LIMIT_DESC),
                "cursor": described::<Option<String>>(
                    generator,
                    "Opaque cursor returned by an earlier `gettransactions` response.",
                ),
                "direction": described::<PageDirection>(generator, DIRECTION_DESC),
                "kind": described::<TransactionKindFilter>(generator, KIND_DESC),
                "flow": described::<ShieldedFlowFilter>(generator, FLOW_DESC),
                "pool": described::<ShieldedPoolFilter>(generator, POOL_DESC),
                "min_zat": min_zat_schema(),
                "from_height": described::<Option<u32>>(generator, FROM_HEIGHT_DESC),
                "to_height": described::<Option<u32>>(generator, TO_HEIGHT_DESC),
            },
            "additionalProperties": false,
        })
    }
}

impl RpcSchema for GetMempoolTransactionsRequest {
    const COMPONENT: Option<&'static str> = Some("GetMempoolTransactionsRequest");

    fn schema(generator: &mut Generator) -> Value {
        json!({
            "description": "Cursor pagination and filter parameters for `getmempooltransactions`.",
            "type": "object",
            "properties": {
                "limit": described::<Option<u32>>(generator, LIMIT_DESC),
                "cursor": described::<Option<String>>(
                    generator,
                    "Opaque cursor returned by an earlier `getmempooltransactions` response.",
                ),
                "direction": described::<PageDirection>(generator, DIRECTION_DESC),
                "kind": described::<TransactionKindFilter>(generator, KIND_DESC),
                "flow": described::<ShieldedFlowFilter>(generator, FLOW_DESC),
                "pool": described::<ShieldedPoolFilter>(generator, POOL_DESC),
                "min_zat": min_zat_schema(),
            },
            "additionalProperties": false,
        })
    }
}

impl RpcSchema for GetAddressTransactionsRequest {
    const COMPONENT: Option<&'static str> = Some("GetAddressTransactionsRequest");

    fn schema(generator: &mut Generator) -> Value {
        json!({
            "description": "Cursor pagination parameters for `getaddresstransactions`.",
            "type": "object",
            "properties": {
                "address": described::<String>(
                    generator,
                    "Transparent address whose canonical transaction history is requested.",
                ),
                "limit": described::<Option<u32>>(generator, LIMIT_DESC),
                "cursor": described::<Option<String>>(
                    generator,
                    "Opaque cursor returned by an earlier response for this address.",
                ),
                "direction": described::<PageDirection>(generator, DIRECTION_DESC),
                "from_height": described::<Option<u32>>(generator, FROM_HEIGHT_DESC),
                "to_height": described::<Option<u32>>(generator, TO_HEIGHT_DESC),
            },
            "required": ["address"],
            "additionalProperties": false,
        })
    }
}

impl RpcSchema for GetAddressUtxosPageRequest {
    const COMPONENT: Option<&'static str> = Some("GetAddressUtxosPageRequest");

    fn schema(generator: &mut Generator) -> Value {
        json!({
            "description": "Cursor pagination parameters for `getaddressutxospage`.",
            "type": "object",
            "properties": {
                "address": described::<String>(
                    generator,
                    "Transparent address whose current unspent outputs are requested.",
                ),
                "limit": described::<Option<u32>>(generator, LIMIT_DESC),
                "cursor": described::<Option<String>>(
                    generator,
                    "Opaque cursor returned by an earlier response for this address.",
                ),
                "direction": described::<PageDirection>(generator, DIRECTION_DESC),
            },
            "required": ["address"],
            "additionalProperties": false,
        })
    }
}

impl RpcSchema for ChartDataRequest {
    const COMPONENT: Option<&'static str> = Some("ChartDataRequest");

    fn schema(generator: &mut Generator) -> Value {
        json!({
            "description": "Inclusive UTC date range for chart snapshots.",
            "type": "object",
            "properties": {
                "start_date": described::<Option<String>>(
                    generator,
                    "First date to return, formatted as `YYYY-MM-DD`.",
                ),
                "end_date": described::<Option<String>>(
                    generator,
                    "Last date to return, formatted as `YYYY-MM-DD`.",
                ),
                "limit": {
                    "description": "Maximum number of entries to return.",
                    "type": ["integer", "null"],
                    "format": "uint",
                    "minimum": 0,
                },
                "include_non_finalized": described::<bool>(
                    generator,
                    "Include provisional daily aggregates from the best non-finalized chain.",
                ),
            },
            "additionalProperties": false,
        })
    }
}

impl RpcSchema for TopBalancesRequest {
    const COMPONENT: Option<&'static str> = Some("TopBalancesRequest");

    fn schema(generator: &mut Generator) -> Value {
        json!({
            "description": "Cursor-paginated top-balances request.",
            "type": "object",
            "properties": {
                "limit": described::<Option<u32>>(
                    generator,
                    "Maximum number of ranked addresses to return.",
                ),
                "cursor": described::<Option<String>>(
                    generator,
                    "Opaque cursor returned by the previous page.",
                ),
            },
            "additionalProperties": false,
        })
    }
}

fn min_zat_schema() -> Value {
    json!({
        "description": "Minimum public flow amount in zatoshis: 0, 1B, 10B, or 100B.",
        "type": "integer",
        "format": "uint64",
        "minimum": 0,
    })
}
