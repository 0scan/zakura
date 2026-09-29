//! Bounded canonical-state reads used by transaction details.

use std::{collections::HashMap, sync::Arc, time::Duration};

use futures::{stream, StreamExt, TryStreamExt};
use tokio::time;
use tower::ServiceExt;
use zakura_chain::{
    block::{Hash, Height},
    transaction::{Hash as TransactionHash, Transaction},
    transparent::OutPoint,
};
use zakura_state::{MinedTx, ReadRequest, ReadResponse, ReadState};

use crate::Error;

use super::response::TransactionChainStatus;

const STATE_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const SPENT_STATUS_CONCURRENCY: usize = 32;

pub(super) async fn transaction<State>(
    read_state: State,
    txid: TransactionHash,
) -> Result<Option<MinedTx>, Error>
where
    State: ReadState,
{
    let response = call(read_state, ReadRequest::Transaction(txid)).await?;
    let ReadResponse::Transaction(transaction) = response else {
        return Err(unexpected_response("Transaction"));
    };
    Ok(transaction)
}

pub(super) async fn chain_status<State>(
    read_state: State,
    block_hash: Hash,
    height: Height,
) -> Result<TransactionChainStatus, Error>
where
    State: ReadState,
{
    let (depth_response, finalized_tip_response) = tokio::try_join!(
        call(read_state.clone(), ReadRequest::Depth(block_hash)),
        call(read_state, ReadRequest::FinalizedTip),
    )?;

    let ReadResponse::Depth(Some(depth)) = depth_response else {
        return Err(Error::TransactionNotIndexed(
            "transaction left the best chain while details were being assembled".to_string(),
        ));
    };
    let confirmations = depth.checked_add(1).ok_or_else(|| {
        Error::Calculation("transaction confirmation count exceeds u32".to_string())
    })?;
    let ReadResponse::FinalizedTip(finalized_tip) = finalized_tip_response else {
        return Err(unexpected_response("FinalizedTip"));
    };

    Ok(TransactionChainStatus {
        confirmations,
        finalized: finalized_tip.is_some_and(|(finalized_height, _)| height <= finalized_height),
    })
}

pub(super) async fn spent_output_statuses<State>(
    read_state: State,
    transaction: &Arc<Transaction>,
) -> Result<HashMap<OutPoint, bool>, Error>
where
    State: ReadState,
{
    let transaction_hash = transaction.hash();
    let outpoints = (0..transaction.outputs().len())
        .map(|output_index| OutPoint::from_usize(transaction_hash, output_index))
        .collect::<Vec<_>>();

    stream::iter(outpoints)
        .map(|outpoint| {
            let read_state = read_state.clone();
            async move {
                let response =
                    call(read_state, ReadRequest::IsTransparentOutputSpent(outpoint)).await?;
                let ReadResponse::IsTransparentOutputSpent(spent) = response else {
                    return Err(unexpected_response("IsTransparentOutputSpent"));
                };
                Ok((outpoint, spent))
            }
        })
        .buffer_unordered(SPENT_STATUS_CONCURRENCY)
        .try_collect()
        .await
}

async fn call<State>(read_state: State, request: ReadRequest) -> Result<ReadResponse, Error>
where
    State: ReadState,
{
    time::timeout(STATE_REQUEST_TIMEOUT, read_state.oneshot(request))
        .await
        .map_err(|_| Error::StateRequest("state request timed out".to_string()))?
        .map_err(|error| Error::StateRequest(error.to_string()))
}

fn unexpected_response(request: &str) -> Error {
    Error::StateResponse(format!("{request} returned a different response variant"))
}
