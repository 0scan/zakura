//! Address summary reads and newest-first canonical transaction history.

use std::{collections::HashMap, sync::Arc};

use tower::ServiceExt;
use zakura_chain::{
    block::Hash, parameters::Network, transaction::Transaction, transparent::Address,
};
use zakura_state::{
    ExplorerPageDirection, ExplorerReadRequest, ExplorerReadResponse, ExplorerTransactionSummary,
    ReadRequest, ReadResponse, ReadState, TransactionLocation,
};

use crate::{
    models::{TransactionPosition, TransactionRecord},
    transactions::{shielded_flow, shielded_pool, transaction_kind},
    types::{
        AddressActivity, AddressFirstFunding, AddressSummary, AddressTransactionListItem,
        AddressTransactionsPagination, AddressTransactionsResponse, PageDirection,
    },
    Error,
};

use super::{
    cursor::AddressTransactionCursor,
    effects::{derive_address_effects, AddressEffect, TransactionAddressEffects},
};

const DEFAULT_QUERY_LIMIT: u32 = 25;
const MAX_QUERY_LIMIT: u32 = 100;

/// Returns a transparent-address summary directly from canonical node state.
pub async fn address_summary_from_state<State>(
    read_state: State,
    network: &Network,
    address: Address,
) -> Result<AddressSummary, Error>
where
    State: ReadState,
{
    let page = load_address_page(
        read_state.clone(),
        address,
        1,
        None,
        ExplorerPageDirection::Older,
    )
    .await?;
    let indexed_tip = page.best_tip;
    let Some(activity) = page.activity else {
        return Ok(AddressSummary {
            address: address.to_string(),
            balance_zat: "0".to_string(),
            total_received_zat: "0".to_string(),
            total_sent_zat: "0".to_string(),
            transaction_count: "0".to_string(),
            first_seen: None,
            last_seen: None,
            first_funding: None,
            indexed_height: indexed_tip.map(|(height, _)| height.0.to_string()),
            indexed_block_hash: indexed_tip.map(|(_, hash)| hash.to_string()),
        });
    };

    let first_funding = if let Some(summary) = page.first_funding.as_ref() {
        let mut effects =
            load_address_effects(read_state, network, std::slice::from_ref(summary)).await?;
        let effects = effects
            .pop()
            .expect("one requested transaction has one address-effect result");
        let received_zat = effects
            .effects
            .iter()
            .find(|effect| effect.address == address)
            .map(|effect| effect.received_zat)
            .filter(|amount| *amount > 0)
            .ok_or_else(|| {
                Error::CorruptData("first funding transaction did not fund the address".to_string())
            })?;
        Some(AddressFirstFunding {
            activity: address_activity(summary),
            amount_zat: received_zat.to_string(),
            funder_address: largest_effect_address(&effects, address, EffectSide::Sent),
            is_coinbase: summary.location.index.as_usize() == 0,
        })
    } else {
        None
    };
    let total_sent_zat = page
        .received_zat
        .checked_sub(page.balance_zat)
        .ok_or_else(|| {
            Error::CorruptData("address balance exceeds its total received value".to_string())
        })?;

    Ok(AddressSummary {
        address: address.to_string(),
        balance_zat: page.balance_zat.to_string(),
        total_received_zat: page.received_zat.to_string(),
        total_sent_zat: total_sent_zat.to_string(),
        transaction_count: activity.transaction_count.to_string(),
        first_seen: page.first_seen.as_ref().map(address_activity),
        last_seen: page.last_seen.as_ref().map(address_activity),
        first_funding,
        indexed_height: indexed_tip.map(|(height, _)| height.0.to_string()),
        indexed_block_hash: indexed_tip.map(|(_, hash)| hash.to_string()),
    })
}

