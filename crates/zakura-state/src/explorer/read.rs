//! Canonical explorer reads merged across finalized and non-finalized state.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::Arc,
};

use zakura_chain::{transaction::Transaction, transparent};

use crate::{
    explorer::{
        ExplorerAddressPage, ExplorerAddressRecord, ExplorerBlockSummary, ExplorerPageDirection,
        ExplorerReadRequest, ExplorerReadResponse, ExplorerTransactionPage,
        ExplorerTransactionQuery, ExplorerTransactionSummary,
    },
    request::Spend,
    service::{
        finalized_state::ZakuraDb,
        non_finalized_state::Chain,
        read::{block_and_size, spending_transaction_hash, transparent_balance},
    },
    TransactionLocation,
};

use super::storage::explorer_transaction_record_with_ordered_utxos;

const EXPLORER_ROLLING_SCAN_LIMIT: u32 = 10_000;
const ROLLING_WINDOW_SECONDS: i64 = 86_400;

const MAX_EXPLORER_PAGE_SIZE: u32 = 100;

/// Handles every explorer read behind the state service's single extension point.
pub fn handle(
    chain: Option<Arc<Chain>>,
    db: &ZakuraDb,
    request: ExplorerReadRequest,
) -> Result<ExplorerReadResponse, crate::BoxError> {
    Ok(match request {
        ExplorerReadRequest::TransactionPage {
            query,
            limit,
            cursor,
            direction,
        } => ExplorerReadResponse::TransactionPage(explorer_transaction_page(
            chain, db, query, limit, cursor, direction,
        )),
        ExplorerReadRequest::TransparentOutputs(outpoints) => {
            ExplorerReadResponse::TransparentOutputs(explorer_transparent_outputs(
                chain, db, &outpoints,
            ))
        }
        ExplorerReadRequest::TransparentOutputSpends(outpoints) => {
            ExplorerReadResponse::TransparentOutputSpends(
                outpoints
                    .iter()
                    .map(|outpoint| {
                        spending_transaction_hash(chain.clone(), db, Spend::OutPoint(*outpoint))
                            .is_some()
                    })
                    .collect(),
            )
        }
        ExplorerReadRequest::TransactionSummary(txid) => {
            ExplorerReadResponse::TransactionSummary(explorer_transaction_summary(chain, db, txid))
        }
        ExplorerReadRequest::BlockSummaries(heights) => {
            ExplorerReadResponse::BlockSummaries(explorer_block_summaries(chain, db, &heights))
        }
        ExplorerReadRequest::AddressPage {
            address,
            limit,
            cursor,
            direction,
        } => ExplorerReadResponse::AddressPage(Box::new(explorer_address_page(
            chain, db, address, limit, cursor, direction,
        )?)),
        ExplorerReadRequest::TransactionsByLocation(locations) => {
            ExplorerReadResponse::TransactionsByLocation(explorer_transactions_by_location(
                chain, db, &locations,
            ))
        }
        ExplorerReadRequest::StatsSnapshot => {
            ExplorerReadResponse::StatsSnapshot(explorer_stats_snapshot(db))
        }
        ExplorerReadRequest::DailyStats {
            include_non_finalized,
        } => {
            ExplorerReadResponse::DailyStats(explorer_daily_stats(chain, db, include_non_finalized))
        }
        ExplorerReadRequest::BalanceRankPage { limit, cursor } => {
            ExplorerReadResponse::BalanceRankPage(explorer_balance_rank_page(db, limit, cursor))
        }
    })
}

