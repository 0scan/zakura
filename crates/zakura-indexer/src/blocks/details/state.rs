//! Bounded reads of canonical block context from the in-process state service.

use std::{collections::HashMap, sync::Arc, time::Duration};

use tokio::time;
use tower::ServiceExt;
use zakura_chain::{
    block::{Block, Hash, Height},
    block_info::BlockInfo,
    ironwood, orchard,
    parameters::{Network, NetworkUpgrade},
    sapling,
    transparent::{OutPoint, Utxo},
};
use zakura_state::{
    ExplorerReadRequest, ExplorerReadResponse, ReadRequest, ReadResponse, ReadState,
};

use crate::Error;

const STATE_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) struct Details {
    pub(super) next_block_hash: Option<Hash>,
    pub(super) confirmations: u32,
    pub(super) finalized: bool,
    pub(super) sapling_tree: Arc<sapling::tree::NoteCommitmentTree>,
    pub(super) orchard_tree: Arc<orchard::tree::NoteCommitmentTree>,
    pub(super) ironwood_tree: Option<Arc<ironwood::tree::NoteCommitmentTree>>,
    pub(super) current_block_info: Option<BlockInfo>,
    pub(super) previous_block_info: Option<BlockInfo>,
}

pub(super) async fn load<State>(
    read_state: State,
    network: &Network,
    hash: Hash,
    height: Height,
    previous_hash: Hash,
) -> Result<Details, Error>
where
    State: ReadState,
{
    let (
        header_response,
        depth_response,
        finalized_tip_response,
        sapling_response,
        orchard_response,
        current_info_response,
        previous_info_response,
    ) = tokio::try_join!(
        call(read_state.clone(), ReadRequest::BlockHeader(hash.into())),
        call(read_state.clone(), ReadRequest::Depth(hash)),
        call(read_state.clone(), ReadRequest::FinalizedTip),
        call(read_state.clone(), ReadRequest::SaplingTree(hash.into())),
        call(read_state.clone(), ReadRequest::OrchardTree(hash.into())),
        call(read_state.clone(), ReadRequest::BlockInfo(hash.into())),
        call(
            read_state.clone(),
            ReadRequest::BlockInfo(previous_hash.into())
        ),
    )?;

    let ReadResponse::BlockHeader {
        hash: returned_hash,
        height: returned_height,
        next_block_hash,
        ..
    } = header_response
    else {
        return Err(unexpected_response("BlockHeader"));
    };
    if returned_hash != hash || returned_height != height {
        return Err(Error::StateResponse(
            "BlockHeader changed while block details were being assembled".to_string(),
        ));
    }

    let ReadResponse::Depth(Some(depth)) = depth_response else {
        return Err(Error::StateResponse(
            "canonical block left the best chain while details were being assembled".to_string(),
        ));
    };
    let confirmations = depth
        .checked_add(1)
        .ok_or_else(|| Error::Calculation("block confirmation count exceeds u32".to_string()))?;

    let ReadResponse::FinalizedTip(finalized_tip) = finalized_tip_response else {
        return Err(unexpected_response("FinalizedTip"));
    };
    let finalized = finalized_tip.is_some_and(|(finalized_height, _)| height <= finalized_height);

    let ReadResponse::SaplingTree(Some(sapling_tree)) = sapling_response else {
        return Err(Error::StateResponse(
            "state returned no Sapling tree for a canonical block".to_string(),
        ));
    };
    let ReadResponse::OrchardTree(Some(orchard_tree)) = orchard_response else {
        return Err(Error::StateResponse(
            "state returned no Orchard tree for a canonical block".to_string(),
        ));
    };
    let ReadResponse::BlockInfo(current_block_info) = current_info_response else {
        return Err(unexpected_response("BlockInfo"));
    };
    let ReadResponse::BlockInfo(previous_block_info) = previous_info_response else {
        return Err(unexpected_response("BlockInfo"));
    };

    let ironwood_active = NetworkUpgrade::Nu6_3
        .activation_height(network)
        .is_some_and(|activation_height| height >= activation_height);
    let ironwood_tree = if ironwood_active {
        let response = call(read_state, ReadRequest::IronwoodTree(hash.into())).await?;
        let ReadResponse::IronwoodTree(Some(tree)) = response else {
            return Err(Error::StateResponse(
                "state returned no Ironwood tree for an NU6.3 block".to_string(),
            ));
        };
        Some(tree)
    } else {
        None
    };

    Ok(Details {
        next_block_hash,
        confirmations,
        finalized,
        sapling_tree,
        orchard_tree,
        ironwood_tree,
        current_block_info,
        previous_block_info,
    })
}

