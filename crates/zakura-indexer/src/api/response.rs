//! Shared HTTP response construction for explorer routes.

use http_body_util::Full;
use hyper::{
    body::Bytes,
    header::{
        ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN,
        CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE,
    },
    Response, StatusCode,
};
use serde::Serialize;

pub(super) type ApiResponse = Response<Full<Bytes>>;

#[derive(Serialize)]
struct ErrorResponse<'a> {
    error: &'a str,
}

pub(super) fn json<T>(status: StatusCode, value: &T) -> ApiResponse
where
    T: Serialize,
{
    let body = serde_json::to_vec(value)
        .expect("explorer API response types contain only JSON-serializable fields");
    response(status, "application/json", body)
}

pub(super) fn error(status: StatusCode, message: &str) -> ApiResponse {
    json(status, &ErrorResponse { error: message })
}

pub(super) fn empty(status: StatusCode) -> ApiResponse {
    response(status, "application/json", Vec::new())
}

fn response(status: StatusCode, content_type: &str, body: Vec<u8>) -> ApiResponse {
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, content_type)
        .header(CONTENT_LENGTH, body.len().to_string())
        .header(CACHE_CONTROL, "no-store")
        .header(ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(ACCESS_CONTROL_ALLOW_METHODS, "GET, OPTIONS")
        .header(ACCESS_CONTROL_ALLOW_HEADERS, "Content-Type")
        .body(Full::new(Bytes::from(body)))
        .expect("static explorer response headers are valid")
}
