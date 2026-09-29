//! Transparent address summaries, ordered history, and cursor pagination.

mod cursor;
mod disk_format;
mod query;
mod write;

pub(crate) use write::PendingAddressRecords;
