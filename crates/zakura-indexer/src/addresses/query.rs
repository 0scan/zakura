//! Address summary reads and newest-first canonical transaction history.

use std::collections::HashMap;

use zakura_chain::{
    block::{Hash, Height},
    transparent::Address,
};

use crate::{
    database::{decode_trailing_transaction_position, DatabaseColumn},
    models::{AddressEffect, TransactionAddressEffects, TransactionPosition, TransactionRecord},
    transactions::{shielded_flow, shielded_pool, transaction_kind},
    types::{
        AddressActivity, AddressFirstFunding, AddressSummary, AddressTransactionListItem,
        AddressTransactionsPagination, AddressTransactionsResponse, PageDirection,
    },
    Error, Indexer,
};

use super::{
    cursor::AddressTransactionCursor,
    disk_format::{address_order_key, address_order_prefix, newest_address_order_key},
};

const DEFAULT_QUERY_LIMIT: u32 = 25;
const MAX_QUERY_LIMIT: u32 = 100;

impl Indexer {
    /// Returns compact general information for one transparent address.
    pub async fn address_summary(&self, address: Address) -> Result<AddressSummary, Error> {
        let indexer = self.clone();
        tokio::task::spawn_blocking(move || indexer.address_summary_blocking(address))
            .await
            .map_err(|error| Error::Task(error.to_string()))?
    }

    /// Returns one cursor-paginated page of canonical transactions for an address.
    pub async fn address_transactions_page(
        &self,
        address: Address,
        limit: Option<u32>,
        cursor: Option<String>,
        direction: PageDirection,
    ) -> Result<AddressTransactionsResponse, Error> {
        let indexer = self.clone();
        tokio::task::spawn_blocking(move || {
            indexer.address_transactions_page_blocking(address, limit, cursor, direction)
        })
        .await
        .map_err(|error| Error::Task(error.to_string()))?
    }

