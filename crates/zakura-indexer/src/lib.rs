//! Explorer response types and canonical-state adapters.

#[cfg(feature = "state-index")]
mod addresses;
#[cfg(feature = "state-index")]
mod blocks;
mod error;
mod height_range;
mod models;
#[cfg(feature = "state-index")]
mod stats;
mod transactions;
mod types;

#[cfg(feature = "state-index")]
pub use addresses::{
    address_summary_from_state, address_transactions_page_from_state,
    address_utxos_page_from_state, top_balances_from_state,
};
#[cfg(feature = "state-index")]
pub use blocks::{block_details_from_state, blocks_page_from_state};
pub use error::Error;
#[cfg(feature = "state-index")]
pub use stats::{chart_data_from_state, stats_from_state};
pub use transactions::{
    classify_unmined_transaction, primary_transaction_endpoints, AmountFilter, ShieldedFlowFilter,
    ShieldedPoolFilter, TransactionKindFilter, TransactionQuery,
};
#[cfg(feature = "state-index")]
pub use transactions::{transaction_details_from_state, transactions_page_from_state};
pub use types::{
    AddressActivity, AddressFirstFunding, AddressSummary, AddressTransactionListItem,
    AddressTransactionsPagination, AddressTransactionsResponse, AddressUtxoSummary,
    AddressUtxosPagination, AddressUtxosResponse, BlockDetails, BlockRecord, BlockTransaction,
    BlockTransactionInput, BlockTransactionOutput, BlockTrees, BlocksPagination, BlocksResponse,
    ChainTotals, ChartDataEntry, ChartDataRequest, ChartDataResponse, IndexerStats, PageDirection,
    RollingDayStats, ShieldedFlow, ShieldedPool, TopBalanceEntry, TopBalancesPagination,
    TopBalancesRequest, TopBalancesResponse, TopBalancesSummary, TransactionClassification,
    TransactionData, TransactionDetails, TransactionEndpoint, TransactionEndpointType,
    TransactionKind, TransactionListItem, TransactionStatus, TransactionsPagination,
    TransactionsResponse, TreeSize, ValuePoolBalance,
};
