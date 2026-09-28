//! Explorer HTTP API configuration.

use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

/// Configuration for the read-only explorer REST API.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    /// IP address and port for the explorer REST API.
    ///
    /// The API is disabled when this value is absent. For example:
    ///
    /// ```toml
    /// [explorer]
    /// listen_addr = "127.0.0.1:8235"
    /// ```
    ///
    /// This endpoint is unauthenticated and exposes public blockchain data.
    /// Bind to loopback when placing it behind a reverse proxy.
    pub listen_addr: Option<SocketAddr>,
}
