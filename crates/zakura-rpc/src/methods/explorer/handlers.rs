//! Explorer JSON-RPC adapters for [`RpcImpl`].

#[cfg(feature = "indexer")]
use chrono::Utc;
use jsonrpsee::core::RpcResult as Result;
#[cfg(feature = "indexer")]
use zakura_chain::{
    block::{self, Height},
    parameters::NetworkUpgrade,
    transparent::Address,
};
use zakura_chain::{chain_sync_status::ChainSyncStatus, chain_tip::ChainTip, transaction};
use zakura_consensus::router::service_trait::BlockVerifierService;
#[cfg(feature = "indexer")]
use zakura_indexer::{
    address_summary_from_state, address_transactions_page_from_state,
    address_utxos_page_from_state, block_details_from_state, blocks_page_from_state,
    chart_data_from_state, stats_from_state, top_balances_from_state,
    transaction_details_from_state, transactions_page_from_state,
};
use zakura_network::address_book_peers::AddressBookPeers;
use zakura_node_services::mempool::{self as node_mempool, MempoolService};
#[cfg(feature = "indexer")]
use zakura_state::HashOrHeight;
use zakura_state::{ReadState as ReadStateService, State as StateService};

use crate::server::{self, error::MapError};

#[cfg(feature = "indexer")]
use super::types::{
    BlockchainRuntimeStats, MempoolStats, MiningStats, NetworkStats, NodeSyncStats,
    SupplyPoolStats, SupplyStats,
};
use super::{
    mempool,
    types::{
        AddressSummary, AddressTransactionsResponse, AddressUtxosResponse, BlockDetails,
        BlocksResponse, ChartDataRequest, ChartDataResponse, ExplorerNetworkStatsResponse,
        GetAddressTransactionsRequest, GetAddressUtxosPageRequest, GetBlocksRequest,
        GetMempoolTransactionsRequest, GetTransactionsRequest, IndexerStatusResponse,
        MempoolTransactionsResponse, TopBalancesRequest, TopBalancesResponse,
        TransactionDetailsResponse, TransactionsResponse,
    },
};
#[cfg(feature = "indexer")]
use crate::methods::RpcServer;
use crate::methods::{call_service, RpcImpl};

impl<Mempool, State, ReadState, Tip, AddressBook, BlockVerifierRouter, SyncStatus>
    RpcImpl<Mempool, State, ReadState, Tip, AddressBook, BlockVerifierRouter, SyncStatus>
