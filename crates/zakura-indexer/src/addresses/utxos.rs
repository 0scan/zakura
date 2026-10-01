//! Newest-first pagination over one address's current transparent UTXO set.

use std::collections::{BTreeMap, HashSet};

use tower::ServiceExt;
use zakura_chain::{
    transaction,
    transparent::{Address, Output},
};
use zakura_state::{HashOrHeight, OutputLocation, ReadRequest, ReadResponse, ReadState};

use crate::{
    types::{AddressUtxoSummary, AddressUtxosPagination, AddressUtxosResponse, PageDirection},
    Error,
};

use super::utxo_cursor::AddressUtxoCursor;

const DEFAULT_QUERY_LIMIT: u32 = 25;
const MAX_QUERY_LIMIT: u32 = 100;

#[derive(Clone)]
struct PendingUtxo {
    txid: transaction::Hash,
    position: OutputLocation,
    output: Output,
}

/// Returns a cursor-paginated, newest-first page of current transparent UTXOs.
pub async fn address_utxos_page_from_state<State>(
    read_state: State,
    address: Address,
    limit: Option<u32>,
    cursor: Option<String>,
    direction: PageDirection,
) -> Result<AddressUtxosResponse, Error>
where
    State: ReadState,
{
    let limit = limit
        .unwrap_or(DEFAULT_QUERY_LIMIT)
        .clamp(1, MAX_QUERY_LIMIT);
    let cursor = cursor
        .map(|encoded| AddressUtxoCursor::decode(&encoded))
        .transpose()?;
    if direction == PageDirection::Previous && cursor.is_none() {
        return Err(Error::InvalidCursor(
            "direction=prev requires a cursor".to_string(),
        ));
    }
    if let Some(cursor) = cursor {
        if cursor.address != address {
            return Err(Error::InvalidCursor(
                "address UTXO cursor was created for a different address".to_string(),
            ));
        }
        validate_cursor_block(read_state.clone(), cursor).await?;
    }

    let response = call(
        read_state.clone(),
        ReadRequest::UtxosByAddresses(HashSet::from([address])),
    )
    .await?;
    let ReadResponse::AddressUtxos(utxos) = response else {
        return Err(unexpected_response("AddressUtxos"));
    };

    let mut all_utxos = utxos
        .utxos()
        .map(|(_, txid, position, output)| PendingUtxo {
            txid: *txid,
            position: *position,
            output: output.clone(),
        })
        .collect::<Vec<_>>();
    all_utxos.reverse();

    let cursor_index = cursor
        .map(|cursor| {
            all_utxos
                .iter()
                .position(|utxo| utxo.position == cursor.position)
                .ok_or_else(|| {
                    Error::InvalidCursor(
                        "address UTXO cursor output is no longer unspent".to_string(),
                    )
                })
        })
        .transpose()?;
    let page_capacity = usize::try_from(limit)
        .map_err(|_| Error::InvalidQuery("page limit is too large".to_string()))?;
    let (start, end, has_next, has_prev) =
        page_bounds(all_utxos.len(), cursor_index, page_capacity, direction);
    let page = all_utxos[start..end].to_vec();

    let finalized_response = call(read_state.clone(), ReadRequest::FinalizedTip).await?;
    let ReadResponse::FinalizedTip(finalized_tip) = finalized_response else {
        return Err(unexpected_response("FinalizedTip"));
    };
    let finalized_height = finalized_tip.map(|(height, _)| height);

    let mut block_context = BTreeMap::new();
    for height in page.iter().map(|utxo| utxo.position.height()) {
        if block_context.contains_key(&height) {
            continue;
        }
        let response = call(
            read_state.clone(),
            ReadRequest::BlockHeader(HashOrHeight::Height(height)),
        )
        .await?;
        let ReadResponse::BlockHeader {
            header,
            hash,
            height: returned_height,
            ..
        } = response
        else {
            return Err(unexpected_response("BlockHeader"));
        };
        if returned_height != height {
            return Err(Error::StateResponse(
                "state returned a block header at the wrong height".to_string(),
            ));
        }
        block_context.insert(height, (hash, header.time.timestamp()));
    }

    let mut summaries = Vec::with_capacity(page.len());
    let mut positioned_summaries = Vec::with_capacity(page.len());
    for utxo in page {
        let height = utxo.position.height();
        let (block_hash, block_time) = block_context.get(&height).ok_or_else(|| {
            Error::StateResponse("missing block context for address UTXO".to_string())
        })?;
        let tx_index = u32::from(utxo.position.transaction_index().index());
        summaries.push(AddressUtxoSummary {
            txid: utxo.txid.to_string(),
            output_index: utxo.position.output_index().index(),
            block_height: height.0.to_string(),
            tx_index,
            value_zat: u64::from(utxo.output.value).to_string(),
            script_hex: hex::encode(utxo.output.lock_script.as_raw_bytes()),
            coinbase: tx_index == 0,
            block_hash: block_hash.to_string(),
            block_time: block_time.to_string(),
            finalized: finalized_height.is_some_and(|finalized| height <= finalized),
        });
        positioned_summaries.push((utxo.position, *block_hash));
    }

    let next_cursor =
        positioned_summaries
            .last()
            .filter(|_| has_next)
            .map(|(position, block_hash)| {
                AddressUtxoCursor::new(address, *position, *block_hash).encode()
            });
    let prev_cursor =
        positioned_summaries
            .first()
            .filter(|_| has_prev)
            .map(|(position, block_hash)| {
                AddressUtxoCursor::new(address, *position, *block_hash).encode()
            });

    Ok(AddressUtxosResponse {
        address: address.to_string(),
        utxos: summaries,
        pagination: AddressUtxosPagination {
            limit,
            has_next,
            has_prev,
            next_cursor,
            prev_cursor,
        },
    })
}

