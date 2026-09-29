//! Shared cursor-pagination request types.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Direction to move from an opaque page cursor.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PageDirection {
    /// Return records older than the cursor, or the newest records without a cursor.
    #[default]
    Next,
    /// Return the immediately newer page preceding the cursor.
    Previous,
}
