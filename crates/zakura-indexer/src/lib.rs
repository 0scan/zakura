//! Rebuildable explorer indexes maintained inside the Zakura node process.
//!
//! [`Indexer`] owns one separate RocksDB shared by block, transaction, and
//! future explorer indexes. Synchronization reads committed data through
//! in-process services and never depends on Zakura's JSON-RPC or gRPC listeners.

mod blocks;
mod database;
mod error;
mod indexer;
mod models;
mod sync;
mod transactions;
mod types;

pub mod api;

pub use database::DATABASE_FORMAT_VERSION;
pub use error::Error;
pub use indexer::Indexer;
pub use sync::spawn_block_sync;
pub use types::{
    BlockDetails, BlockRecord, BlockTransaction, BlockTransactionInput, BlockTransactionOutput,
    BlockTrees, BlocksPagination, BlocksResponse, PageDirection, ShieldedFlow, ShieldedPool,
    TransactionKind, TransactionListItem, TransactionsPagination, TransactionsResponse, TreeSize,
    ValuePoolBalance,
};
