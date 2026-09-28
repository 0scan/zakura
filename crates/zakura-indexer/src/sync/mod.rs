//! In-process synchronization workers for each index domain.

mod blocks;

pub use blocks::spawn_block_sync;
