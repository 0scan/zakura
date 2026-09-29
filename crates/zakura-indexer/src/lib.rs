//! Rebuildable explorer indexes maintained inside the Zakura node process.
//!
//! [`Indexer`] owns one separate RocksDB shared by block, transaction, and
//! future explorer indexes. Synchronization reads committed data through
//! in-process services and never depends on Zakura's JSON-RPC or gRPC listeners.

mod addresses;
mod blocks;
mod database;
mod error;
mod indexer;
mod models;
mod stats;
mod sync;
mod transactions;
mod types;

pub use database::DATABASE_FORMAT_VERSION;
pub use error::Error;
pub use indexer::Indexer;
pub use sync::spawn_block_sync;
pub use transactions::{
    classify_unmined_transaction, AmountFilter, ShieldedFlowFilter, ShieldedPoolFilter,
    TransactionKindFilter, TransactionQuery,
};
pub use types::{
    AddressActivity, AddressFirstFunding, AddressSummary, AddressTransactionListItem,
    AddressTransactionsPagination, AddressTransactionsResponse, BlockDetails, BlockRecord,
    BlockTransaction, BlockTransactionInput, BlockTransactionOutput, BlockTrees, BlocksPagination,
    BlocksResponse, ChainTotals, ChartDataEntry, ChartDataRequest, ChartDataResponse, IndexerStats,
    PageDirection, RollingDayStats, ShieldedFlow, ShieldedPool, TransactionClassification,
    TransactionData, TransactionDetails, TransactionKind, TransactionListItem, TransactionStatus,
    TransactionsPagination, TransactionsResponse, TreeSize, ValuePoolBalance,
};