/// Returns canonical block summaries for `heights`, preserving request order.
pub fn explorer_block_summaries(
    chain: Option<Arc<Chain>>,
    db: &ZakuraDb,
    heights: &[zakura_chain::block::Height],
) -> Vec<Option<ExplorerBlockSummary>> {
    heights
        .iter()
        .map(|height| {
            let (block, serialized_size) = block_and_size(chain.clone(), db, (*height).into())?;
            let total_fees_zat = if let Some(contextual) = chain
                .as_ref()
                .and_then(|chain| chain.block((*height).into()))
            {
                contextual
                    .block
                    .transactions
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| *index > 0)
                    .map(|(index, transaction)| {
                        explorer_transaction_record_with_ordered_utxos(
                            transaction,
                            index,
                            &contextual.spent_outputs,
                        )
                        .fee_zat
                    })
                    .try_fold(0_u64, u64::checked_add)
                    .expect("verified block transaction fees fit in u64")
            } else {
                db.explorer_block_total_fees(*height)
            };
            Some(ExplorerBlockSummary {
                block,
                serialized_size: u32::try_from(serialized_size)
                    .expect("verified block size fits in u32"),
                total_fees_zat,
            })
        })
        .collect()
}

/// Resolves historical transparent outputs in one state snapshot.
///
/// Parent transactions are cached by hash, so many inputs from the same
/// transaction require one finalized-state transaction read.
pub fn explorer_transparent_outputs(
    chain: Option<Arc<Chain>>,
    db: &ZakuraDb,
    outpoints: &[transparent::OutPoint],
) -> Vec<Option<transparent::Utxo>> {
    let mut parent_transactions =
        HashMap::<_, Option<(Arc<Transaction>, TransactionLocation)>>::new();

    outpoints
        .iter()
        .map(|outpoint| {
            if let Some(utxo) = chain
                .as_ref()
                .and_then(|chain| chain.created_utxo(outpoint))
                .or_else(|| db.utxo(outpoint).map(|ordered| ordered.utxo))
            {
                return Some(utxo);
            }

            let parent = parent_transactions.entry(outpoint.hash).or_insert_with(|| {
                let location = db.transaction_location(outpoint.hash)?;
                let (transaction, _, _) = db.transaction(outpoint.hash)?;
                Some((transaction, location))
            });
            parent.as_ref().and_then(|(transaction, location)| {
                transaction
                    .outputs()
                    .get(usize::try_from(outpoint.index).expect("u32 output index fits in usize"))
                    .cloned()
                    .map(|output| {
                        transparent::Utxo::from_location(
                            output,
                            location.height,
                            location.index.as_usize(),
                        )
                    })
            })
        })
        .collect()
}

/// Returns one canonical transaction page, including the non-finalized best-chain suffix.
pub fn explorer_transaction_page(
    chain: Option<Arc<Chain>>,
    db: &ZakuraDb,
    query: ExplorerTransactionQuery,
    limit: u32,
    cursor: Option<TransactionLocation>,
    direction: ExplorerPageDirection,
) -> ExplorerTransactionPage {
    let finalized_tip = db.tip();
    let best_tip = chain
        .as_ref()
        .map(|chain| chain.non_finalized_tip())
        .or(finalized_tip);
    if !query.is_valid() {
        return ExplorerTransactionPage {
            best_tip,
            finalized_tip,
            cursor_valid: cursor.is_none(),
            transactions: Vec::new(),
            has_more: false,
        };
    }

    let requested = usize::try_from(limit.clamp(1, MAX_EXPLORER_PAGE_SIZE))
        .expect("the explorer page-size limit fits in usize");
    let scan_limit = requested.saturating_add(1);
    let cursor_valid = cursor.is_none_or(|location| {
        explorer_transaction_summary_at_location(chain.as_ref(), db, location)
            .is_some_and(|summary| query.matches(location, summary.record))
    });
    let mut transactions = finalized_summaries(db, query, cursor, direction, scan_limit);
    if let Some(chain) = chain {
        transactions.extend(non_finalized_summaries(&chain, query, cursor, direction));
    }

    transactions.sort_unstable_by_key(|summary| summary.location);
    transactions.dedup_by_key(|summary| summary.location);
    match direction {
        ExplorerPageDirection::Older => transactions.reverse(),
        ExplorerPageDirection::Newer => {
            transactions.truncate(scan_limit);
            transactions.reverse();
        }
    }
    let has_more = transactions.len() > requested;
    transactions.truncate(requested);

    ExplorerTransactionPage {
        best_tip,
        finalized_tip,
        cursor_valid,
        transactions,
        has_more,
    }
}

