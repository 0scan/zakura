//! Public data contracts returned by indexer queries.

mod block;
mod block_page;

pub use block::BlockRecord;
pub use block_page::{BlocksPagination, BlocksResponse};
