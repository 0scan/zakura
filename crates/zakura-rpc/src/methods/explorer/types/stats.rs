//! Explorer indexer status and P0 network overview responses.

use schemars::JsonSchema;
use serde::Serialize;
use zakura_indexer::{ChainTotals, RollingDayStats};

/// Progress of the rebuildable explorer index relative to the node state tip.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct IndexerStatusResponse {
    /// Current canonical state tip height.
    pub chain_height: Option<String>,
    /// Current canonical state tip hash.
    pub chain_block_hash: Option<String>,
    /// Highest canonical height covered by the explorer index.
    pub indexed_height: Option<String>,
    /// Canonical hash at `indexed_height`.
    pub indexed_block_hash: Option<String>,
    /// Number of canonical blocks not yet indexed.
    pub lag: String,
    /// Indexed canonical block percentage, formatted with four decimal places.
    pub sync_progress: String,
    /// True only when indexed height and hash equal the state tip.
    pub synced: bool,
}

/// Current mining data supplied by the node.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct MiningStats {
    /// Current proof-of-work difficulty.
    pub difficulty: String,
    /// Estimated network solution rate in solutions per second.
    pub network_solps: String,
    /// Current total block subsidy in zatoshis, excluding fees.
    ///
    /// This is unavailable at checkpointed heights where `getblocksubsidy`
    /// cannot calculate the historical founders' reward.
    pub block_reward_zat: Option<String>,
    /// Current miner subsidy in zatoshis, excluding transaction fees.
    pub miner_reward_zat: Option<String>,
    /// Current founders' reward in zatoshis.
    pub founders_reward_zat: Option<String>,
    /// Current non-deferred funding stream total in zatoshis.
    pub funding_streams_zat: Option<String>,
    /// Current deferred funding stream total in zatoshis.
    pub lockbox_zat: Option<String>,
    /// Consensus target block interval at the current chain tip.
    pub target_block_time_seconds: Option<u64>,
}

/// Current node network information.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct NetworkStats {
    /// Recently live peer count visible to this node.
    pub peer_count: String,
    /// Current wire protocol version.
    pub protocol_version: u32,
    /// Node user-agent string.
    pub node_version: String,
}

/// Current mempool resource usage.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct MempoolStats {
    /// Number of pending transactions.
    pub transaction_count: String,
    /// Sum of serialized pending transaction sizes.
    pub bytes: String,
    /// Approximate in-memory usage in bytes.
    pub memory_usage: String,
}

/// One current consensus value-pool balance.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct SupplyPoolStats {
    /// Stable pool identifier such as `transparent`, `sapling`, or `orchard`.
    pub id: String,
    /// Current balance in zatoshis.
    pub balance_zat: String,
    /// Whether the state currently monitors this pool.
    pub monitored: bool,
}

/// Current chain supply and pool distribution.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct SupplyStats {
    /// Total chain supply in zatoshis.
    pub chain_supply_zat: String,
    /// Transparent, shielded, and lockbox pool balances.
    pub pools: Vec<SupplyPoolStats>,
}

/// Node state storage and verification information.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct BlockchainRuntimeStats {
    /// On-disk state size reported by the state service.
    pub state_size_bytes: String,
    /// Node verification progress, formatted with six decimal places.
    pub verification_progress: String,
    /// Whether historical block bodies are subject to pruning.
    pub pruned: bool,
}

/// P0 explorer overview composed from indexer aggregates and live node services.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct ExplorerNetworkStatsResponse {
    /// Index catch-up progress.
    pub indexer: IndexerStatusResponse,
    /// All-time indexed canonical totals.
    pub totals: ChainTotals,
    /// Trailing activity ending at the indexed tip timestamp.
    pub trailing_24h: RollingDayStats,
    /// Current mining data.
    pub mining: MiningStats,
    /// Current peer and protocol data.
    pub network: NetworkStats,
    /// Current mempool data.
    pub mempool: MempoolStats,
    /// Current chain supply data.
    pub supply: SupplyStats,
    /// Current node state data.
    pub blockchain: BlockchainRuntimeStats,
    /// Response creation time as Unix seconds.
    pub generated_at: String,
}
