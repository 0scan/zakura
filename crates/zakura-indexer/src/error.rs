//! Error contract shared by indexer storage, synchronization, and queries.

/// Errors returned by indexer operations.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// RocksDB could not complete an operation.
    #[error("indexer database error: {0}")]
    Database(#[from] rocksdb::Error),

    /// An ephemeral database directory could not be created.
    #[error("indexer temporary directory error: {0}")]
    TemporaryDirectory(#[from] std::io::Error),

    /// A stored index record could not be encoded or decoded.
    #[error("indexer record error: {0}")]
    Record(#[from] serde_json::Error),

    /// The database was created by an incompatible indexer format.
    #[error("indexer database format mismatch: expected {expected}, found {actual:?}")]
    DatabaseFormat {
        /// Format version supported by this binary.
        expected: u64,
        /// Raw version bytes stored in the database.
        actual: Vec<u8>,
    },

    /// Durable index data violated its encoding or consistency contract.
    #[error("corrupt indexer data: {0}")]
    CorruptData(String),

    /// A page cursor was malformed or no longer points to the canonical chain.
    #[error("invalid indexer page cursor: {0}")]
    InvalidCursor(String),

    /// State has the block, but the explorer index has not caught up to it yet.
    #[error("block is not available in the explorer index yet: {0}")]
    BlockNotIndexed(String),

    /// A consensus-serialized transparent output was invalid.
    #[error("indexer transparent output error: {0}")]
    TransparentOutput(String),

    /// An indexed value could not be derived without violating an invariant.
    #[error("indexer calculation error: {0}")]
    Calculation(String),

    /// The state service did not return the response expected by the indexer.
    #[error("indexer state response error: {0}")]
    StateResponse(String),

    /// A state request failed or timed out.
    #[error("indexer state request error: {0}")]
    StateRequest(String),

    /// A blocking indexing task failed.
    #[error("indexer task failed: {0}")]
    Task(String),
}