/// Returns a cursor-paginated transparent-address history from canonical node state.
pub async fn address_transactions_page_from_state<State>(
    read_state: State,
    network: &Network,
    address: Address,
    limit: Option<u32>,
    cursor: Option<String>,
    direction: PageDirection,
) -> Result<AddressTransactionsResponse, Error>
where
    State: ReadState,
{
    let limit = limit
        .unwrap_or(DEFAULT_QUERY_LIMIT)
        .clamp(1, MAX_QUERY_LIMIT);
    let cursor = cursor
        .map(|encoded| AddressTransactionCursor::decode(&encoded))
        .transpose()?;
    if direction == PageDirection::Previous && cursor.is_none() {
        return Err(Error::InvalidCursor(
            "direction=prev requires a cursor".to_string(),
        ));
    }
    if let Some(cursor) = cursor {
        if cursor.address != address {
            return Err(Error::InvalidCursor(
                "address cursor was created for a different address".to_string(),
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

    let state_direction = match direction {
        PageDirection::Next => ExplorerPageDirection::Older,
        PageDirection::Previous => ExplorerPageDirection::Newer,
    };
    let page = load_address_page(
        read_state.clone(),
        address,
        limit,
        cursor.map(|cursor| {
            TransactionLocation::from_u64(
                cursor.position.height,
                u64::from(cursor.position.transaction_index),
            )
        }),
        state_direction,
    )
    .await?;
    if !page.cursor_valid {
        return Err(Error::InvalidCursor(
            "cursor transaction no longer belongs to this address".to_string(),
        ));
    }

    let effects = load_address_effects(read_state, network, &page.transactions).await?;
    let has_rows = !page.transactions.is_empty();
    let (has_next, has_prev) = match direction {
        PageDirection::Next => (page.has_more, cursor.is_some() && has_rows),
        PageDirection::Previous => (cursor.is_some() && has_rows, page.has_more),
    };
    let mut matched = Vec::with_capacity(page.transactions.len());
    for (summary, effects) in page.transactions.into_iter().zip(effects) {
        let effect = effects
            .effects
            .iter()
            .find(|effect| effect.address == address)
            .copied()
            .ok_or_else(|| {
                Error::CorruptData("address transaction has no matching effect".to_string())
            })?;
        let position = TransactionPosition {
            height: summary.location.height,
            transaction_index: u32::from(summary.location.index.index()),
        };
        let item = address_transaction_item(
            address,
            summary.txid.to_string(),
            summary.block_hash,
            summary.block_time.to_string(),
            TransactionRecord::from_state(summary.location, summary.record),
            effect,
            &effects,
        )?;
        matched.push((position, summary.block_hash, item));
    }

    let next_cursor = matched
        .last()
        .filter(|_| has_next)
        .map(|(position, block_hash, _)| {
            AddressTransactionCursor::new(address, *position, *block_hash).encode()
        });
    let prev_cursor = matched
        .first()
        .filter(|_| has_prev)
        .map(|(position, block_hash, _)| {
            AddressTransactionCursor::new(address, *position, *block_hash).encode()
        });

    Ok(AddressTransactionsResponse {
        address: address.to_string(),
        transactions: matched.into_iter().map(|(_, _, item)| item).collect(),
        pagination: AddressTransactionsPagination {
            limit,
            has_next,
            has_prev,
            next_cursor,
            prev_cursor,
        },
    })
}

async fn load_address_page<State>(
    read_state: State,
    address: Address,
    limit: u32,
    cursor: Option<TransactionLocation>,
    direction: ExplorerPageDirection,
) -> Result<zakura_state::ExplorerAddressPage, Error>
where
    State: ReadState,
{
    let response = read_state
        .oneshot(ReadRequest::Explorer(ExplorerReadRequest::AddressPage {
            address,
            limit,
            cursor,
            direction,
        }))
        .await
        .map_err(|error| Error::StateRequest(error.to_string()))?;
    let ReadResponse::Explorer(ExplorerReadResponse::AddressPage(page)) = response else {
        return Err(Error::StateResponse(
            "state returned the wrong response for an explorer address request".to_string(),
        ));
    };
    Ok(*page)
}

async fn load_address_effects<State>(
    read_state: State,
    network: &Network,
    summaries: &[ExplorerTransactionSummary],
) -> Result<Vec<TransactionAddressEffects>, Error>
where
    State: ReadState,
{
    let locations = summaries
        .iter()
        .map(|summary| summary.location)
        .collect::<Vec<_>>();
    let response = read_state
        .clone()
        .oneshot(ReadRequest::Explorer(
            ExplorerReadRequest::TransactionsByLocation(locations.into()),
        ))
        .await
        .map_err(|error| Error::StateRequest(error.to_string()))?;
    let ReadResponse::Explorer(ExplorerReadResponse::TransactionsByLocation(transactions)) =
        response
    else {
        return Err(Error::StateResponse(
            "state returned the wrong response for explorer transactions".to_string(),
        ));
    };
    let transactions = transactions
        .into_iter()
        .map(|transaction| {
            transaction.ok_or_else(|| {
                Error::StateResponse(
                    "address transaction left the canonical chain while reading".to_string(),
                )
            })
        })
        .collect::<Result<Vec<Arc<Transaction>>, Error>>()?;
    let outpoints = transactions
        .iter()
        .flat_map(|transaction| transaction.inputs())
        .filter_map(|input| input.outpoint())
        .collect::<Vec<_>>();
    let response = read_state
        .oneshot(ReadRequest::Explorer(
            ExplorerReadRequest::TransparentOutputs(outpoints.clone().into()),
        ))
        .await
        .map_err(|error| Error::StateRequest(error.to_string()))?;
    let ReadResponse::Explorer(ExplorerReadResponse::TransparentOutputs(outputs)) = response else {
        return Err(Error::StateResponse(
            "state returned the wrong response for transparent outputs".to_string(),
        ));
    };
    let spent_utxos = outpoints
        .into_iter()
        .zip(outputs)
        .map(|(outpoint, output)| {
            output.map(|output| (outpoint, output)).ok_or_else(|| {
                Error::Calculation(format!("missing transparent output {outpoint:?}"))
            })
        })
        .collect::<Result<HashMap<_, _>, Error>>()?;

    transactions
        .iter()
        .map(|transaction| derive_address_effects(network, transaction, &spent_utxos))
        .collect()
}

fn address_activity(summary: &ExplorerTransactionSummary) -> AddressActivity {
    AddressActivity {
        txid: summary.txid.to_string(),
        block_height: summary.location.height.0.to_string(),
        block_hash: summary.block_hash.to_string(),
        block_time: summary.block_time.to_string(),
        transaction_index: u32::from(summary.location.index.index()),
    }
}

#[allow(clippy::too_many_arguments)]
fn address_transaction_item(
    address: Address,
    txid: String,
    block_hash: Hash,
    block_time: String,
    record: TransactionRecord,
    effect: AddressEffect,
    effects: &TransactionAddressEffects,
) -> Result<AddressTransactionListItem, Error> {
    let net_change_zat = i128::from(effect.received_zat) - i128::from(effect.sent_zat);
    let primary_counterparty = if net_change_zat > 0 {
        largest_effect_address(effects, address, EffectSide::Sent)
    } else if net_change_zat < 0 {
        largest_effect_address(effects, address, EffectSide::Received)
    } else {
        None
    };
    let sender_count = effect_count(effects, address, EffectSide::Sent)?;
    let recipient_count = effect_count(effects, address, EffectSide::Received)?;

    Ok(AddressTransactionListItem {
        txid,
        block_height: record.position.height.0.to_string(),
        block_hash: block_hash.to_string(),
        block_time,
        transaction_index: record.position.transaction_index,
        size: record.serialized_size,
        kind: transaction_kind(&record),
        pool: shielded_pool(&record),
        flow: shielded_flow(&record)?,
        has_sprout: record.joinsplit_count > 0,
        has_sapling: record.sapling_spend_count > 0 || record.sapling_output_count > 0,
        has_orchard: record.orchard_action_count > 0,
        has_ironwood: record.ironwood_action_count > 0,
        received_zat: effect.received_zat.to_string(),
        sent_zat: effect.sent_zat.to_string(),
        net_change_zat: net_change_zat.to_string(),
        primary_counterparty,
        sender_count,
        recipient_count,
    })
}

#[derive(Clone, Copy)]
enum EffectSide {
    Received,
    Sent,
}

fn effect_amount(effect: &AddressEffect, side: EffectSide) -> u64 {
    match side {
        EffectSide::Received => effect.received_zat,
        EffectSide::Sent => effect.sent_zat,
    }
}

fn largest_effect_address(
    effects: &TransactionAddressEffects,
    excluded_address: Address,
    side: EffectSide,
) -> Option<String> {
    effects
        .effects
        .iter()
        .filter(|effect| effect.address != excluded_address && effect_amount(effect, side) > 0)
        .max_by_key(|effect| (effect_amount(effect, side), effect.address.to_string()))
        .map(|effect| effect.address.to_string())
}

fn effect_count(
    effects: &TransactionAddressEffects,
    excluded_address: Address,
    side: EffectSide,
) -> Result<u32, Error> {
    u32::try_from(
        effects
            .effects
            .iter()
            .filter(|effect| effect.address != excluded_address && effect_amount(effect, side) > 0)
            .count(),
    )
    .map_err(|_| Error::Calculation("address counterparty count exceeds u32".to_string()))
}
