//! Canonical-chain aggregate maintenance and bounded rolling queries.

mod query;
mod write;

pub(crate) use write::BlockTransactionStats;
