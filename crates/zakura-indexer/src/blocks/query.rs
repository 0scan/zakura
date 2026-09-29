//! Canonical block reads and stable cursor pagination.

use tower::ServiceExt;
use zakura_chain::{block::Height, parameters::Network};
use zakura_state::{ExplorerBlockSummary, ReadRequest, ReadResponse, ReadState};

use super::cursor::BlockCursor;
use crate::{
    types::{BlockRecord, BlocksPagination, BlocksResponse, PageDirection},
    Error,
};

const DEFAULT_QUERY_LIMIT: u32 = 5;
const MAX_QUERY_LIMIT: u32 = 100;

/// Returns canonical block summaries directly from the node state database.
pub async fn blocks_page_from_state<S>(
    read_state: S,
    network: &Network,
    limit: Option<u32>,
    cursor: Option<String>,
    direction: PageDirection,
) -> Result<BlocksResponse, Error>
where
    S: ReadState,
{
    let limit = limit
        .unwrap_or(DEFAULT_QUERY_LIMIT)
        .clamp(1, MAX_QUERY_LIMIT);
    let cursor = cursor
        .map(|encoded| BlockCursor::decode(&encoded))
        .transpose()?;
    if direction == PageDirection::Previous && cursor.is_none() {
        return Err(Error::InvalidCursor(
            "direction=prev requires a cursor".to_string(),
        ));
    }

    let response = read_state
        .clone()
        .oneshot(ReadRequest::Tip)
        .await
        .map_err(|error| Error::StateRequest(error.to_string()))?;
    let ReadResponse::Tip(tip) = response else {
        return Err(Error::StateResponse(
            "state returned the wrong response for a tip request".to_string(),
        ));
    };
    let Some((tip_height, _)) = tip else {
        if cursor.is_some() {
            return Err(Error::InvalidCursor(
                "cursor cannot reference an empty chain".to_string(),
            ));
        }
        return Ok(empty_response(limit, 0));
    };
    if let Some(cursor) = cursor {
        let response = read_state
            .clone()
            .oneshot(ReadRequest::BlockHeader(cursor.height.into()))
            .await
            .map_err(|_| {
                Error::InvalidCursor("cursor block is no longer on the canonical chain".to_string())
            })?;
        let ReadResponse::BlockHeader { hash, .. } = response else {
            return Err(Error::StateResponse(
                "state returned the wrong response for a block-header request".to_string(),
            ));
        };
        if hash != cursor.hash {
            return Err(Error::InvalidCursor(
                "cursor block is no longer on the canonical chain".to_string(),
            ));
        }
    }

    let start_height = match (direction, cursor) {
        (PageDirection::Next, Some(cursor)) => cursor.height.previous().ok(),
        (PageDirection::Next, None) => Some(tip_height),
        (PageDirection::Previous, Some(cursor)) => cursor
            .height
            .next()
            .ok()
            .filter(|height| *height <= tip_height),
        (PageDirection::Previous, None) => unreachable!("previous pages require a cursor"),
    };
    let Some(mut height) = start_height else {
        return Ok(empty_response(limit, u64::from(tip_height.0) + 1));
    };
    let mut heights = Vec::with_capacity(
        usize::try_from(limit).expect("the maximum explorer block page size fits in usize"),
    );
    for _ in 0..limit {
        heights.push(height);
        height = match direction {
            PageDirection::Next => match height.previous() {
                Ok(previous) => previous,
                Err(_) => break,
            },
            PageDirection::Previous => match height.next() {
                Ok(next) if next <= tip_height => next,
                Ok(_) | Err(_) => break,
            },
        };
    }

    let response = read_state
        .oneshot(ReadRequest::ExplorerBlockSummaries(heights.into()))
        .await
        .map_err(|error| Error::StateRequest(error.to_string()))?;
    let ReadResponse::ExplorerBlockSummaries(summaries) = response else {
        return Err(Error::StateResponse(
            "state returned the wrong response for explorer block summaries".to_string(),
        ));
    };
    let mut records = summaries
        .into_iter()
        .map(|summary| {
            let summary = summary.ok_or_else(|| {
                Error::StateResponse("canonical block disappeared during page read".to_string())
            })?;
            let hash = summary.block.hash();
            let height = summary.block.coinbase_height().ok_or_else(|| {
                Error::StateResponse("canonical block is missing its height".to_string())
            })?;
            Ok((
                BlockCursor::new(height, hash),
                block_record_from_state(network, summary)?,
            ))
        })
        .collect::<Result<Vec<_>, Error>>()?;
    if direction == PageDirection::Previous {
        records.reverse();
    }
    let first_position = records.first().map(|(position, _)| *position);
    let last_position = records.last().map(|(position, _)| *position);
    let has_prev = first_position.is_some_and(|position| position.height < tip_height);
    let has_next = last_position.is_some_and(|position| position.height > Height::MIN);

    Ok(BlocksResponse {
        blocks: records.into_iter().map(|(_, record)| record).collect(),
        pagination: BlocksPagination {
            limit,
            total: (u64::from(tip_height.0) + 1).to_string(),
            has_next,
            has_prev,
            next_cursor: last_position.filter(|_| has_next).map(BlockCursor::encode),
            prev_cursor: first_position.filter(|_| has_prev).map(BlockCursor::encode),
        },
    })
}

pub(crate) fn block_record_from_state(
    network: &Network,
    summary: ExplorerBlockSummary,
) -> Result<BlockRecord, Error> {
    let hash = summary.block.hash();
    let height = summary.block.coinbase_height().ok_or_else(|| {
        Error::StateResponse("canonical block is missing its coinbase height".to_string())
    })?;
    let transaction_count = u32::try_from(summary.block.transactions.len())
        .map_err(|_| Error::Calculation("block transaction count exceeds u32".to_string()))?;
    let coinbase = summary.block.transactions.first().ok_or_else(|| {
        Error::StateResponse("canonical block is missing its coinbase transaction".to_string())
    })?;
    let (miner_address, miner_pool) = super::miner_attribution::identify_miner(coinbase, network);
    Ok(BlockRecord {
        height: height.0.to_string(),
        hash: hash.to_string(),
        timestamp: summary.block.header.time.timestamp().to_string(),
        transaction_count,
        size: summary.serialized_size,
        difficulty: format!(
            "{:.6}",
            summary
                .block
                .header
                .difficulty_threshold
                .relative_to_network(network)
        ),
        miner_address,
        total_fees: summary.total_fees_zat.to_string(),
        miner_pool,
    })
}

fn empty_response(limit: u32, total: u64) -> BlocksResponse {
    BlocksResponse {
        blocks: Vec::new(),
        pagination: BlocksPagination {
            limit,
            total: total.to_string(),
            has_next: false,
            has_prev: false,
            next_cursor: None,
            prev_cursor: None,
        },
    }
}
