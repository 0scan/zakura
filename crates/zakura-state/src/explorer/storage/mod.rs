//! Durable explorer storage facade.

use crate::service::finalized_state::ZakuraDb;

mod balances;
mod columns;
pub(super) mod disk_format;
mod refill_transaction_amounts;
mod stats;
mod transactions;

pub(crate) use columns::{
    EXPLORER_ADDRESS_META, EXPLORER_BALANCE_ORDER, EXPLORER_BLOCK_STATS, EXPLORER_CHAIN_STATS,
    EXPLORER_DAILY_STATS, EXPLORER_SCHEMA, EXPLORER_SHIELDED_TRANSACTION_BY_CLASS_LOC,
    EXPLORER_TRANSACTION_BY_KIND_LOC, EXPLORER_TRANSACTION_META_BY_LOC,
};
pub use refill_transaction_amounts::{
    refill_transaction_amounts, RefillTransactionAmountsError, RefillTransactionAmountsOptions,
    RefillTransactionAmountsSummary,
};
pub(crate) use transactions::*;

/// Explorer facts carried between the existing transparent and chain commit phases.
///
/// Keeping this feature-only context in the explorer package lets the core write methods retain
/// their original return values while reusing facts they have already calculated.
pub struct ExplorerBlockCommitContext {
    funded_transparent_address_count: u64,
}

impl ExplorerBlockCommitContext {
    pub(crate) fn new(db: &ZakuraDb) -> Self {
        Self {
            funded_transparent_address_count: db
                .explorer_chain_stats()
                .funded_transparent_address_count,
        }
    }

    pub(crate) fn set_funded_transparent_address_count(&mut self, count: u64) {
        self.funded_transparent_address_count = count;
    }

    pub(crate) fn funded_transparent_address_count(&self) -> u64 {
        self.funded_transparent_address_count
    }
}