/// Returns one canonical explorer transaction summary by transaction hash.
pub fn explorer_transaction_summary(
    chain: Option<Arc<Chain>>,
    db: &ZakuraDb,
    txid: zakura_chain::transaction::Hash,
) -> Option<ExplorerTransactionSummary> {
    let location = chain
        .as_ref()
        .and_then(|chain| chain.transaction_location(txid))
        .or_else(|| db.transaction_location(txid))?;
    explorer_transaction_summary_at_location(chain.as_ref(), db, location)
}

/// Returns canonical transactions for `locations`, preserving request order.
pub fn explorer_transactions_by_location(
    chain: Option<Arc<Chain>>,
    db: &ZakuraDb,
    locations: &[TransactionLocation],
) -> Vec<Option<Arc<Transaction>>> {
    locations
        .iter()
        .map(|location| {
            chain
                .as_ref()
                .and_then(|chain| chain.transaction_by_loc(*location).cloned())
                .or_else(|| {
                    let txid = db.transaction_hash(*location)?;
                    db.transaction(txid).map(|(transaction, _, _)| transaction)
                })
        })
        .collect()
}

/// Returns canonical transparent-address state and one transaction page.
pub fn explorer_address_page(
    chain: Option<Arc<Chain>>,
    db: &ZakuraDb,
    address: transparent::Address,
    limit: u32,
    cursor: Option<TransactionLocation>,
    direction: ExplorerPageDirection,
) -> Result<ExplorerAddressPage, crate::BoxError> {
    let requested = usize::try_from(limit.clamp(1, MAX_EXPLORER_PAGE_SIZE))
        .map_err(|_| crate::BoxError::from("explorer page size exceeds usize"))?;
    let scan_limit = requested.saturating_add(1);
    let finalized_tip = db.tip();
    let best_tip = chain
        .as_ref()
        .map(|chain| chain.non_finalized_tip())
        .or(finalized_tip);
    let addresses = HashSet::from([address]);
    let (balance, received_zat) = transparent_balance(chain.clone(), db, addresses.clone())?;

    let mut locations =
        db.explorer_address_transaction_locations(address, cursor, direction, scan_limit);
    let mut all_non_finalized_locations = Vec::new();
    if let Some(chain) = chain.as_ref() {
        let finalized_height = finalized_tip.map(|(height, _)| height);
        all_non_finalized_locations = chain
            .partial_transparent_tx_ids(
                &addresses,
                chain.non_finalized_root_height()..=chain.non_finalized_tip_height(),
            )
            .into_keys()
            .filter(|location| finalized_height.is_none_or(|height| location.height > height))
            .collect();
        locations.extend(
            all_non_finalized_locations
                .iter()
                .copied()
                .filter(|location| match (direction, cursor) {
                    (ExplorerPageDirection::Older, Some(cursor)) => *location < cursor,
                    (ExplorerPageDirection::Newer, Some(cursor)) => *location > cursor,
                    (_, None) => true,
                }),
        );
    }
    let cursor_valid = cursor.is_none_or(|cursor| {
        db.explorer_address_contains_transaction(address, cursor)
            || all_non_finalized_locations.contains(&cursor)
    });
    locations.sort_unstable();
    locations.dedup();
    match direction {
        ExplorerPageDirection::Older => locations.reverse(),
        ExplorerPageDirection::Newer => {
            locations.truncate(scan_limit);
            locations.reverse();
        }
    }
    let has_more = locations.len() > requested;
    locations.truncate(requested);
    let transactions = locations
        .into_iter()
        .map(|location| {
            explorer_transaction_summary_at_location(chain.as_ref(), db, location)
                .expect("an address transaction index has its canonical transaction")
        })
        .collect();

    let mut activity = db.explorer_address_record(address);
    all_non_finalized_locations.sort_unstable();
    for location in all_non_finalized_locations {
        let transaction = chain
            .as_ref()
            .and_then(|chain| chain.transaction_by_loc(location))
            .expect("a non-finalized address transaction location has its transaction");
        let funded = transaction.outputs().iter().any(|output| {
            output.address(
                &chain
                    .as_ref()
                    .expect("non-finalized locations require a chain")
                    .network(),
            ) == Some(address)
                && output.value().zatoshis() > 0
        });
        let record = activity.get_or_insert(ExplorerAddressRecord {
            transaction_count: 0,
            first_location: location,
            last_location: location,
            first_funding_location: None,
        });
        record.transaction_count = record
            .transaction_count
            .checked_add(1)
            .expect("address transaction count fits in u64");
        record.last_location = location;
        if funded && record.first_funding_location.is_none() {
            record.first_funding_location = Some(location);
        }
    }

    let first_seen = activity.and_then(|record| {
        explorer_transaction_summary_at_location(chain.as_ref(), db, record.first_location)
    });
    let last_seen = activity.and_then(|record| {
        explorer_transaction_summary_at_location(chain.as_ref(), db, record.last_location)
    });
    let first_funding = activity.and_then(|record| {
        record.first_funding_location.and_then(|location| {
            explorer_transaction_summary_at_location(chain.as_ref(), db, location)
        })
    });

    Ok(ExplorerAddressPage {
        best_tip,
        cursor_valid,
        balance_zat: u64::try_from(balance.zatoshis()).map_err(|_| {
            crate::BoxError::from("transparent address balance must be nonnegative")
        })?,
        received_zat,
        activity,
        first_seen,
        last_seen,
        first_funding,
        transactions,
        has_more,
    })
}

