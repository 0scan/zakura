//! Explorer state subsystem.
//!
//! This facade owns the explorer query contracts, canonical reads, analytics,
//! and durable indexes. Core state modules only call its commit, rollback, and
//! read entry points.

mod analytics;
pub(crate) mod read;
mod storage;
mod types;

pub use storage::{
    apply_prepared_transaction_amount_refill, prepare_transaction_amount_refill,
    refill_transaction_amounts, AppliedPreparedTransactionAmountRefillSummary,
    PreparedTransactionAmountRefillSummary, RefillTransactionAmountsError,
    RefillTransactionAmountsOptions, RefillTransactionAmountsSummary,
};
pub(crate) use storage::{
    ExplorerBlockCommitContext, PendingExplorerAddressRecords, EXPLORER_ADDRESS_META,
    EXPLORER_BALANCE_ORDER, EXPLORER_BLOCK_STATS, EXPLORER_CHAIN_STATS, EXPLORER_DAILY_STATS,
    EXPLORER_SCHEMA, EXPLORER_SHIELDED_TRANSACTION_BY_CLASS_LOC, EXPLORER_TRANSACTION_BY_KIND_LOC,
    EXPLORER_TRANSACTION_META_BY_LOC,
};
pub use types::*;