    fn address_summary_blocking(&self, address: Address) -> Result<AddressSummary, Error> {
        let indexed_tip = self.indexed_block_tip()?;
        let Some(record) = self.address_record(address)? else {
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

        let balance_zat = record
            .total_received_zat
            .checked_sub(record.total_sent_zat)
            .ok_or_else(|| {
                Error::CorruptData(format!(
                    "address {} sent total exceeds its received total",
                    address
                ))
            })?;
        let first_seen = self.address_activity(record.first_position)?;
        let last_seen = self.address_activity(record.last_position)?;
        let first_funding = record
            .first_funding_position
            .map(|position| self.address_first_funding(address, position))
            .transpose()?;

        Ok(AddressSummary {
            address: address.to_string(),
            balance_zat: balance_zat.to_string(),
            total_received_zat: record.total_received_zat.to_string(),
            total_sent_zat: record.total_sent_zat.to_string(),
            transaction_count: record.transaction_count.to_string(),
            first_seen: Some(first_seen),
            last_seen: Some(last_seen),
            first_funding,
            indexed_height: indexed_tip.map(|(height, _)| height.0.to_string()),
            indexed_block_hash: indexed_tip.map(|(_, hash)| hash.to_string()),
        })
    }

    fn address_transactions_page_blocking(
        &self,
        address: Address,
        limit: Option<u32>,
        cursor: Option<String>,
        direction: PageDirection,
    ) -> Result<AddressTransactionsResponse, Error> {
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
            if self.canonical_block_hash(cursor.position.height)? != Some(cursor.block_hash) {
                return Err(Error::InvalidCursor(
                    "cursor block is no longer on the indexed canonical chain".to_string(),
                ));
            }
            if self
                .database
                .get(
                    DatabaseColumn::AddressTransactionOrder,
                    address_order_key(address, cursor.position),
                )?
                .is_none()
            {
                return Err(Error::InvalidCursor(
                    "cursor transaction no longer belongs to this address".to_string(),
                ));
            }
        }

        let prefix = address_order_prefix(address);
        let start_key = cursor.map_or_else(
            || newest_address_order_key(address),
            |cursor| address_order_key(address, cursor.position),
        );
        let requested = usize::try_from(limit)
            .map_err(|_| Error::Calculation("address query limit exceeds usize".to_string()))?;
        let scan_limit = requested
            .checked_add(2)
            .ok_or_else(|| Error::Calculation("address scan limit overflow".to_string()))?;
        let entries = match direction {
            PageDirection::Next => self.database.scan_prefix_reverse_from(
                DatabaseColumn::AddressTransactionOrder,
                &prefix,
                &start_key,
                scan_limit,
            )?,
            PageDirection::Previous => self.database.scan_prefix_forward_from(
                DatabaseColumn::AddressTransactionOrder,
                &prefix,
                &start_key,
                scan_limit,
            )?,
        };
        let mut entries = entries
            .into_iter()
            .filter(|(key, _)| cursor.is_none() || key.as_slice() != start_key.as_slice())
            .collect::<Vec<_>>();
        let has_more_in_direction = entries.len() > requested;
        entries.truncate(requested);
        if direction == PageDirection::Previous {
            entries.reverse();
        }

        let mut block_cache = HashMap::new();
        let mut matched = Vec::with_capacity(entries.len());
        for (key, _) in entries {
            let position = decode_trailing_transaction_position(&key)?;
            let txid = self.canonical_transaction_hash(position)?.ok_or_else(|| {
                Error::CorruptData(format!(
                    "missing canonical transaction at height {}, index {}",
                    position.height.0, position.transaction_index
                ))
            })?;
            let record = self.transaction_record(txid)?.ok_or_else(|| {
                Error::CorruptData(format!("missing transaction record for {txid}"))
            })?;
            if record.position != position {
                return Err(Error::CorruptData(format!(
                    "transaction {txid} record position does not match address order"
                )));
            }
            let effects = self.transaction_address_effects(position)?.ok_or_else(|| {
                Error::CorruptData(format!(
                    "missing address effects at height {}, index {}",
                    position.height.0, position.transaction_index
                ))
            })?;
            let effect = effects
                .effects
                .iter()
                .find(|effect| effect.address == address)
                .copied()
                .ok_or_else(|| {
                    Error::CorruptData("address order entry has no matching effect".to_string())
                })?;
            let (block_hash, block_time) =
                self.address_transaction_block_fields(position.height, &mut block_cache)?;
            let item = address_transaction_item(
                address,
                txid.to_string(),
                block_hash,
                block_time,
                record,
                effect,
                &effects,
            )?;
            matched.push((position, block_hash, item));
        }

        let has_rows = !matched.is_empty();
        let (has_next, has_prev) = match direction {
            PageDirection::Next => (has_more_in_direction, cursor.is_some() && has_rows),
            PageDirection::Previous => (cursor.is_some() && has_rows, has_more_in_direction),
        };
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

    fn address_activity(&self, position: TransactionPosition) -> Result<AddressActivity, Error> {
        let txid = self.canonical_transaction_hash(position)?.ok_or_else(|| {
            Error::CorruptData("address activity is missing its canonical transaction".to_string())
        })?;
        let block_hash = self.canonical_block_hash(position.height)?.ok_or_else(|| {
            Error::CorruptData("address activity is missing its canonical block".to_string())
        })?;
        let block = self.block_record(block_hash)?.ok_or_else(|| {
            Error::CorruptData("address activity is missing its block record".to_string())
        })?;
        Ok(AddressActivity {
            txid: txid.to_string(),
            block_height: position.height.0.to_string(),
            block_hash: block_hash.to_string(),
            block_time: block.timestamp,
            transaction_index: position.transaction_index,
        })
    }

    fn address_first_funding(
        &self,
        address: Address,
        position: TransactionPosition,
    ) -> Result<AddressFirstFunding, Error> {
        let effects = self.transaction_address_effects(position)?.ok_or_else(|| {
            Error::CorruptData("first funding transaction is missing address effects".to_string())
        })?;
        let received_zat = effects
            .effects
            .iter()
            .find(|effect| effect.address == address)
            .map(|effect| effect.received_zat)
            .filter(|amount| *amount > 0)
            .ok_or_else(|| {
                Error::CorruptData("first funding transaction did not fund the address".to_string())
            })?;
        let funder_address = largest_effect_address(&effects, address, EffectSide::Sent);

        Ok(AddressFirstFunding {
            activity: self.address_activity(position)?,
            amount_zat: received_zat.to_string(),
            funder_address,
            is_coinbase: position.transaction_index == 0,
        })
    }

    fn address_transaction_block_fields(
        &self,
        height: Height,
        cache: &mut HashMap<u32, (Hash, String)>,
    ) -> Result<(Hash, String), Error> {
        if let Some((hash, timestamp)) = cache.get(&height.0) {
            return Ok((*hash, timestamp.clone()));
        }
        let hash = self.canonical_block_hash(height)?.ok_or_else(|| {
            Error::CorruptData(format!("missing canonical block at height {}", height.0))
        })?;
        let block = self.block_record(hash)?.ok_or_else(|| {
            Error::CorruptData(format!("missing block record for canonical hash {hash}"))
        })?;
        cache.insert(height.0, (hash, block.timestamp.clone()));
        Ok((hash, block.timestamp))
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