where
    Mempool: MempoolService,
    State: StateService,
    ReadState: ReadStateService,
    Tip: ChainTip + Clone + Send + Sync + 'static,
    AddressBook: AddressBookPeers + Clone + Send + Sync + 'static,
    BlockVerifierRouter: BlockVerifierService,
    SyncStatus: ChainSyncStatus + Clone + Send + Sync + 'static,
{
    pub(in crate::methods) async fn explorer_get_blocks(
        &self,
        request: Option<GetBlocksRequest>,
    ) -> Result<BlocksResponse> {
        #[cfg(not(feature = "indexer"))]
        {
            let _ = request;
            return explorer_index_disabled();
        }
        #[cfg(feature = "indexer")]
        {
            let request = request.unwrap_or_default();

            match blocks_page_from_state(
                self.read_state.clone(),
                &self.network,
                request.limit,
                request.cursor,
                request.direction,
            )
            .await
            {
                Ok(response) => Ok(response),
                Err(error @ zakura_indexer::Error::InvalidCursor(_)) => {
                    Err(error).map_error(server::error::LegacyCode::InvalidParameter)
                }
                Err(error) => Err(error).map_misc_error(),
            }
        }
    }

    pub(in crate::methods) async fn explorer_get_block_details(
        &self,
        hash_or_height: String,
    ) -> Result<BlockDetails> {
        #[cfg(not(feature = "indexer"))]
        {
            let _ = hash_or_height;
            return explorer_index_disabled();
        }
        #[cfg(feature = "indexer")]
        {
            let identifier =
                HashOrHeight::new(&hash_or_height, self.latest_chain_tip.best_tip_height())
                    .map_error(server::error::LegacyCode::InvalidParameter)?;

            match block_details_from_state(self.read_state.clone(), &self.network, identifier).await
            {
                Ok(Some(details)) => Ok(details),
                Ok(None) => Err("Block not found in the best chain")
                    .map_error(server::error::LegacyCode::InvalidAddressOrKey),
                Err(error) => Err(error).map_misc_error(),
            }
        }
    }

    pub(in crate::methods) async fn explorer_get_transactions(
        &self,
        request: Option<GetTransactionsRequest>,
    ) -> Result<TransactionsResponse> {
        #[cfg(not(feature = "indexer"))]
        {
            let _ = request;
            return explorer_index_disabled();
        }
        #[cfg(feature = "indexer")]
        {
            let request = request.unwrap_or_default();
            let query = request
                .transaction_query()
                .map_error(server::error::LegacyCode::InvalidParameter)?;
            let height_range = request.height_range();

            match transactions_page_from_state(
                self.read_state.clone(),
                query,
                request.limit,
                request.cursor,
                request.direction,
                height_range,
            )
            .await
            {
                Ok(response) => Ok(response),
                Err(
                    error @ (zakura_indexer::Error::InvalidCursor(_)
                    | zakura_indexer::Error::InvalidQuery(_)),
                ) => Err(error).map_error(server::error::LegacyCode::InvalidParameter),
                Err(error) => Err(error).map_misc_error(),
            }
        }
    }

    pub(in crate::methods) async fn explorer_get_mempool_transactions(
        &self,
        request: Option<GetMempoolTransactionsRequest>,
    ) -> Result<MempoolTransactionsResponse> {
        let response = call_service(
            self.mempool.clone(),
            node_mempool::Request::FullTransactions,
        )
        .await?;
        let node_mempool::Response::FullTransactions {
            transactions,
            transaction_dependencies,
            last_seen_tip_hash: _,
        } = response
        else {
            unreachable!("unmatched response to a mempool FullTransactions request")
        };
        let request = request.unwrap_or_default();

        match mempool::transactions_page(
            &self.network,
            transactions,
            &transaction_dependencies,
            &request,
        ) {
            Ok(response) => Ok(response),
            Err(
                error @ (zakura_indexer::Error::InvalidCursor(_)
                | zakura_indexer::Error::InvalidQuery(_)),
            ) => Err(error).map_error(server::error::LegacyCode::InvalidParameter),
            Err(error) => Err(error).map_misc_error(),
        }
    }

    pub(in crate::methods) async fn explorer_get_transaction_details(
        &self,
        txid: String,
    ) -> Result<TransactionDetailsResponse> {
        let txid = txid
            .parse::<transaction::Hash>()
            .map_error(server::error::LegacyCode::InvalidParameter)?;

        match call_service(
            self.mempool.clone(),
            node_mempool::Request::FullTransactions,
        )
        .await
        {
            Ok(node_mempool::Response::FullTransactions {
                transactions,
                transaction_dependencies,
                last_seen_tip_hash: _,
            }) => {
                if let Some(transaction) = transactions
                    .iter()
                    .find(|transaction| transaction.transaction.id().mined_id() == txid)
                {
                    return mempool::transaction_details(
                        transaction,
                        &transactions,
                        &transaction_dependencies,
                        &self.network,
                    )
                    .map(TransactionDetailsResponse::Pending)
                    .map_misc_error();
                }
            }
            Ok(_) => unreachable!("unmatched response to a mempool FullTransactions request"),
            Err(error) => {
                tracing::debug!(?error, %txid, "mempool lookup failed; checking canonical index");
            }
        }

        #[cfg(not(feature = "indexer"))]
        return explorer_index_disabled();

        #[cfg(feature = "indexer")]
        match transaction_details_from_state(self.read_state.clone(), &self.network, txid).await {
            Ok(Some(details)) => Ok(TransactionDetailsResponse::Mined(details)),
            Ok(None) => Err("Transaction not found in the best chain")
                .map_error(server::error::LegacyCode::InvalidAddressOrKey),
            Err(error @ zakura_indexer::Error::ExplorerDataUnavailable(_)) => {
                Err(error).map_error(server::error::LegacyCode::InWarmup)
            }
            Err(error) => Err(error).map_misc_error(),
        }
    }

    pub(in crate::methods) async fn explorer_get_address_summary(
        &self,
        address: String,
    ) -> Result<AddressSummary> {
        #[cfg(not(feature = "indexer"))]
        {
            let _ = address;
            return explorer_index_disabled();
        }
        #[cfg(feature = "indexer")]
        {
            let address = explorer_transparent_address(&self.network, &address)
                .map_error(server::error::LegacyCode::InvalidAddressOrKey)?;

            address_summary_from_state(self.read_state.clone(), &self.network, address)
                .await
                .map_misc_error()
        }
    }

    pub(in crate::methods) async fn explorer_get_address_transactions(
        &self,
        request: GetAddressTransactionsRequest,
    ) -> Result<AddressTransactionsResponse> {
        #[cfg(not(feature = "indexer"))]
        {
            let _ = request;
            return explorer_index_disabled();
        }
        #[cfg(feature = "indexer")]
        {
            let address = explorer_transparent_address(&self.network, &request.address)
                .map_error(server::error::LegacyCode::InvalidAddressOrKey)?;
            let height_range = request.height_range();

            match address_transactions_page_from_state(
                self.read_state.clone(),
                &self.network,
                address,
                request.limit,
                request.cursor,
                request.direction,
                height_range,
            )
            .await
            {
                Ok(response) => Ok(response),
                Err(
                    error @ (zakura_indexer::Error::InvalidCursor(_)
                    | zakura_indexer::Error::InvalidQuery(_)),
                ) => Err(error).map_error(server::error::LegacyCode::InvalidParameter),
                Err(error) => Err(error).map_misc_error(),
            }
        }
    }

    pub(in crate::methods) async fn explorer_get_address_utxos_page(
        &self,
        request: GetAddressUtxosPageRequest,
    ) -> Result<AddressUtxosResponse> {
        #[cfg(not(feature = "indexer"))]
        {
            let _ = request;
            return explorer_index_disabled();
        }
        #[cfg(feature = "indexer")]
        {
            let address = explorer_transparent_address(&self.network, &request.address)
                .map_error(server::error::LegacyCode::InvalidAddressOrKey)?;

            match address_utxos_page_from_state(
                self.read_state.clone(),
                address,
                request.limit,
                request.cursor,
                request.direction,
            )
            .await
            {
                Ok(response) => Ok(response),
                Err(error @ zakura_indexer::Error::InvalidCursor(_)) => {
                    Err(error).map_error(server::error::LegacyCode::InvalidParameter)
                }
                Err(error) => Err(error).map_misc_error(),
            }
        }
    }

    pub(in crate::methods) async fn explorer_get_indexer_status(
        &self,
    ) -> Result<IndexerStatusResponse> {
        #[cfg(not(feature = "indexer"))]
        return explorer_index_disabled();

        #[cfg(feature = "indexer")]
        {
            let chain_tip = self.latest_chain_tip.best_tip_height_and_hash();
            let indexer_stats = stats_from_state(self.read_state.clone())
                .await
                .map_misc_error()?;
            Ok(indexer_status(
                chain_tip,
                indexer_stats.indexed_height.as_deref(),
                indexer_stats.indexed_block_hash.as_deref(),
            ))
        }
    }

    pub(in crate::methods) async fn explorer_get_network_stats(
        &self,
    ) -> Result<ExplorerNetworkStatsResponse> {
        #[cfg(not(feature = "indexer"))]
        return explorer_index_disabled();

        #[cfg(feature = "indexer")]
        {
            let chain_tip = self.latest_chain_tip.best_tip_height_and_hash();
            let indexer_stats = stats_from_state(self.read_state.clone())
                .await
                .map_misc_error()?;

            let (blockchain, network_solps, mempool, subsidy) = tokio::join!(
                self.get_blockchain_info(),
                self.get_network_sol_ps(None, None),
                self.get_mempool_info(),
                self.get_block_subsidy(None),
            );
            let blockchain = blockchain?;
            let network_solps = network_solps?;
            let mempool = mempool?;
            let subsidy = subsidy.ok();
            let estimated_network_height = blockchain.estimated_height();
            let node_height = chain_tip.map(|(height, _)| height);
            let sync = NodeSyncStats {
                estimated_network_height: estimated_network_height.0.to_string(),
                node_height: node_height.map(|height| height.0.to_string()),
                lag: node_height.map(|height| {
                    estimated_network_height
                        .0
                        .saturating_sub(height.0)
                        .to_string()
                }),
                verification_progress: format!("{:.6}", blockchain.verification_progress()),
                synced: self
                    .latest_chain_tip
                    .is_at_or_near_network_tip(&self.network),
            };
            let target_block_time_seconds = chain_tip.as_ref().and_then(|(height, _)| {
                u64::try_from(
                    NetworkUpgrade::target_spacing_for_height(&self.network, *height).num_seconds(),
                )
                .ok()
            });
            let supply = SupplyStats {
                chain_supply_zat: blockchain
                    .chain_supply()
                    .chain_value_zat()
                    .zatoshis()
                    .to_string(),
                pools: blockchain
                    .value_pools()
                    .iter()
                    .map(|pool| SupplyPoolStats {
                        id: pool.id().clone(),
                        balance_zat: pool.chain_value_zat().zatoshis().to_string(),
                        monitored: pool.monitored(),
                    })
                    .collect(),
            };
            let response = ExplorerNetworkStatsResponse {
                sync,
                totals: indexer_stats.totals,
                trailing_24h: indexer_stats.trailing_24h,
                mining: MiningStats {
                    difficulty: format!("{:.6}", blockchain.difficulty()),
                    network_solps: network_solps.to_string(),
                    block_reward_zat: subsidy
                        .as_ref()
                        .map(|subsidy| subsidy.total_block_subsidy().zatoshis().to_string()),
                    miner_reward_zat: subsidy
                        .as_ref()
                        .map(|subsidy| subsidy.miner().zatoshis().to_string()),
                    founders_reward_zat: subsidy
                        .as_ref()
                        .map(|subsidy| subsidy.founders().zatoshis().to_string()),
                    funding_streams_zat: subsidy
                        .as_ref()
                        .map(|subsidy| subsidy.funding_streams_total().zatoshis().to_string()),
                    lockbox_zat: subsidy
                        .as_ref()
                        .map(|subsidy| subsidy.lockbox_total().zatoshis().to_string()),
                    target_block_time_seconds,
                },
                network: NetworkStats {
                    peer_count: self
                        .address_book
                        .recently_live_peers(Utc::now())
                        .len()
                        .to_string(),
                    protocol_version: zakura_network::constants::CURRENT_NETWORK_PROTOCOL_VERSION.0,
                    node_version: self.user_agent.clone(),
                },
                mempool: MempoolStats {
                    transaction_count: mempool.size.to_string(),
                    bytes: mempool.bytes.to_string(),
                    memory_usage: mempool.usage.to_string(),
                },
                supply,
                blockchain: BlockchainRuntimeStats {
                    state_size_bytes: blockchain.size_on_disk().to_string(),
                    pruned: blockchain.pruned(),
                },
                generated_at: Utc::now().timestamp().to_string(),
            };

            Ok(response)
        }
    }

    pub(in crate::methods) async fn explorer_get_chart_data(
        &self,
        request: ChartDataRequest,
    ) -> Result<ChartDataResponse> {
        #[cfg(not(feature = "indexer"))]
        {
            let _ = request;
            return explorer_index_disabled();
        }
        #[cfg(feature = "indexer")]
        {
            match chart_data_from_state(self.read_state.clone(), &self.network, request).await {
                Ok(response) => Ok(response),
                Err(error @ zakura_indexer::Error::InvalidQuery(_)) => {
                    Err(error).map_error(server::error::LegacyCode::InvalidParameter)
                }
                Err(error) => Err(error).map_misc_error(),
            }
        }
    }

    pub(in crate::methods) async fn explorer_get_top_balances(
        &self,
        request: TopBalancesRequest,
    ) -> Result<TopBalancesResponse> {
        #[cfg(not(feature = "indexer"))]
        {
            let _ = request;
            return explorer_index_disabled();
        }
        #[cfg(feature = "indexer")]
        {
            match top_balances_from_state(self.read_state.clone(), request).await {
                Ok(response) => Ok(response),
                Err(error @ zakura_indexer::Error::InvalidCursor(_))
                | Err(error @ zakura_indexer::Error::InvalidQuery(_)) => {
                    Err(error).map_error(server::error::LegacyCode::InvalidParameter)
                }
                Err(error) => Err(error).map_misc_error(),
            }
        }
    }
}

