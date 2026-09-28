//! Recent canonical block route.

use std::time::Duration;

use hyper::StatusCode;

use crate::{Error, Indexer};

use super::super::response::{self, ApiResponse};

const QUERY_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_QUERY_LENGTH: usize = 512;

#[derive(Default)]
struct BlocksQuery {
    limit: Option<u32>,
    cursor: Option<String>,
}

pub(super) async fn get(query: Option<&str>, indexer: Indexer) -> ApiResponse {
    let query = match BlocksQuery::parse(query) {
        Ok(query) => query,
        Err(message) => return response::error(StatusCode::BAD_REQUEST, &message),
    };

    match tokio::time::timeout(
        QUERY_TIMEOUT,
        indexer.recent_blocks(query.limit, query.cursor),
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

impl BlocksQuery {
    fn parse(query: Option<&str>) -> Result<Self, String> {
        let Some(query) = query else {
            return Ok(Self::default());
        };

        if query.len() > MAX_QUERY_LENGTH {
            return Err("query string is too long".to_string());
        }

        let mut parsed = Self::default();
        for parameter in query.split('&').filter(|parameter| !parameter.is_empty()) {
            let (name, value) = parameter
                .split_once('=')
                .ok_or_else(|| format!("query parameter `{parameter}` must contain `=`"))?;

            match name {
                "limit" if parsed.limit.is_none() => {
                    parsed.limit = Some(value.parse::<u32>().map_err(|_| {
                        "query parameter `limit` must be an unsigned integer".to_string()
                    })?);
                }
                "cursor" if parsed.cursor.is_none() && !value.is_empty() => {
                    parsed.cursor = Some(value.to_string());
                }
                "limit" | "cursor" => {
                    return Err(format!(
                        "query parameter `{name}` must appear once and have a value"
                    ));
                }
                _ => return Err(format!("unknown query parameter `{name}`")),
            }
        }

        Ok(parsed)
    }
}

#[cfg(test)]
mod tests {
    use super::BlocksQuery;

    #[test]
    fn parses_cursor_pagination_query() {
        let query = BlocksQuery::parse(Some("limit=5&cursor=abc_DEF-123"))
            .expect("valid query should parse");

        assert_eq!(query.limit, Some(5));
        assert_eq!(query.cursor.as_deref(), Some("abc_DEF-123"));
    }

    #[test]
    fn rejects_duplicate_parameters() {
        assert!(BlocksQuery::parse(Some("limit=5&limit=10")).is_err());
        assert!(BlocksQuery::parse(Some("cursor=one&cursor=two")).is_err());
    }
}