/// Returns finalized explorer totals and a bounded trailing 24-hour window.
pub fn explorer_stats_snapshot(db: &ZakuraDb) -> crate::ExplorerStatsSnapshot {
    let best_tip = db.tip();
    let totals = db.explorer_chain_stats();
    let Some((tip_height, _)) = best_tip else {
        return crate::ExplorerStatsSnapshot {
            best_tip,
            totals,
            trailing_24h: crate::ExplorerRollingStats {
                complete: true,
                ..Default::default()
            },
        };
    };
    let tip = db
        .explorer_block_stats(tip_height)
        .expect("finalized explorer tip has block analytics");
    let window_end = tip.timestamp;
    let window_start = window_end.saturating_sub(ROLLING_WINDOW_SECONDS);
    let mut rolling = crate::ExplorerChainStats::default();
    let mut oldest_timestamp = window_end;
    let mut complete = false;
    let mut scheduled_subsidy_zat = 0_u128;
    let mut coinbase_unclaimed_zat = 0_u128;
    let mut boundary_issued_supply = None;
    let mut height = tip_height;

    for _ in 0..EXPLORER_ROLLING_SCAN_LIMIT {
        let block = db
            .explorer_block_stats(height)
            .expect("finalized explorer heights have block analytics");
        if block.timestamp < window_start {
            complete = true;
            boundary_issued_supply = Some(block.total_issuance);
            break;
        }
        crate::explorer::analytics::add_block_to_chain_stats(&mut rolling, &block);
        scheduled_subsidy_zat = scheduled_subsidy_zat
            .checked_add(block.interval.total_subsidy_zat)
            .expect("a bounded rolling window subsidy total fits in u128");
        coinbase_unclaimed_zat = coinbase_unclaimed_zat
            .checked_add(block.interval.coinbase_unclaimed_zat)
            .expect("a bounded rolling window unclaimed total fits in u128");
        oldest_timestamp = oldest_timestamp.min(block.timestamp);
        match height.previous() {
            Ok(previous) => height = previous,
            Err(_) => {
                complete = true;
                boundary_issued_supply = Some(0);
                break;
            }
        }
    }

    let issued_supply_change_zat = issued_supply_change(tip.total_issuance, boundary_issued_supply);

    crate::ExplorerStatsSnapshot {
        best_tip,
        totals,
        trailing_24h: crate::ExplorerRollingStats {
            complete,
            window_start: Some(window_start),
            window_end: Some(window_end),
            oldest_timestamp: Some(oldest_timestamp),
            totals: rolling,
            scheduled_subsidy_zat,
            coinbase_unclaimed_zat,
            issued_supply_change_zat,
        },
    }
}

