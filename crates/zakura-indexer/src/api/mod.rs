//! Read-only HTTP API backed directly by the in-process explorer index.
//!
//! The API exposes cursor-paginated canonical blocks and transactions at
//! `GET /api/v1/blocks` and `GET /api/v1/transactions`, plus complete block
//! details at `GET /api/v1/blocks/{height_or_hash}` and transaction details at
//! `GET /api/v1/transactions/{txid}` on a dedicated listener.
//! List routes use `direction=next|prev` with the corresponding cursor returned
//! in `next_cursor` or `prev_cursor`; responses remain newest-first in both directions.

mod config;
mod response;
mod routes;
mod server;

pub use config::Config;
pub use server::init;
