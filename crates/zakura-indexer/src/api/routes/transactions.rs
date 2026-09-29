//! Recent canonical transaction route and strict filter parsing.

use std::time::Duration;

use hyper::StatusCode;

use crate::{
    transactions::{
        AmountFilter, ShieldedFlowFilter, ShieldedPoolFilter, TransactionKindFilter,
        TransactionQuery,
    },
    Error, Indexer,
};

use super::super::response::{self, ApiResponse};

const QUERY_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_QUERY_LENGTH: usize = 1_024;

#[derive(Default)]
struct TransactionsQuery {
    limit: Option<u32>,
    cursor: Option<String>,
    filters: TransactionQuery,
}

pub(super) async fn get(query: Option<&str>, indexer: Indexer) -> ApiResponse {
    let query = match TransactionsQuery::parse(query) {
        Ok(query) => query,
        Err(message) => return response::error(StatusCode::BAD_REQUEST, &message),
    };

    match tokio::time::timeout(
        QUERY_TIMEOUT,
        indexer.recent_transactions(query.filters, query.limit, query.cursor),
    )
    .await
    {
        Ok(Ok(transactions)) => response::json(StatusCode::OK, &transactions),
        Ok(Err(Error::InvalidCursor(message))) => {
            response::error(StatusCode::BAD_REQUEST, &message)
        }
        Ok(Err(error)) => {
            tracing::error!(?error, "explorer transaction query failed");
            response::error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "transaction query failed",
            )
        }
        Err(_) => {
            tracing::warn!("explorer transaction query timed out");
            response::error(StatusCode::GATEWAY_TIMEOUT, "transaction query timed out")
        }
    }
}

impl TransactionsQuery {
    fn parse(query: Option<&str>) -> Result<Self, String> {
        let Some(query) = query else {
            return Ok(Self::default());
        };
        if query.len() > MAX_QUERY_LENGTH {
            return Err("query string is too long".to_string());
        }

        let mut parsed = Self::default();
        let mut seen = std::collections::HashSet::new();
        for parameter in query.split('&').filter(|parameter| !parameter.is_empty()) {
            let (name, value) = parameter
                .split_once('=')
                .ok_or_else(|| format!("query parameter `{parameter}` must contain `=`"))?;
            if value.is_empty() || !seen.insert(name) {
                return Err(format!(
                    "query parameter `{name}` must appear once and have a value"
                ));
            }

            match name {
                "limit" => {
                    parsed.limit = Some(value.parse::<u32>().map_err(|_| {
                        "query parameter `limit` must be an unsigned integer".to_string()
                    })?);
                }
                "cursor" => parsed.cursor = Some(value.to_string()),
                "type" => parsed.filters.kind = parse_kind(value)?,
                "flow_type" => parsed.filters.flow = parse_flow(value)?,
                "pool" => parsed.filters.pool = parse_pool(value)?,
                "min_zec" => parsed.filters.amount = parse_amount(value)?,
                _ => return Err(format!("unknown query parameter `{name}`")),
            }
        }

        parsed.filters = parsed.filters.validate()?;
        Ok(parsed)
    }
}

fn parse_kind(value: &str) -> Result<TransactionKindFilter, String> {
    match value {
        "all" => Ok(TransactionKindFilter::All),
        "shielded" => Ok(TransactionKindFilter::Shielded),
        "transparent" => Ok(TransactionKindFilter::Transparent),
        "coinbase" => Ok(TransactionKindFilter::Coinbase),
        _ => Err("type must be all, shielded, transparent, or coinbase".to_string()),
    }
}

fn parse_flow(value: &str) -> Result<ShieldedFlowFilter, String> {
    match value {
        "all" => Ok(ShieldedFlowFilter::All),
        "shield" => Ok(ShieldedFlowFilter::Shield),
        "deshield" => Ok(ShieldedFlowFilter::Deshield),
        "fully_shielded" => Ok(ShieldedFlowFilter::FullyShielded),
        "complex" => Ok(ShieldedFlowFilter::Complex),
        _ => Err("flow_type must be all, shield, deshield, fully_shielded, or complex".to_string()),
    }
}

fn parse_pool(value: &str) -> Result<ShieldedPoolFilter, String> {
    match value {
        "all" => Ok(ShieldedPoolFilter::All),
        "sprout" => Ok(ShieldedPoolFilter::Sprout),
        "sapling" => Ok(ShieldedPoolFilter::Sapling),
        "orchard" => Ok(ShieldedPoolFilter::Orchard),
        "ironwood" => Ok(ShieldedPoolFilter::Ironwood),
        "mixed" => Ok(ShieldedPoolFilter::Mixed),
        _ => Err("pool must be all, sprout, sapling, orchard, ironwood, or mixed".to_string()),
    }
}

fn parse_amount(value: &str) -> Result<AmountFilter, String> {
    match value {
        "0" => Ok(AmountFilter::Any),
        "10" => Ok(AmountFilter::AtLeastTenZec),
        "100" => Ok(AmountFilter::AtLeastOneHundredZec),
        "1000" => Ok(AmountFilter::AtLeastOneThousandZec),
        _ => Err("min_zec must be 0, 10, 100, or 1000".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cipher_scan_compatible_filters() {
        let query = TransactionsQuery::parse(Some(
            "limit=25&type=shielded&flow_type=shield&pool=ironwood&min_zec=10",
        ))
        .expect("supported transaction filters should parse");

        assert_eq!(query.limit, Some(25));
        assert_eq!(query.filters.kind, TransactionKindFilter::Shielded);
        assert_eq!(query.filters.flow, ShieldedFlowFilter::Shield);
        assert_eq!(query.filters.pool, ShieldedPoolFilter::Ironwood);
        assert_eq!(query.filters.amount, AmountFilter::AtLeastTenZec);
    }

    #[test]
    fn rejects_shielded_subfilters_for_other_transaction_types() {
        assert!(TransactionsQuery::parse(Some("type=all&pool=ironwood")).is_err());
        assert!(TransactionsQuery::parse(Some("type=transparent&min_zec=10")).is_err());
    }

    #[test]
    fn rejects_unindexed_arbitrary_amount_thresholds() {
        assert!(TransactionsQuery::parse(Some("type=shielded&min_zec=42")).is_err());
    }
}