fn issued_supply_change(
    tip_issued_supply: u64,
    boundary_issued_supply: Option<u64>,
) -> Option<i128> {
    boundary_issued_supply.map(|boundary| i128::from(tip_issued_supply) - i128::from(boundary))
}

/// Returns UTC daily snapshots in ascending order, optionally merged with the best live chain.
pub fn explorer_daily_stats(
    chain: Option<Arc<Chain>>,
    db: &ZakuraDb,
    include_non_finalized: bool,
) -> Vec<crate::ExplorerDailyStats> {
    let finalized = db.explorer_daily_stats();
    if !include_non_finalized {
        return finalized;
    }
    let Some(chain) = chain else {
        return finalized;
    };

    let mut by_day = finalized
        .into_iter()
        .map(|stats| (stats.day, stats))
        .collect::<BTreeMap<_, _>>();
    let network = chain.network();
    let first_height = chain.non_finalized_root_height();
    let last_height = chain.non_finalized_tip_height();
    let finalized_anchor = first_height
        .previous()
        .ok()
        .and_then(|height| db.explorer_block_stats(height));
    let mut previous_timestamp = finalized_anchor.as_ref().map(|block| block.timestamp);
    let mut previous_pool_nsm = finalized_anchor
        .as_ref()
        .map(|block| block.pool_nsm)
        .unwrap_or_default();
    let mut funded_transparent_address_count =
        db.explorer_chain_stats().funded_transparent_address_count;
    let mut transparent_balances = HashMap::new();

    for raw_height in first_height.0..=last_height.0 {
        let height = zakura_chain::block::Height(raw_height);
        let contextual = chain
            .block(height.into())
            .expect("best non-finalized chain contains every height in its range");
        funded_transparent_address_count = apply_live_transparent_balance_changes(
            db,
            &network,
            contextual,
            &mut transparent_balances,
            funded_transparent_address_count,
        );
        let block_info = chain
            .block_info(height.into())
            .expect("best non-finalized chain contains block info for every height");
        let spent_utxos = contextual
            .spent_outputs
            .iter()
            .map(|(outpoint, ordered)| (*outpoint, ordered.utxo.clone()))
            .collect::<HashMap<_, _>>();
        let block = super::analytics::derive_block_stats(
            &contextual.block,
            height,
            block_info.size(),
            *block_info.value_pools(),
            &spent_utxos,
            previous_pool_nsm,
            funded_transparent_address_count,
            &network,
        );
        let day = super::analytics::day_number(block.timestamp);
        super::analytics::add_block_to_daily_stats(
            by_day.entry(day).or_default(),
            &block,
            previous_timestamp,
        );
        previous_timestamp = Some(block.timestamp);
        previous_pool_nsm = block.pool_nsm;
    }

    by_day.into_values().collect()
}

fn apply_live_transparent_balance_changes(
    db: &ZakuraDb,
    network: &zakura_chain::parameters::Network,
    block: &crate::ContextuallyVerifiedBlock,
    balances: &mut HashMap<transparent::Address, u64>,
    mut funded_count: u64,
) -> u64 {
    let mut changes = HashMap::<transparent::Address, i128>::new();
    for transaction in &block.block.transactions {
        for outpoint in transaction
            .inputs()
            .iter()
            .filter_map(|input| input.outpoint())
        {
            let spent = block
                .spent_outputs
                .get(&outpoint)
                .expect("verified block inputs have resolved transparent outputs");
            if let Some(address) = spent.utxo.output.address(network) {
                let value = i128::from(spent.utxo.output.value().zatoshis());
                *changes.entry(address).or_default() -= value;
            }
        }
        for output in transaction.outputs() {
            if let Some(address) = output.address(network) {
                let value = i128::from(output.value().zatoshis());
                *changes.entry(address).or_default() += value;
            }
        }
    }

    for (address, change) in changes {
        let old_balance = balances.get(&address).copied().unwrap_or_else(|| {
            db.address_balance(&address)
                .map(|(amount, _)| {
                    u64::try_from(amount.zatoshis())
                        .expect("finalized transparent balances are nonnegative")
                })
                .unwrap_or_default()
        });
        let new_balance = u64::try_from(i128::from(old_balance) + change)
            .expect("verified transparent balance remains nonnegative");
        match (old_balance > 0, new_balance > 0) {
            (false, true) => {
                funded_count = funded_count
                    .checked_add(1)
                    .expect("funded transparent address count fits in u64");
            }
            (true, false) => {
                funded_count = funded_count
                    .checked_sub(1)
                    .expect("removed funded address was previously counted");
            }
            _ => {}
        }
        balances.insert(address, new_balance);
    }

    funded_count
}

