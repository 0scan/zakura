//! Rebuildable explorer indexes maintained inside the Zakura node process.
//!
//! [`Indexer`] owns one separate RocksDB shared by block, transaction, and
//! future explorer indexes. Synchronization reads committed data through
//! in-process services and never depends on Zakura's JSON-RPC or gRPC listeners.

mod blocks;
mod database;
mod error;
mod indexer;
mod sync;
mod types;

pub mod api;

pub use database::DATABASE_FORMAT_VERSION;
pub use error::Error;
pub use indexer::Indexer;
pub use sync::spawn_block_sync;
pub use types::{BlockRecord, BlocksPagination, BlocksResponse};
