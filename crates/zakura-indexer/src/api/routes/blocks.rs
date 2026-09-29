//! Recent canonical block route.

use std::time::Duration;

use hyper::StatusCode;
use zakura_state::{HashOrHeight, ReadState};

use crate::{Error, Indexer, PageDirection};

use super::super::response::{self, ApiResponse};

const QUERY_TIMEOUT: Duration = Duration::from_secs(10);
const DETAIL_QUERY_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_QUERY_LENGTH: usize = 512;

#[derive(Default)]
struct BlocksQuery {
    limit: Option<u32>,
    cursor: Option<String>,
    direction: PageDirection,
}

pub(super) async fn get(query: Option<&str>, indexer: Indexer) -> ApiResponse {
    let query = match BlocksQuery::parse(query) {
        Ok(query) => query,
        Err(message) => return response::error(StatusCode::BAD_REQUEST, &message),
    };

    match tokio::time::timeout(
        QUERY_TIMEOUT,
        indexer.blocks_page(query.limit, query.cursor, query.direction),
    )
    .await
    {
        Ok(Ok(blocks)) => response::json(StatusCode::OK, &blocks),
        Ok(Err(Error::InvalidCursor(message))) => {
            response::error(StatusCode::BAD_REQUEST, &message)
        }
        Ok(Err(error)) => {
            tracing::error!(?error, "explorer block query failed");
            response::error(StatusCode::INTERNAL_SERVER_ERROR, "block query failed")
        }
        Err(_) => {
            tracing::warn!("explorer block query timed out");
            response::error(StatusCode::GATEWAY_TIMEOUT, "block query timed out")
        }
    }
}

pub(super) async fn get_details<State>(
    identifier: &str,
    indexer: Indexer,
    read_state: State,
) -> ApiResponse
where
    State: ReadState,
{
    let identifier = match HashOrHeight::new(identifier, None) {
        Ok(identifier) => identifier,
        Err(message) => return response::error(StatusCode::BAD_REQUEST, &message),
    };

    match tokio::time::timeout(
        DETAIL_QUERY_TIMEOUT,
        indexer.block_details(read_state, identifier),
    )
    .await
    {
        Ok(Ok(Some(block))) => response::json(StatusCode::OK, &block),
        Ok(Ok(None)) => response::error(StatusCode::NOT_FOUND, "block not found"),
        Ok(Err(Error::BlockNotIndexed(message))) => {
            tracing::debug!(%message, "explorer block detail is waiting for the index");
            response::error(
                StatusCode::SERVICE_UNAVAILABLE,
                "block index is still catching up",
            )
        }
        Ok(Err(Error::StateRequest(message))) => {
            tracing::error!(%message, "explorer state query failed");
            response::error(StatusCode::SERVICE_UNAVAILABLE, "block state query failed")
        }
        Ok(Err(error)) => {
            tracing::error!(?error, "explorer block detail query failed");
            response::error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "block detail query failed",
            )
        }
        Err(_) => {
            tracing::warn!("explorer block detail query timed out");
            response::error(StatusCode::GATEWAY_TIMEOUT, "block detail query timed out")
        }
    }
}

impl BlocksQuery {
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
                "direction" if value == "next" => parsed.direction = PageDirection::Next,
                "direction" if value == "prev" => parsed.direction = PageDirection::Previous,
                "direction" => {
                    return Err("direction must be next or prev".to_string());
                }
                _ => return Err(format!("unknown query parameter `{name}`")),
            }
        }

        if parsed.direction == PageDirection::Previous && parsed.cursor.is_none() {
            return Err("direction=prev requires a cursor".to_string());
        }

        Ok(parsed)
    }
}

#[cfg(test)]
mod tests {
    use super::{BlocksQuery, PageDirection};

    #[test]
    fn parses_cursor_pagination_query() {
        let query = BlocksQuery::parse(Some("limit=5&cursor=abc_DEF-123"))
            .expect("valid query should parse");

        assert_eq!(query.limit, Some(5));
        assert_eq!(query.cursor.as_deref(), Some("abc_DEF-123"));
        assert_eq!(query.direction, PageDirection::Next);
    }

    #[test]
    fn parses_previous_page_direction() {
        let query = BlocksQuery::parse(Some("cursor=abc_DEF-123&direction=prev"))
            .expect("previous query should parse");

        assert_eq!(query.direction, PageDirection::Previous);
    }

    #[test]
    fn rejects_duplicate_parameters() {
        assert!(BlocksQuery::parse(Some("limit=5&limit=10")).is_err());
        assert!(BlocksQuery::parse(Some("cursor=one&cursor=two")).is_err());
        assert!(BlocksQuery::parse(Some("cursor=one&direction=next&direction=prev")).is_err());
        assert!(BlocksQuery::parse(Some("direction=prev")).is_err());
    }
}