/// Returns one stable page from the finalized transparent balance ranking.
pub fn explorer_balance_rank_page(
    db: &ZakuraDb,
    limit: u32,
    cursor: Option<crate::ExplorerBalanceRankCursor>,
) -> crate::ExplorerBalanceRankPage {
    let best_tip = db.tip();
    let cursor_valid = cursor.is_none_or(|cursor| {
        best_tip.is_some_and(|(_, hash)| hash == cursor.block_hash)
            && db.explorer_contains_balance_entry(cursor.address, cursor.balance_zat)
    });
    if !cursor_valid {
        return crate::ExplorerBalanceRankPage {
            best_tip,
            cursor_valid,
            ..Default::default()
        };
    }

    let requested =
        usize::try_from(limit.clamp(1, 100)).expect("explorer balance page limit fits in usize");
    let page_size = requested.saturating_add(1);
    // The first page and the concentration summary both start at the richest address. Read the
    // larger of those ranges once instead of seeking and decoding the same keys twice.
    let scan_size = if cursor.is_none() {
        page_size.max(100)
    } else {
        page_size
    };
    let mut entries = db.explorer_balance_entries(
        cursor.map(|cursor| (cursor.address, cursor.balance_zat)),
        scan_size,
    );
    let (top_10_balance_zat, top_100_balance_zat) = if cursor.is_none() {
        top_balance_totals(&entries)
    } else {
        top_balance_totals(&db.explorer_balance_entries(None, 100))
    };
    let has_more = entries.len() > requested;
    entries.truncate(requested);
    let totals = db.explorer_chain_stats();
    let transparent_supply_zat = best_tip
        .and_then(|(height, _)| db.explorer_block_stats(height))
        .map(|stats| stats.pool_transparent)
        .unwrap_or(0);

    crate::ExplorerBalanceRankPage {
        best_tip,
        cursor_valid,
        entries,
        funded_transparent_address_count: totals.funded_transparent_address_count,
        transparent_supply_zat,
        top_10_balance_zat,
        top_100_balance_zat,
        has_more,
    }
}

fn top_balance_totals(entries: &[crate::ExplorerBalanceRankEntry]) -> (u64, u64) {
    entries.iter().take(100).enumerate().fold(
        (0_u64, 0_u64),
        |(top_10, top_100), (index, entry)| {
            let top_100 = top_100
                .checked_add(entry.balance_zat)
                .expect("transparent supply bounds top balances");
            let top_10 = if index < 10 {
                top_10
                    .checked_add(entry.balance_zat)
                    .expect("transparent supply bounds top balances")
            } else {
                top_10
            };
            (top_10, top_100)
        },
    )
}

