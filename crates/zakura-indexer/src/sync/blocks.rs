//! Block backfill and canonical-chain reconciliation through the state service.

use std::time::Duration;

use futures::{stream, StreamExt, TryStreamExt};
use tokio::{task::JoinHandle, time};
use tower::ServiceExt;
use tracing::{info, warn};
use zakura_chain::{
    block::{Hash, Height},
    chain_tip::ChainTip,
};
use zakura_state::{ReadRequest, ReadResponse, ReadState};

use crate::{Error, Indexer};

const STATE_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const RETRY_DELAY: Duration = Duration::from_secs(1);
const BLOCK_BATCH_SIZE: u32 = 128;
const BLOCK_INFO_CONCURRENCY: usize = 32;

/// Spawns the in-process block backfill and tip reconciliation worker.
pub fn spawn_block_sync<State, Tip>(
    indexer: Indexer,
    read_state: State,
    mut chain_tip: Tip,
) -> JoinHandle<()>
where
    State: ReadState,
    Tip: ChainTip + Clone + Send + Sync + 'static,
{
    tokio::spawn(async move {
        loop {
            chain_tip.mark_best_tip_seen();

            if let Err(error) = sync_blocks_to_state_tip(&indexer, read_state.clone()).await {
                warn!(?error, "block index sync attempt failed; retrying");
            }

            tokio::select! {
                changed = chain_tip.best_tip_changed() => {
                    if let Err(error) = changed {
                        info!(?error, "block index sync stopped because the chain tip channel closed");
                        break;
                    }
                }
                () = time::sleep(RETRY_DELAY) => {}
            }
        }
    })
}

async fn sync_blocks_to_state_tip<State>(indexer: &Indexer, read_state: State) -> Result<(), Error>
where
    State: ReadState,
{
    let Some((state_tip_height, state_tip_hash)) = state_tip(read_state.clone()).await? else {
        return Ok(());
    };

    reconcile_canonical_blocks(
        indexer,
        read_state.clone(),
        state_tip_height,
        state_tip_hash,
    )
    .await?;

    let mut next_height = indexer
        .indexed_block_tip()?
        .and_then(|(height, _)| height.next().ok())
        .unwrap_or(Height::MIN);

    while next_height <= state_tip_height {
        let remaining = state_tip_height.0 - next_height.0 + 1;
        let count = remaining.min(BLOCK_BATCH_SIZE);
        let response = call_state(
            read_state.clone(),
            ReadRequest::BlocksByHeightRange {
                start: next_height,
                count,
            },
        )
        .await?;
        let ReadResponse::Blocks(blocks) = response else {
            return Err(Error::StateResponse(
                "BlocksByHeightRange returned a different response variant".to_string(),
            ));
        };
        if blocks.is_empty() {
            return Err(Error::StateResponse(format!(
                "state returned no block at height {}; archive state is required for initial block indexing",
                next_height.0
            )));
        }

        let indexed_count = blocks.len();
        let blocks = stream::iter(blocks)
            .map(|(height, block, serialized_size)| {
                let read_state = read_state.clone();
                async move {
                    let response =
                        call_state(read_state, ReadRequest::BlockInfo(height.into())).await?;
                    let ReadResponse::BlockInfo(Some(info)) = response else {
                        return Err(Error::StateResponse(format!(
                            "state returned no block info at indexed height {}",
                            height.0
                        )));
                    };
                    Ok((height, block, serialized_size, *info.value_pools()))
                }
            })
            .buffered(BLOCK_INFO_CONCURRENCY)
            .try_collect::<Vec<_>>()
            .await?;

        let indexer_for_batch = indexer.clone();
        tokio::task::spawn_blocking(move || indexer_for_batch.index_blocks(blocks))
            .await
            .map_err(|error| Error::Task(error.to_string()))??;

        let indexed_count = u32::try_from(indexed_count)
            .map_err(|_| Error::Calculation("state block batch length exceeds u32".to_string()))?;
        next_height = Height(next_height.0.checked_add(indexed_count).ok_or_else(|| {
            Error::Calculation("next block index height exceeds u32".to_string())
        })?);
    }

    Ok(())
}

async fn reconcile_canonical_blocks<State>(
    indexer: &Indexer,
    read_state: State,
    state_tip_height: Height,
    state_tip_hash: Hash,
) -> Result<(), Error>
where
    State: ReadState,
{
    let Some((indexed_tip_height, indexed_tip_hash)) = indexer.indexed_block_tip()? else {
        return Ok(());
    };

    if indexed_tip_height == state_tip_height && indexed_tip_hash == state_tip_hash {
        return Ok(());
    }

    let comparison_height = Height(indexed_tip_height.0.min(state_tip_height.0));
    let indexed_hash = indexer.canonical_block_hash(comparison_height)?;
    let state_hash = state_block_hash(read_state.clone(), comparison_height).await?;
    if indexed_hash == state_hash && indexed_tip_height <= state_tip_height {
        return Ok(());
    }

    let mut candidate_height = Some(comparison_height);
    let common_ancestor = loop {
        let Some(height) = candidate_height else {
            break None;
        };
        if indexer.canonical_block_hash(height)?
            == state_block_hash(read_state.clone(), height).await?
        {
            break Some(height);
        }
        candidate_height = height.previous().ok();
    };

    let indexer_for_rollback = indexer.clone();
    tokio::task::spawn_blocking(move || indexer_for_rollback.rollback_blocks_to(common_ancestor))
        .await
        .map_err(|error| Error::Task(error.to_string()))??;

    Ok(())
}

async fn state_tip<State>(read_state: State) -> Result<Option<(Height, Hash)>, Error>
where
    State: ReadState,
{
    match call_state(read_state, ReadRequest::Tip).await? {
        ReadResponse::Tip(tip) => Ok(tip),
        _ => Err(Error::StateResponse(
            "Tip returned a different response variant".to_string(),
        )),
    }
}

async fn state_block_hash<State>(read_state: State, height: Height) -> Result<Option<Hash>, Error>
where
    State: ReadState,
{
    match call_state(read_state, ReadRequest::Block(height.into())).await? {
        ReadResponse::Block(block) => Ok(block.map(|block| block.hash())),
        _ => Err(Error::StateResponse(
            "Block returned a different response variant".to_string(),
        )),
    }
}

async fn call_state<State>(read_state: State, request: ReadRequest) -> Result<ReadResponse, Error>
where
    State: ReadState,
{
    time::timeout(STATE_REQUEST_TIMEOUT, read_state.oneshot(request))
        .await
        .map_err(|_| Error::StateRequest("state request timed out".to_string()))?
        .map_err(|error| Error::StateRequest(error.to_string()))
}
