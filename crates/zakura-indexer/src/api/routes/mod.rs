//! Versioned explorer REST routes.

mod blocks;
mod transactions;

use std::convert::Infallible;

use hyper::{Method, Request, StatusCode};
use zakura_state::ReadState;

use crate::Indexer;

use super::response::{self, ApiResponse};

const BLOCKS_PATH: &str = "/api/v1/blocks";
const TRANSACTIONS_PATH: &str = "/api/v1/transactions";

pub(super) async fn handle<B, State>(
    request: Request<B>,
    indexer: Indexer,
    read_state: State,
) -> Result<ApiResponse, Infallible>
where
    State: ReadState,
{
    let path = request.uri().path();
    let block_identifier = path
        .strip_prefix(BLOCKS_PATH)
        .and_then(|suffix| suffix.strip_prefix('/'))
        .filter(|identifier| !identifier.is_empty() && !identifier.contains('/'));
    let response = match (request.method(), path, block_identifier) {
        (&Method::GET, BLOCKS_PATH, _) => blocks::get(request.uri().query(), indexer).await,
        (&Method::OPTIONS, BLOCKS_PATH, _) => response::empty(StatusCode::NO_CONTENT),
        (&Method::GET, TRANSACTIONS_PATH, _) => {
            transactions::get(request.uri().query(), indexer).await
        }
        (&Method::OPTIONS, TRANSACTIONS_PATH, _) => response::empty(StatusCode::NO_CONTENT),
        (&Method::GET, _, Some(identifier)) => {
            blocks::get_details(identifier, indexer, read_state).await
        }
        (&Method::OPTIONS, _, Some(_)) => response::empty(StatusCode::NO_CONTENT),
        (_, BLOCKS_PATH, _) | (_, TRANSACTIONS_PATH, _) | (_, _, Some(_)) => response::error(
            StatusCode::METHOD_NOT_ALLOWED,
            "only GET and OPTIONS are supported for this route",
        ),
        _ => response::error(StatusCode::NOT_FOUND, "route not found"),
    };

    Ok(response)
}

#[cfg(test)]
mod tests {
    use http_body_util::BodyExt;
    use hyper::{header::ACCESS_CONTROL_ALLOW_ORIGIN, Request, StatusCode};
    use serde_json::Value;
    use tower::BoxError;
    use zakura_chain::parameters::Network;
    use zakura_state::{ReadRequest, ReadResponse};

    use crate::Indexer;

    use super::handle;

    #[tokio::test]
    async fn returns_an_empty_block_page_as_json() {
        let response = request("/api/v1/blocks?limit=7").await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[ACCESS_CONTROL_ALLOW_ORIGIN], "*");
        let body = response
            .into_body()
            .collect()
            .await
            .expect("test response body should be readable")
            .to_bytes();
        let body: Value = serde_json::from_slice(&body).expect("response should contain JSON");
        assert_eq!(body["blocks"], serde_json::json!([]));
        assert_eq!(body["pagination"]["limit"], 7);
        assert_eq!(body["pagination"]["total"], "0");
        assert_eq!(body["pagination"]["hasNext"], false);
        assert_eq!(body["pagination"]["hasPrev"], false);
        assert_eq!(body["pagination"]["nextCursor"], Value::Null);
        assert_eq!(body["pagination"]["prevCursor"], Value::Null);
    }

    #[tokio::test]
    async fn rejects_invalid_block_query_parameters() {
        let response = request("/api/v1/blocks?limit=not-a-number").await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let response = request("/api/v1/blocks?unknown=value").await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let response = request("/api/v1/blocks?direction=prev").await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn rejects_invalid_or_stale_cursors_as_bad_requests() {
        let response = request("/api/v1/blocks?cursor=not-a-cursor").await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn returns_an_empty_transaction_page_as_json() {
        let response = request("/api/v1/transactions?limit=25&type=shielded").await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = response
            .into_body()
            .collect()
            .await
            .expect("test response body should be readable")
            .to_bytes();
        let body: Value = serde_json::from_slice(&body).expect("response should contain JSON");
        assert_eq!(body["transactions"], serde_json::json!([]));
        assert_eq!(body["pagination"]["limit"], 25);
        assert_eq!(body["pagination"]["hasNext"], false);
        assert_eq!(body["pagination"]["hasPrev"], false);
        assert_eq!(body["pagination"]["nextCursor"], Value::Null);
        assert_eq!(body["pagination"]["prevCursor"], Value::Null);
    }

    #[tokio::test]
    async fn rejects_invalid_transaction_filters() {
        let response = request("/api/v1/transactions?type=all&pool=ironwood").await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn returns_not_found_for_unknown_routes() {
        let response = request("/api/v1/not-a-route").await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn validates_and_resolves_block_detail_identifiers() {
        let response = request("/api/v1/blocks/not-a-block").await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let response = request("/api/v1/blocks/42").await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    async fn request(uri: &str) -> super::ApiResponse {
        let indexer =
            Indexer::open_ephemeral(Network::Mainnet).expect("ephemeral test indexer should open");
        let request = Request::builder()
            .uri(uri)
            .body(())
            .expect("test request should be valid");

        handle(request, indexer, empty_read_state())
            .await
            .expect("explorer request handling is infallible")
    }

    fn empty_read_state() -> impl tower::Service<
        ReadRequest,
        Response = ReadResponse,
        Error = BoxError,
        Future: Send + 'static,
    > + Clone
           + Send
           + Sync
           + 'static {
        tower::service_fn(|request: ReadRequest| async move {
            match request {
                ReadRequest::BlockAndSize(_) => Ok(ReadResponse::BlockAndSize(None)),
                request => Err(format!("unexpected test state request: {request:?}").into()),
            }
        })
    }
}
