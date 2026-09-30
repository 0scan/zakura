//! On-demand canonical block details assembled directly from state.

mod response;
mod state;
mod value_pools;

use zakura_chain::parameters::Network;
use zakura_state::{
    ExplorerReadRequest, ExplorerReadResponse, HashOrHeight, ReadRequest, ReadResponse, ReadState,
};

use crate::{
    blocks::query::block_record_from_state, transactions::build_block_transactions,
    types::BlockDetails, Error,
};

/// Returns complete explorer details directly from canonical state.
pub async fn block_details_from_state<State>(
    read_state: State,
    network: &Network,
    identifier: HashOrHeight,
) -> Result<Option<BlockDetails>, Error>
where
    State: ReadState,
{
    let height = match identifier {
        HashOrHeight::Height(height) => height,
        HashOrHeight::Hash(hash) => {
            let response =
                state::call(read_state.clone(), ReadRequest::BlockHeader(hash.into())).await?;
            let ReadResponse::BlockHeader { height, .. } = response else {
                return Err(state::unexpected_response("BlockHeader"));
            };
            height
        }
    };
    let response = state::call(
        read_state.clone(),
        ReadRequest::Explorer(ExplorerReadRequest::BlockSummaries(vec![height].into())),
    )
    .await?;
    let ReadResponse::Explorer(ExplorerReadResponse::BlockSummaries(mut summaries)) = response
    else {
        return Err(state::unexpected_response("ExplorerBlockSummaries"));
    };
    let Some(summary_data) = summaries.pop().flatten() else {
        return Ok(None);
    };
    let block = summary_data.block.clone();

    let hash = block.hash();
    let returned_height = block.coinbase_height().ok_or_else(|| {
        Error::StateResponse("canonical block is missing its coinbase height".to_string())
    })?;
    if returned_height != height {
        return Err(Error::StateResponse(
            "state returned a block at a different height".to_string(),
        ));
    }
    let mut state_details = state::load(
        read_state.clone(),
        network,
        hash,
        height,
        block.header.previous_block_hash,
    )
    .await?;
    let (transparent_inputs, spent_outputs) = tokio::try_join!(
        state::load_transparent_inputs(read_state.clone(), &block),
        state::load_spent_output_statuses(read_state.clone(), &block),
    )?;
    state_details.confirmations = state::confirmations(read_state, hash).await?;

    let network_for_build = network.clone();
    let block_for_index = block.clone();
    let (summary, block_transactions) = tokio::task::spawn_blocking(move || {
        let summary = block_record_from_state(&network_for_build, summary_data)?;
        let block_transactions = build_block_transactions(
            &network_for_build,
            &block_for_index,
            height,
            hash,
            &transparent_inputs,
            &spent_outputs,
        )?;
        Ok::<_, Error>((summary, block_transactions))
    })
    .await
    .map_err(|error| Error::Task(error.to_string()))??;

    Ok(Some(response::assemble(
        network,
        block,
        summary,
        height,
        state_details,
        block_transactions,
    )?))
}

#[cfg(test)]
mod tests {
    use super::block_details_from_state;
    use tower::{service_fn, BoxError};
    use zakura_chain::{
        block::{genesis::regtest_genesis_block, Height},
        block_info::BlockInfo,
        parameters::{testnet::RegtestParameters, Network},
        serialization::ZcashSerialize,
    };
    use zakura_state::{
        ExplorerBlockSummary, ExplorerReadRequest, ExplorerReadResponse, ReadRequest, ReadResponse,
    };

    #[tokio::test]
    async fn assembles_rpc_and_explorer_fields_without_an_rpc_call() {
        let network = Network::new_regtest(RegtestParameters::default());
        let block = regtest_genesis_block();
        let hash = block.hash();
        let serialized_size = block.zcash_serialized_size();
        let state_block = block.clone();
        let read_state = service_fn(move |request: ReadRequest| {
            let block = state_block.clone();
            async move {
                Ok::<_, BoxError>(match request {
                    ReadRequest::BlockAndSize(_) => {
                        ReadResponse::BlockAndSize(Some((block.clone(), serialized_size)))
                    }
                    ReadRequest::Explorer(ExplorerReadRequest::BlockSummaries(heights)) => {
                        assert_eq!(&*heights, &[Height(0)]);
                        ReadResponse::Explorer(ExplorerReadResponse::BlockSummaries(vec![Some(
                            ExplorerBlockSummary {
                                block: block.clone(),
                                serialized_size: u32::try_from(serialized_size)
                                    .expect("test block size fits in u32"),
                                total_fees_zat: 0,
                            },
                        )]))
                    }
                    ReadRequest::Explorer(ExplorerReadRequest::TransparentOutputs(outpoints)) => {
                        assert!(outpoints.is_empty());
                        ReadResponse::Explorer(ExplorerReadResponse::TransparentOutputs(Vec::new()))
                    }
                    ReadRequest::BlockHeader(_) => ReadResponse::BlockHeader {
                        header: block.header.clone(),
                        hash: block.hash(),
                        height: Height(0),
                        next_block_hash: None,
                    },
                    ReadRequest::Depth(_) => ReadResponse::Depth(Some(0)),
                    ReadRequest::FinalizedTip => {
                        ReadResponse::FinalizedTip(Some((Height(0), block.hash())))
                    }
                    ReadRequest::SaplingTree(_) => {
                        ReadResponse::SaplingTree(Some(Default::default()))
                    }
                    ReadRequest::OrchardTree(_) => {
                        ReadResponse::OrchardTree(Some(Default::default()))
                    }
                    ReadRequest::BlockInfo(_) => {
                        ReadResponse::BlockInfo(Some(BlockInfo::default()))
                    }
                    ReadRequest::Explorer(ExplorerReadRequest::TransparentOutputSpends(
                        outpoints,
                    )) => ReadResponse::Explorer(ExplorerReadResponse::TransparentOutputSpends(
                        vec![false; outpoints.len()],
                    )),
                    _ => panic!("unexpected test state request"),
                })
            }
        });

        let details = block_details_from_state(read_state, &network, Height(0).into())
            .await
            .expect("block details should be assembled")
            .expect("genesis block should exist");

        assert_eq!(details.summary.hash, hash.to_string());
        assert_eq!(
            details.summary.size,
            u32::try_from(serialized_size).expect("test block size fits in u32")
        );
        assert_eq!(details.confirmations, 1);
        assert!(details.canonical);
        assert!(details.finalized);
        assert_eq!(details.transactions.len(), block.transactions.len());
        assert!(details.transactions[0].transaction.is_coinbase);
        assert!(details.coinbase_hex.is_some());
        assert!(details.chain_supply.is_some());
        assert_eq!(details.value_pools.len(), 6);

        let json = serde_json::to_value(details).expect("block details serialize as JSON");
        assert!(json["transactions"][0].get("tx_index").is_some());
        assert!(json["transactions"][0].get("transaction_index").is_none());
    }
}