async fn validate_cursor_block<State>(
    read_state: State,
    cursor: AddressUtxoCursor,
) -> Result<(), Error>
where
    State: ReadState,
{
    let response = read_state
        .oneshot(ReadRequest::BlockHeader(HashOrHeight::Height(
            cursor.position.height(),
        )))
        .await
        .map_err(|_| {
            Error::InvalidCursor("cursor block is no longer on the canonical chain".to_string())
        })?;
    let ReadResponse::BlockHeader { hash, .. } = response else {
        return Err(Error::StateResponse(
            "state returned the wrong response for a block-header request".to_string(),
        ));
    };
    if hash != cursor.block_hash {
        return Err(Error::InvalidCursor(
            "cursor block is no longer on the canonical chain".to_string(),
        ));
    }
    Ok(())
}

fn page_bounds(
    total: usize,
    cursor_index: Option<usize>,
    limit: usize,
    direction: PageDirection,
) -> (usize, usize, bool, bool) {
    match direction {
        PageDirection::Next => {
            let start = cursor_index.map_or(0, |index| index.saturating_add(1));
            let end = start.saturating_add(limit).min(total);
            let has_rows = start < end;
            (
                start.min(total),
                end,
                end < total,
                cursor_index.is_some() && has_rows,
            )
        }
        PageDirection::Previous => {
            let end = cursor_index.unwrap_or(0);
            let start = end.saturating_sub(limit);
            let has_rows = start < end;
            (start, end, cursor_index.is_some() && has_rows, start > 0)
        }
    }
}

async fn call<State>(read_state: State, request: ReadRequest) -> Result<ReadResponse, Error>
where
    State: ReadState,
{
    read_state
        .oneshot(request)
        .await
        .map_err(|error| Error::StateRequest(error.to_string()))
}

fn unexpected_response(expected: &str) -> Error {
    Error::StateResponse(format!(
        "state returned the wrong response for an explorer {expected} request"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_page_moves_to_older_outputs() {
        assert_eq!(
            page_bounds(10, None, 3, PageDirection::Next),
            (0, 3, true, false)
        );
        assert_eq!(
            page_bounds(10, Some(2), 3, PageDirection::Next),
            (3, 6, true, true)
        );
        assert_eq!(
            page_bounds(5, Some(2), 3, PageDirection::Next),
            (3, 5, false, true)
        );
    }

    #[test]
    fn previous_page_returns_the_immediately_newer_page() {
        assert_eq!(
            page_bounds(10, Some(8), 3, PageDirection::Previous),
            (5, 8, true, true)
        );
        assert_eq!(
            page_bounds(10, Some(2), 3, PageDirection::Previous),
            (0, 2, true, false)
        );
    }
}
