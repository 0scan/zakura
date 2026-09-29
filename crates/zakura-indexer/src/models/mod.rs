//! Authoritative records persisted in the rebuildable indexer database.

mod block;
mod transaction;

pub(crate) use block::IndexedBlockRecord;
pub(crate) use transaction::{TransactionPosition, TransactionRecord};