pub(super) async fn load_spent_output_statuses<State>(
    read_state: State,
    block: &Block,
) -> Result<HashMap<OutPoint, bool>, Error>
where
    State: ReadState,
{
    let outpoints = block
        .transactions
        .iter()
        .flat_map(|transaction| {
            let transaction_hash = transaction.hash();
            (0..transaction.outputs().len())
                .map(move |output_index| OutPoint::from_usize(transaction_hash, output_index))
        })
        .collect::<Vec<_>>();

    let response = call(
        read_state,
        ReadRequest::Explorer(ExplorerReadRequest::TransparentOutputSpends(
            outpoints.clone().into(),
        )),
    )
    .await?;
    let ReadResponse::Explorer(ExplorerReadResponse::TransparentOutputSpends(spent)) = response
    else {
        return Err(unexpected_response("ExplorerTransparentOutputSpends"));
    };
    Ok(outpoints.into_iter().zip(spent).collect())
}

pub(super) async fn load_transparent_inputs<State>(
    read_state: State,
    block: &Block,
) -> Result<HashMap<OutPoint, Utxo>, Error>
where
    State: ReadState,
{
    let outpoints = block
        .transactions
        .iter()
        .flat_map(|transaction| {
            transaction
                .inputs()
                .iter()
                .filter_map(|input| input.outpoint())
        })
        .collect::<Vec<_>>();
    let response = call(
        read_state,
        ReadRequest::Explorer(ExplorerReadRequest::TransparentOutputs(
            outpoints.clone().into(),
        )),
    )
    .await?;
    let ReadResponse::Explorer(ExplorerReadResponse::TransparentOutputs(outputs)) = response else {
        return Err(unexpected_response("ExplorerTransparentOutputs"));
    };
    outpoints
        .into_iter()
        .zip(outputs)
        .map(|(outpoint, output)| {
            output.map(|output| (outpoint, output)).ok_or_else(|| {
                Error::StateResponse(format!(
                    "state is missing historical transparent output {outpoint:?}"
                ))
            })
        })
        .collect()
}

/// Rechecks canonical membership after the potentially large output-status query.
pub(super) async fn confirmations<State>(read_state: State, hash: Hash) -> Result<u32, Error>
where
    State: ReadState,
{
    let response = call(read_state, ReadRequest::Depth(hash)).await?;
    let ReadResponse::Depth(Some(depth)) = response else {
        return Err(Error::StateResponse(
            "canonical block left the best chain while details were being assembled".to_string(),
        ));
    };

    depth
        .checked_add(1)
        .ok_or_else(|| Error::Calculation("block confirmation count exceeds u32".to_string()))
}

pub(super) async fn call<State>(
    read_state: State,
    request: ReadRequest,
) -> Result<ReadResponse, Error>
where
    State: ReadState,
{
    time::timeout(STATE_REQUEST_TIMEOUT, read_state.oneshot(request))
        .await
        .map_err(|_| Error::StateRequest("state request timed out".to_string()))?
        .map_err(|error| Error::StateRequest(error.to_string()))
}

pub(super) fn unexpected_response(request: &str) -> Error {
    Error::StateResponse(format!("{request} returned a different response variant"))
}