fn explorer_transaction_summary_at_location(
    chain: Option<&Arc<Chain>>,
    db: &ZakuraDb,
    location: TransactionLocation,
) -> Option<ExplorerTransactionSummary> {
    if let Some(chain) = chain {
        if let Some(block) = chain.block(location.height.into()) {
            let transaction = block.block.transactions.get(location.index.as_usize())?;
            let record = explorer_transaction_record_with_ordered_utxos(
                transaction,
                location.index.as_usize(),
                &block.spent_outputs,
            );
            return Some(ExplorerTransactionSummary {
                location,
                txid: *block.transaction_hashes.get(location.index.as_usize())?,
                block_hash: block.hash,
                block_time: block.block.header.time.timestamp(),
                record,
            });
        }
    }

    let txid = db.transaction_hash(location)?;
    let block_hash = db.hash(location.height)?;
    let block_time = db.block_header(location.height.into())?.time.timestamp();
    let record = db.explorer_transaction_record(location)?;
    Some(ExplorerTransactionSummary {
        location,
        txid,
        block_hash,
        block_time,
        record,
    })
}

fn finalized_summaries(
    db: &ZakuraDb,
    query: ExplorerTransactionQuery,
    cursor: Option<TransactionLocation>,
    direction: ExplorerPageDirection,
    limit: usize,
) -> Vec<ExplorerTransactionSummary> {
    db.explorer_transaction_locations(query, cursor, direction, limit)
        .into_iter()
        .map(|location| {
            let txid = db
                .transaction_hash(location)
                .expect("explorer transaction location is written with its transaction hash");
            let block_hash = db
                .hash(location.height)
                .expect("explorer transaction location is written with its block hash");
            let block_time = db
                .block_header(location.height.into())
                .expect("explorer transaction location is written with its block header")
                .time
                .timestamp();
            let record = db
                .explorer_transaction_record(location)
                .expect("explorer transaction index is written with its metadata");
            ExplorerTransactionSummary {
                location,
                txid,
                block_hash,
                block_time,
                record,
            }
        })
        .collect()
}

fn non_finalized_summaries(
    chain: &Chain,
    query: ExplorerTransactionQuery,
    cursor: Option<TransactionLocation>,
    direction: ExplorerPageDirection,
) -> Vec<ExplorerTransactionSummary> {
    let root = chain.non_finalized_root_height();
    let tip = chain.non_finalized_tip_height();
    let mut summaries = Vec::new();

    for height in root.0..=tip.0 {
        let block = chain
            .block(zakura_chain::block::Height(height).into())
            .expect("every height between the non-finalized root and tip has a block");
        for (transaction_index, transaction) in block.block.transactions.iter().enumerate() {
            let location = TransactionLocation::from_usize(block.height, transaction_index);
            let is_cursor_adjacent = match (direction, cursor) {
                (ExplorerPageDirection::Older, Some(cursor)) => location < cursor,
                (ExplorerPageDirection::Newer, Some(cursor)) => location > cursor,
                (_, None) => true,
            };
            if !is_cursor_adjacent {
                continue;
            }

            let record = explorer_transaction_record_with_ordered_utxos(
                transaction,
                transaction_index,
                &block.spent_outputs,
            );
            if !query.matches(location, record) {
                continue;
            }
            let txid = *block
                .transaction_hashes
                .get(transaction_index)
                .expect("verified block transaction hashes parallel its transactions");
            summaries.push(ExplorerTransactionSummary {
                location,
                txid,
                block_hash: block.hash,
                block_time: block.block.header.time.timestamp(),
                record,
            });
        }
    }

    summaries
}

#[cfg(test)]
mod tests {
    use zakura_chain::{parameters::NetworkKind, transparent::Address};

    use super::{issued_supply_change, top_balance_totals};

    #[test]
    fn issued_supply_change_requires_a_complete_window_boundary() {
        assert_eq!(issued_supply_change(125, Some(100)), Some(25));
        assert_eq!(issued_supply_change(100, Some(125)), Some(-25));
        assert_eq!(issued_supply_change(125, None), None);
    }

    #[test]
    fn top_balance_totals_use_only_the_first_hundred_entries() {
        let entries = (0..105)
            .map(|index| crate::ExplorerBalanceRankEntry {
                address: Address::from_pub_key_hash(NetworkKind::Mainnet, [index; 20]),
                balance_zat: 1,
                transaction_count: u64::from(index),
            })
            .collect::<Vec<_>>();

        assert_eq!(top_balance_totals(&entries), (10, 100));
    }
}
