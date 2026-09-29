//! Error contract shared by explorer response adapters.

/// Errors returned while reading or deriving explorer data.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Canonical state data violated an explorer consistency contract.
    #[error("inconsistent explorer data: {0}")]
    CorruptData(String),

    /// A page cursor was malformed or no longer points to the canonical chain.
    #[error("invalid explorer page cursor: {0}")]
    InvalidCursor(String),

    /// A transaction query combined incompatible or unsupported filters.
    #[error("invalid explorer query: {0}")]
    InvalidQuery(String),

    /// Explorer metadata became unavailable while assembling a canonical response.
    #[error("explorer metadata is temporarily unavailable: {0}")]
    ExplorerDataUnavailable(String),

    /// A response value could not be derived without violating an invariant.
    #[error("explorer calculation error: {0}")]
    Calculation(String),

    /// The state service did not return the response expected by the explorer.
    #[error("explorer state response error: {0}")]
    StateResponse(String),

    /// A state request failed or timed out.
    #[error("explorer state request error: {0}")]
    StateRequest(String),

    /// A blocking explorer response task failed.
    #[error("explorer task failed: {0}")]
    Task(String),
}
