//! Read-only HTTP API backed directly by the in-process explorer index.
//!
//! The API exposes cursor-paginated canonical blocks at
//! `GET /api/v1/blocks?limit=5&cursor=...` and complete canonical block details
//! at `GET /api/v1/blocks/{height_or_hash}` on a dedicated listener.

mod config;
mod response;
mod routes;
mod server;

pub use config::Config;
pub use server::init;