#[cfg(not(feature = "indexer"))]
fn explorer_index_disabled<T>() -> Result<T> {
    Err("explorer state index is not enabled in this zakurad process").map_misc_error()
}

#[cfg(feature = "indexer")]
fn explorer_transparent_address(
    network: &zakura_chain::parameters::Network,
    encoded: &str,
) -> std::result::Result<Address, String> {
    let address = encoded
        .parse::<Address>()
        .map_err(|_| "invalid transparent address".to_string())?;
    if address.network_kind() != network.kind() {
        return Err("transparent address belongs to a different network".to_string());
    }
    if matches!(address, Address::Tex { .. }) {
        return Err(
            "TEX addresses do not identify the receiving address stored on-chain".to_string(),
        );
    }

    Ok(address)
}

#[cfg(feature = "indexer")]
fn indexer_status(
    chain_tip: Option<(Height, block::Hash)>,
    indexed_height: Option<&str>,
    indexed_block_hash: Option<&str>,
) -> IndexerStatusResponse {
    let indexed_height_value = indexed_height.map(|height| {
        height
            .parse::<u64>()
            .expect("indexer heights are generated from valid u32 values")
    });
    let chain_height = chain_tip.map(|(height, _)| u64::from(height.0));
    let indexed_block_count = indexed_height_value.map_or(0, |height| height.saturating_add(1));
    let chain_block_count = chain_height.map_or(0, |height| height.saturating_add(1));
    let lag = chain_block_count.saturating_sub(indexed_block_count);
    let empty_chain_progress = if indexed_block_count == 0 {
        1_000_000
    } else {
        0
    };
    let progress_units = indexed_block_count
        .min(chain_block_count)
        .saturating_mul(1_000_000)
        .checked_div(chain_block_count)
        .unwrap_or(empty_chain_progress);
    let synced = match (chain_tip, indexed_height_value.as_ref()) {
        (None, None) => true,
        (Some((height, hash)), Some(indexed_height)) => {
            let chain_hash = hash.to_string();
            u64::from(height.0) == *indexed_height
                && indexed_block_hash == Some(chain_hash.as_str())
        }
        (None, Some(_)) | (Some(_), None) => false,
    };

    IndexerStatusResponse {
        chain_height: chain_tip.map(|(height, _)| height.0.to_string()),
        chain_block_hash: chain_tip.map(|(_, hash)| hash.to_string()),
        indexed_height: indexed_height.map(ToOwned::to_owned),
        indexed_block_hash: indexed_block_hash.map(ToOwned::to_owned),
        lag: lag.to_string(),
        sync_progress: format!("{}.{:04}", progress_units / 10_000, progress_units % 10_000),
        synced,
    }
}

#[cfg(all(test, feature = "indexer"))]
mod tests {
    use zakura_chain::block::{Hash, Height};

    use super::indexer_status;

    #[test]
    fn indexer_status_compares_height_and_hash() {
        let hash = Hash([0x51; 32]);
        let encoded_hash = hash.to_string();

        let synced = indexer_status(Some((Height(10), hash)), Some("10"), Some(&encoded_hash));
        assert!(synced.synced);
        assert_eq!(synced.lag, "0");
        assert_eq!(synced.sync_progress, "100.0000");

        let lagged = indexer_status(Some((Height(10), hash)), Some("9"), Some(&encoded_hash));
        assert!(!lagged.synced);
        assert_eq!(lagged.lag, "1");
        assert_eq!(lagged.sync_progress, "90.9090");

        let empty = indexer_status(None, None, None);
        assert!(empty.synced);
        assert_eq!(empty.sync_progress, "100.0000");
    }
}
