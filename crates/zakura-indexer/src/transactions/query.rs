//! Canonical newest-first transaction queries over materialized filter indexes.

use std::ops::RangeInclusive;

use tower::ServiceExt;
use zakura_chain::{block::Hash, transaction::Hash as TransactionHash};
use zakura_state::{
    ExplorerAmountFilter, ExplorerPageDirection, ExplorerReadRequest, ExplorerReadResponse,
    ExplorerShieldedFlowFilter, ExplorerShieldedPoolFilter, ExplorerTransactionKindFilter,
    ExplorerTransactionQuery, ReadRequest, ReadResponse, ReadState, TransactionLocation,
};

use crate::{
    height_range::TransactionHeightRange,
    models::{TransactionPosition, TransactionRecord},
    types::{PageDirection, TransactionListItem, TransactionsPagination, TransactionsResponse},
    Error,
};

use super::{
    classify::{
        public_flow_amount, shielded_flow, shielded_pool, shielded_value_balance, transaction_kind,
        transparent_input_total,
    },
    cursor::TransactionCursor,
    filter::{TransactionKindFilter, TransactionQuery},
};

const DEFAULT_QUERY_LIMIT: u32 = 25;
const MAX_QUERY_LIMIT: u32 = 100;

/// Reads canonical transaction summaries directly from the node state database.
pub async fn transactions_page_from_state<S>(
    read_state: S,
    query: TransactionQuery,
    limit: Option<u32>,
    cursor: Option<String>,
    direction: PageDirection,
    height_range: RangeInclusive<u32>,
) -> Result<TransactionsResponse, Error>
where
    S: ReadState,
{
    let query = query.validate().map_err(Error::InvalidQuery)?;
    let height_range = TransactionHeightRange::new(height_range)?;
    let limit = limit
        .unwrap_or(DEFAULT_QUERY_LIMIT)
        .clamp(1, MAX_QUERY_LIMIT);
    let cursor = cursor
        .map(|encoded| TransactionCursor::decode(&encoded))
        .transpose()?;
    if direction == PageDirection::Previous && cursor.is_none() {
        return Err(Error::InvalidCursor(
            "direction=prev requires a cursor".to_string(),
        ));
    }
    if let Some(cursor) = cursor {
        if !cursor.matches(query, height_range) {
            return Err(Error::InvalidCursor(
                "transaction cursor was created for different filters".to_string(),
            ));
        }
        let response = read_state
            .clone()
            .oneshot(ReadRequest::BlockHeader(cursor.position.height.into()))
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
    }

    let state_query = state_query(query);
    let state_direction = match direction {
        PageDirection::Next => ExplorerPageDirection::Older,
        PageDirection::Previous => ExplorerPageDirection::Newer,
    };
    let response = read_state
        .oneshot(ReadRequest::Explorer(
            ExplorerReadRequest::TransactionPage {
                query: state_query,
                limit,
                cursor: cursor.map(|cursor| {
                    TransactionLocation::from_u64(
                        cursor.position.height,
                        u64::from(cursor.position.transaction_index),
                    )
                }),
                direction: state_direction,
                height_range: height_range.from..=height_range.to,
            },
        ))
        .await
        .map_err(|error| Error::StateRequest(error.to_string()))?;
    let ReadResponse::Explorer(ExplorerReadResponse::TransactionPage(page)) = response else {
        return Err(Error::StateResponse(
            "state returned the wrong response for an explorer transaction request".to_string(),
        ));
    };
    if !page.cursor_valid {
        return Err(Error::InvalidCursor(
            "cursor transaction no longer matches the requested filters".to_string(),
        ));
    }

    let has_rows = !page.transactions.is_empty();
    let (has_next, has_prev) = match direction {
        PageDirection::Next => (page.has_more, cursor.is_some() && has_rows),
        PageDirection::Previous => (cursor.is_some() && has_rows, page.has_more),
    };
    let mut positions = Vec::with_capacity(page.transactions.len());
    let mut transactions = Vec::with_capacity(page.transactions.len());
    for summary in page.transactions {
        let position = TransactionPosition {
            height: summary.location.height,
            transaction_index: u32::from(summary.location.index.index()),
        };
        let record = TransactionRecord::from_state(summary.location, summary.record);
        transactions.push(transaction_list_item(
            summary.txid,
            summary.block_hash,
            summary.block_time.to_string(),
            record,
        )?);
        positions.push((position, summary.block_hash));
    }

    let next_cursor = positions
        .last()
        .filter(|_| has_next)
        .map(|(position, hash)| {
            TransactionCursor::new(*position, *hash, query, height_range).encode()
        });
    let prev_cursor = positions
        .first()
        .filter(|_| has_prev)
        .map(|(position, hash)| {
            TransactionCursor::new(*position, *hash, query, height_range).encode()
        });

    Ok(TransactionsResponse {
        transactions,
        pagination: TransactionsPagination {
            limit,
            has_next,
            has_prev,
            next_cursor,
            prev_cursor,
        },
    })
}

