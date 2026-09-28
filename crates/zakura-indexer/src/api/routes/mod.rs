//! Versioned explorer REST routes.

mod blocks;

use std::convert::Infallible;

use hyper::{Method, Request, StatusCode};

use crate::Indexer;

use super::response::{self, ApiResponse};

const BLOCKS_PATH: &str = "/api/v1/blocks";

pub(super) async fn handle<B>(
    request: Request<B>,
    indexer: Indexer,
) -> Result<ApiResponse, Infallible> {
    let response = match (request.method(), request.uri().path()) {
        (&Method::GET, BLOCKS_PATH) => blocks::get(request.uri().query(), indexer).await,
        (&Method::OPTIONS, BLOCKS_PATH) => response::empty(StatusCode::NO_CONTENT),
        (_, BLOCKS_PATH) => response::error(
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
    use zakura_chain::parameters::Network;

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
        assert_eq!(body["pagination"]["hasMore"], false);
    }

    #[tokio::test]
    async fn rejects_invalid_block_query_parameters() {
        let response = request("/api/v1/blocks?limit=not-a-number").await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let response = request("/api/v1/blocks?unknown=value").await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn rejects_invalid_or_stale_cursors_as_bad_requests() {
        let response = request("/api/v1/blocks?cursor=not-a-cursor").await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn returns_not_found_for_unknown_routes() {
        let response = request("/api/v1/transactions").await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    async fn request(uri: &str) -> super::ApiResponse {
        let indexer =
            Indexer::open_ephemeral(Network::Mainnet).expect("ephemeral test indexer should open");
        let request = Request::builder()
            .uri(uri)
            .body(())
            .expect("test request should be valid");

        handle(request, indexer)
            .await
            .expect("explorer request handling is infallible")
    }
}
