//! Bounded canonical-state reads used by transaction details.

use std::{collections::HashMap, sync::Arc, time::Duration};

use tokio::time;
use tower::ServiceExt;
use zakura_chain::{
    block::{Hash, Height},
    transaction::{Hash as TransactionHash, Transaction},
    transparent::{OutPoint, Utxo},
};
use zakura_state::{
    ExplorerReadRequest, ExplorerReadResponse, MinedTx, ReadRequest, ReadResponse, ReadState,
};

use crate::Error;

use super::response::TransactionChainStatus;

const STATE_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

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

pub(super) async fn transaction_summary<State>(
    read_state: State,
    txid: TransactionHash,
) -> Result<Option<zakura_state::ExplorerTransactionSummary>, Error>
where
    State: ReadState,
{
    let response = call(
        read_state,
        ReadRequest::Explorer(ExplorerReadRequest::TransactionSummary(txid)),
    )
    .await?;
    let ReadResponse::Explorer(ExplorerReadResponse::TransactionSummary(summary)) = response else {
        return Err(unexpected_response("ExplorerTransactionSummary"));
    };
    Ok(summary)
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
        return Err(Error::ExplorerDataUnavailable(
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

pub(super) async fn transparent_inputs<State>(
    read_state: State,
    transaction: &Transaction,
) -> Result<HashMap<OutPoint, Utxo>, Error>
where
    State: ReadState,
{
    let outpoints = transaction
        .inputs()
        .iter()
        .filter_map(|input| input.outpoint())
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