fn state_query(query: TransactionQuery) -> ExplorerTransactionQuery {
    ExplorerTransactionQuery {
        kind: match query.kind {
            TransactionKindFilter::All => ExplorerTransactionKindFilter::All,
            TransactionKindFilter::Shielded => ExplorerTransactionKindFilter::Shielded,
            TransactionKindFilter::Transparent => ExplorerTransactionKindFilter::Transparent,
            TransactionKindFilter::Coinbase => ExplorerTransactionKindFilter::Coinbase,
        },
        flow: match query.flow {
            super::ShieldedFlowFilter::All => ExplorerShieldedFlowFilter::All,
            super::ShieldedFlowFilter::Shield => ExplorerShieldedFlowFilter::Shield,
            super::ShieldedFlowFilter::Deshield => ExplorerShieldedFlowFilter::Deshield,
            super::ShieldedFlowFilter::FullyShielded => ExplorerShieldedFlowFilter::FullyShielded,
            super::ShieldedFlowFilter::Complex => ExplorerShieldedFlowFilter::Complex,
        },
        pool: match query.pool {
            super::ShieldedPoolFilter::All => ExplorerShieldedPoolFilter::All,
            super::ShieldedPoolFilter::Sprout => ExplorerShieldedPoolFilter::Sprout,
            super::ShieldedPoolFilter::Sapling => ExplorerShieldedPoolFilter::Sapling,
            super::ShieldedPoolFilter::Orchard => ExplorerShieldedPoolFilter::Orchard,
            super::ShieldedPoolFilter::Ironwood => ExplorerShieldedPoolFilter::Ironwood,
            super::ShieldedPoolFilter::Mixed => ExplorerShieldedPoolFilter::Mixed,
        },
        amount: match query.amount {
            super::AmountFilter::Any => ExplorerAmountFilter::Any,
            super::AmountFilter::AtLeastOneBillionZat => ExplorerAmountFilter::AtLeastOneBillion,
            super::AmountFilter::AtLeastTenBillionZat => ExplorerAmountFilter::AtLeastTenBillion,
            super::AmountFilter::AtLeastOneHundredBillionZat => {
                ExplorerAmountFilter::AtLeastOneHundredBillion
            }
        },
    }
}

fn transaction_list_item(
    txid: TransactionHash,
    block_hash: Hash,
    block_time: String,
    record: TransactionRecord,
) -> Result<TransactionListItem, Error> {
    let flow_amount_zat = public_flow_amount(&record)?;
    Ok(TransactionListItem {
        txid: txid.to_string(),
        block_height: record.position.height.0.to_string(),
        block_hash: block_hash.to_string(),
        block_time,
        transaction_index: record.position.transaction_index,
        size: record.serialized_size,
        kind: transaction_kind(&record),
        pool: shielded_pool(&record),
        flow: shielded_flow(&record)?,
        flow_amount_zat: flow_amount_zat.map(|amount| amount.to_string()),
        fee: record.fee_zat.to_string(),
        vin_count: record.transparent_input_count,
        vout_count: record.transparent_output_count,
        total_input: transparent_input_total(&record)?.to_string(),
        total_output: record.transparent_output_total_zat.to_string(),
        value_balance_transparent: record.transparent_value_balance_zat.to_string(),
        shielded_value_balance: shielded_value_balance(&record)?.to_string(),
        value_balance_sapling: record.sapling_value_balance_zat.to_string(),
        value_balance_orchard: record.orchard_value_balance_zat.to_string(),
        value_balance_ironwood: record.ironwood_value_balance_zat.to_string(),
        joinsplit_count: record.joinsplit_count,
        sapling_spend_count: record.sapling_spend_count,
        sapling_output_count: record.sapling_output_count,
        orchard_actions: record.orchard_action_count,
        ironwood_actions: record.ironwood_action_count,
    })
}
