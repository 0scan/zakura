//! Transparent address summaries, ordered history, and cursor pagination.

mod cursor;
mod disk_format;
mod query;
mod rich_list;
mod rich_list_cursor;
mod write;

pub(crate) use write::PendingAddressRecords;
