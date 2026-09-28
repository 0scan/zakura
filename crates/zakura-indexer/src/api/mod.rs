//! Read-only HTTP API backed directly by the in-process explorer index.
//!
//! The first version exposes `GET /api/v1/blocks?limit=5&cursor=...` on a
//! dedicated listener. The cursor is the opaque `nextCursor` value returned by
//! the preceding page.

mod config;
mod response;
mod routes;
mod server;

pub use config::Config;
pub use server::init;
