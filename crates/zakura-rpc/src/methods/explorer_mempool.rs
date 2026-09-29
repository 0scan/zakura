//! Mempool explorer response assembly and volatile cursor pagination.

use std::collections::HashSet;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use zakura_chain::{
    parameters::Network,
    serialization::ZcashSerialize,
    transaction::{Hash as TransactionHash, VerifiedUnminedTx},
    transparent::{Input, OutPoint},
};
use zakura_indexer::{
    classify_unmined_transaction, Error, PageDirection, ShieldedFlowFilter, ShieldedPoolFilter,
    TransactionClassification, TransactionData, TransactionKind, TransactionKindFilter,
    TransactionStatus, TransactionsPagination,
};
use zakura_node_services::mempool::TransactionDependencies;

use super::types::{
    explorer::GetTransactionsRequest,
    explorer_mempool::{
        MempoolTransactionListItem, MempoolTransactionMetadata, MempoolTransactionSummary,
        MempoolTransactionsResponse, PendingTransactionDetails,
    },
};

const DEFAULT_QUERY_LIMIT: u32 = 25;
const MAX_QUERY_LIMIT: u32 = 100;
const CURSOR_BYTE_LENGTH: usize = 44;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MempoolPosition {
    first_seen: i64,
    txid: TransactionHash,
}

struct PositionedTransaction {
    position: MempoolPosition,
    item: MempoolTransactionListItem,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MempoolCursor {
    position: MempoolPosition,
    filter_tags: [u8; 4],
}

pub(super) fn transactions_page(
    transactions: Vec<VerifiedUnminedTx>,
    dependencies: &TransactionDependencies,
    request: &GetTransactionsRequest,
) -> Result<MempoolTransactionsResponse, Error> {
    let query = request.transaction_query().map_err(Error::InvalidQuery)?;
    let filter_tags = filter_tags(request)?;
    let cursor = request
        .cursor
        .as_deref()
        .map(MempoolCursor::decode)
        .transpose()?;
    if request.direction == PageDirection::Previous && cursor.is_none() {
        return Err(Error::InvalidCursor(
            "direction=prev requires a cursor".to_string(),
        ));
    }
    if cursor.is_some_and(|cursor| cursor.filter_tags != filter_tags) {
        return Err(Error::InvalidCursor(
            "mempool cursor was created for different filters".to_string(),
        ));
    }

    let mut summary = MempoolTransactionSummary::default();
    let mut matches = Vec::new();
    for transaction in &transactions {
        let fee_zat = fee_zat(transaction)?;
        let classification = classify_unmined_transaction(
            transaction.transaction.transaction().as_ref(),
            fee_zat,
            transaction.spent_outputs.as_slice(),
        )?;
        record_summary_transaction(&mut summary, classification.kind)?;
        if query.matches(classification) {
            matches.push(positioned_list_item(
                transaction,
                dependencies,
                fee_zat,
                classification,
            )?);
        }
    }
    matches.sort_by(|left, right| {
        right
            .position
            .first_seen
            .cmp(&left.position.first_seen)
            .then_with(|| right.position.txid.0.cmp(&left.position.txid.0))
    });

    let cursor_index = cursor
        .map(|cursor| {
            matches
                .iter()
                .position(|entry| entry.position == cursor.position)
                .ok_or_else(|| {
                    Error::InvalidCursor(
                        "mempool cursor transaction is no longer available".to_string(),
                    )
                })
        })
        .transpose()?;
    let limit = request
        .limit
        .unwrap_or(DEFAULT_QUERY_LIMIT)
        .clamp(1, MAX_QUERY_LIMIT);
    let requested = usize::try_from(limit)
        .map_err(|_| Error::Calculation("mempool query limit exceeds usize".to_string()))?;
    let (start, end) = match request.direction {
        PageDirection::Next => {
            let start = cursor_index.map_or(0, |index| index.saturating_add(1));
            (start, start.saturating_add(requested).min(matches.len()))
        }
        PageDirection::Previous => {
            let end = cursor_index.ok_or_else(|| {
                Error::InvalidCursor(
                    "previous direction requires a cursor that is still in the mempool".to_string(),
                )
            })?;
            (end.saturating_sub(requested), end)
        }
    };
    let page = &matches[start..end];
    let has_rows = !page.is_empty();
    let has_prev = has_rows && start > 0;
    let has_next = has_rows && end < matches.len();
    let previous_cursor = page
        .first()
        .filter(|_| has_prev)
        .map(|entry| MempoolCursor::new(entry.position, filter_tags).encode());
    let next_cursor = page
        .last()
        .filter(|_| has_next)
        .map(|entry| MempoolCursor::new(entry.position, filter_tags).encode());
    let transactions = page.iter().map(|entry| entry.item.clone()).collect();

    Ok(MempoolTransactionsResponse {
        summary,
        transactions,
        pagination: TransactionsPagination {
            limit,
            has_next,
            has_prev,
            next_cursor,
            prev_cursor: previous_cursor,
        },
    })
}

pub(super) fn transaction_details(
    transaction: &VerifiedUnminedTx,
    transactions: &[VerifiedUnminedTx],
    dependencies: &TransactionDependencies,
    network: &Network,
) -> Result<PendingTransactionDetails, Error> {
    let fee_zat = fee_zat(transaction)?;
    let raw_transaction = transaction.transaction.transaction().as_ref();
    let classification = classify_unmined_transaction(
        raw_transaction,
        fee_zat,
        transaction.spent_outputs.as_slice(),
    )?;
    let spent_outpoints = transactions
        .iter()
        .flat_map(|transaction| transaction.transaction.transaction().inputs())
        .filter_map(Input::outpoint)
        .collect::<HashSet<_>>();

    Ok(PendingTransactionDetails {
        transaction: transaction_data(transaction, network, &spent_outpoints)?,
        status: TransactionStatus::Pending,
        block_height: None,
        block_hash: None,
        block_time: None,
        transaction_index: None,
        confirmations: 0,
        canonical: false,
        finalized: false,
        kind: classification.kind,
        pool: classification.pool,
        flow: classification.flow,
        amount_zat: classification.amount_zat.map(|amount| amount.to_string()),
        joinsplit_count: count_u32(raw_transaction.joinsplit_count(), "Sprout JoinSplit count")?,
        coinbase_hex: None,
        mempool: mempool_metadata(transaction, dependencies),
    })
}

fn positioned_list_item(
    transaction: &VerifiedUnminedTx,
    dependencies: &TransactionDependencies,
    fee_zat: u64,
    classification: TransactionClassification,
) -> Result<PositionedTransaction, Error> {
    let raw_transaction = transaction.transaction.transaction().as_ref();

    let txid = raw_transaction.hash();
    let first_seen = transaction.time.map(|time| time.timestamp());
    Ok(PositionedTransaction {
        position: MempoolPosition {
            first_seen: first_seen.unwrap_or(i64::MIN),
            txid,
        },
        item: MempoolTransactionListItem {
            txid: txid.to_string(),
            status: TransactionStatus::Pending,
            first_seen: first_seen.map(|time| time.to_string()),
            entry_height: transaction.height.map(|height| height.0.to_string()),
            size: count_u32(
                transaction.transaction.size(),
                "serialized transaction size",
            )?,
            kind: classification.kind,
            pool: classification.pool,
            flow: classification.flow,
            amount_zat: classification.amount_zat.map(|amount| amount.to_string()),
            fee: fee_zat.to_string(),
            vin_count: count_u32(raw_transaction.inputs().len(), "transparent input count")?,
            vout_count: count_u32(raw_transaction.outputs().len(), "transparent output count")?,
            shielded_value_balance: classification.shielded_value_balance_zat.to_string(),
            value_balance_sapling: raw_transaction
                .sapling_value_balance()
                .sapling_amount()
                .zatoshis()
                .to_string(),
            value_balance_orchard: raw_transaction
                .orchard_value_balance()
                .orchard_amount()
                .zatoshis()
                .to_string(),
            value_balance_ironwood: raw_transaction
                .ironwood_value_balance()
                .ironwood_amount()
                .zatoshis()
                .to_string(),
            joinsplit_count: count_u32(
                raw_transaction.joinsplit_count(),
                "Sprout JoinSplit count",
            )?,
            sapling_spend_count: count_u32(
                raw_transaction.sapling_spends_per_anchor().count(),
                "Sapling spend count",
            )?,
            sapling_output_count: count_u32(
                raw_transaction.sapling_outputs().count(),
                "Sapling output count",
            )?,
            orchard_actions: count_u32(
                raw_transaction.orchard_actions().count(),
                "Orchard action count",
            )?,
            ironwood_actions: count_u32(
                raw_transaction.ironwood_actions().count(),
                "Ironwood action count",
            )?,
            depends: direct_dependencies(txid, dependencies),
        },
    })
}

fn record_summary_transaction(
    summary: &mut MempoolTransactionSummary,
    kind: TransactionKind,
) -> Result<(), Error> {
    let next_total = summary
        .total
        .checked_add(1)
        .ok_or_else(|| Error::Calculation("mempool transaction count exceeds u64".to_string()))?;
    let (current_count, label) = match kind {
        TransactionKind::Shielded => (summary.shielded, "shielded"),
        TransactionKind::Transparent => (summary.transparent, "transparent"),
        TransactionKind::Coinbase => {
            return Err(Error::Calculation(
                "coinbase transaction cannot be present in the mempool".to_string(),
            ))
        }
    };
    let next_count = current_count.checked_add(1).ok_or_else(|| {
        Error::Calculation(format!("mempool {label} transaction count exceeds u64"))
    })?;
    summary.total = next_total;
    match kind {
        TransactionKind::Shielded => summary.shielded = next_count,
        TransactionKind::Transparent => summary.transparent = next_count,
        TransactionKind::Coinbase => unreachable!("coinbase returns before updating summary"),
    }
    Ok(())
}

fn transaction_data(
    verified: &VerifiedUnminedTx,
    network: &Network,
    spent_outpoints: &HashSet<OutPoint>,
) -> Result<TransactionData, Error> {
    let transaction = verified.transaction.transaction().as_ref();
    let txid = transaction.hash();
    let expected_inputs = transaction
        .inputs()
        .iter()
        .filter_map(Input::outpoint)
        .count();
    if expected_inputs != verified.spent_outputs.len() {
        return Err(Error::Calculation(format!(
            "mempool transaction has {expected_inputs} transparent inputs but {} resolved outputs",
            verified.spent_outputs.len()
        )));
    }

    let inputs = transaction
        .inputs()
        .iter()
        .filter_map(|input| match input {
            Input::Coinbase { .. } => None,
            Input::PrevOut {
                outpoint,
                unlock_script,
                sequence,
            } => Some((outpoint, unlock_script, sequence)),
        })
        .zip(verified.spent_outputs.iter())
        .map(|((outpoint, unlock_script, sequence), output)| {
            zakura_indexer::BlockTransactionInput {
                previous_transaction_id: outpoint.hash.to_string(),
                previous_output_index: outpoint.index,
                address: output.address(network).map(|address| address.to_string()),
                value: output.value().zatoshis().to_string(),
                script_sig: hex::encode(unlock_script.as_raw_bytes()),
                sequence: *sequence,
            }
        })
        .collect();
    let outputs = transaction
        .outputs()
        .iter()
        .enumerate()
        .map(|(output_index, output)| {
            let output_index = count_u32(output_index, "transaction output index")?;
            let outpoint = OutPoint {
                hash: txid,
                index: output_index,
            };
            Ok(zakura_indexer::BlockTransactionOutput {
                transaction_id: txid.to_string(),
                address: output.address(network).map(|address| address.to_string()),
                value: output.value().zatoshis().to_string(),
                output_index,
                script_pub_key: hex::encode(output.lock_script.as_raw_bytes()),
                spent: spent_outpoints.contains(&outpoint),
            })
        })
        .collect::<Result<Vec<_>, Error>>()?;
    let total_input = verified
        .spent_outputs
        .iter()
        .try_fold(0_i64, |total, output| {
            total.checked_add(output.value().zatoshis()).ok_or_else(|| {
                Error::Calculation("transaction input total exceeds i64".to_string())
            })
        })?;
    let total_output = transaction
        .outputs()
        .iter()
        .try_fold(0_i64, |total, output| {
            total.checked_add(output.value().zatoshis()).ok_or_else(|| {
                Error::Calculation("transaction output total exceeds i64".to_string())
            })
        })?;
    let value_balance_transparent = total_input.checked_sub(total_output).ok_or_else(|| {
        Error::Calculation("transparent transaction value balance exceeds i64".to_string())
    })?;
    let value_balance_sapling = transaction.sapling_value_balance().sapling_amount();
    let value_balance_orchard = transaction.orchard_value_balance().orchard_amount();
    let value_balance_ironwood = transaction.ironwood_value_balance().ironwood_amount();
    let value_balance = [
        value_balance_sapling.zatoshis(),
        value_balance_orchard.zatoshis(),
        value_balance_ironwood.zatoshis(),
    ]
    .into_iter()
    .try_fold(0_i64, |total, value| {
        total
            .checked_add(value)
            .ok_or_else(|| Error::Calculation("transaction value balance exceeds i64".to_string()))
    })?;
    let sapling_spend_count = count_u32(
        transaction.sapling_spends_per_anchor().count(),
        "Sapling spend count",
    )?;
    let sapling_output_count = count_u32(
        transaction.sapling_outputs().count(),
        "Sapling output count",
    )?;
    let orchard_actions = count_u32(
        transaction.orchard_actions().count(),
        "Orchard action count",
    )?;
    let ironwood_actions = count_u32(
        transaction.ironwood_actions().count(),
        "Ironwood action count",
    )?;
    let serialized = transaction
        .zcash_serialize_to_vec()
        .map_err(|error| Error::Calculation(error.to_string()))?;

    Ok(TransactionData {
        txid: txid.to_string(),
        hex: hex::encode(&serialized),
        size: count_u32(serialized.len(), "serialized transaction size")?,
        version: transaction.version(),
        version_group_id: transaction
            .version_group_id()
            .map(|id| hex::encode(id.to_le_bytes())),
        lock_time: transaction.raw_lock_time().to_string(),
        expiry_height: transaction
            .expiry_height()
            .map(|height| height.0.to_string()),
        auth_digest: transaction.auth_digest().map(|digest| digest.to_string()),
        overwintered: transaction.is_overwintered(),
        vin_count: count_u32(transaction.inputs().len(), "transparent input count")?,
        vout_count: count_u32(transaction.outputs().len(), "transparent output count")?,
        value_balance: value_balance.to_string(),
        value_balance_transparent: value_balance_transparent.to_string(),
        value_balance_sapling: value_balance_sapling.zatoshis().to_string(),
        value_balance_orchard: value_balance_orchard.zatoshis().to_string(),
        value_balance_ironwood: value_balance_ironwood.zatoshis().to_string(),
        has_sapling: sapling_spend_count > 0 || sapling_output_count > 0,
        has_orchard: orchard_actions > 0,
        has_ironwood: ironwood_actions > 0,
        has_sprout: transaction.joinsplit_count() > 0,
        sapling_spend_count,
        sapling_output_count,
        orchard_actions,
        ironwood_actions,
        fee: fee_zat(verified)?.to_string(),
        total_input: total_input.to_string(),
        total_output: total_output.to_string(),
        is_coinbase: false,
        inputs,
        outputs,
    })
}

fn mempool_metadata(
    transaction: &VerifiedUnminedTx,
    dependencies: &TransactionDependencies,
) -> MempoolTransactionMetadata {
    let txid = transaction.transaction.id().mined_id();
    MempoolTransactionMetadata {
        first_seen: transaction.time.map(|time| time.timestamp().to_string()),
        entry_height: transaction.height.map(|height| height.0.to_string()),
        depends: direct_dependencies(txid, dependencies),
    }
}

fn direct_dependencies(
    txid: TransactionHash,
    dependencies: &TransactionDependencies,
) -> Vec<String> {
    let mut dependencies = dependencies
        .direct_dependencies(&txid)
        .into_iter()
        .map(|txid| txid.to_string())
        .collect::<Vec<_>>();
    dependencies.sort();
    dependencies
}

fn fee_zat(transaction: &VerifiedUnminedTx) -> Result<u64, Error> {
    u64::try_from(transaction.miner_fee.zatoshis())
        .map_err(|_| Error::Calculation("mempool transaction fee must be non-negative".to_string()))
}

fn count_u32(value: usize, name: &str) -> Result<u32, Error> {
    u32::try_from(value).map_err(|_| Error::Calculation(format!("{name} exceeds u32")))
}

fn filter_tags(request: &GetTransactionsRequest) -> Result<[u8; 4], Error> {
    let kind = match request.kind {
        TransactionKindFilter::All => 0,
        TransactionKindFilter::Shielded => 1,
        TransactionKindFilter::Transparent => 2,
        TransactionKindFilter::Coinbase => 3,
    };
    let flow = match request.flow {
        ShieldedFlowFilter::All => 0,
        ShieldedFlowFilter::Shield => 1,
        ShieldedFlowFilter::Deshield => 2,
        ShieldedFlowFilter::FullyShielded => 3,
        ShieldedFlowFilter::Complex => 4,
    };
    let pool = match request.pool {
        ShieldedPoolFilter::All => 0,
        ShieldedPoolFilter::Sprout => 1,
        ShieldedPoolFilter::Sapling => 2,
        ShieldedPoolFilter::Orchard => 3,
        ShieldedPoolFilter::Ironwood => 4,
        ShieldedPoolFilter::Mixed => 5,
    };
    let amount = match request.min_zat {
        0 => 0,
        1_000_000_000 => 1,
        10_000_000_000 => 2,
        100_000_000_000 => 3,
        unsupported => {
            return Err(Error::InvalidQuery(format!(
                "unsupported mempool minimum amount: {unsupported}"
            )))
        }
    };
    Ok([kind, flow, pool, amount])
}

impl MempoolCursor {
    fn new(position: MempoolPosition, filter_tags: [u8; 4]) -> Self {
        Self {
            position,
            filter_tags,
        }
    }

    fn encode(self) -> String {
        let mut bytes = [0; CURSOR_BYTE_LENGTH];
        bytes[..8].copy_from_slice(&self.position.first_seen.to_be_bytes());
        bytes[8..40].copy_from_slice(&self.position.txid.0);
        bytes[40..].copy_from_slice(&self.filter_tags);
        URL_SAFE_NO_PAD.encode(bytes)
    }

    fn decode(encoded: &str) -> Result<Self, Error> {
        let bytes = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| Error::InvalidCursor("cursor is not valid URL-safe base64".to_string()))?;
        if bytes.len() != CURSOR_BYTE_LENGTH {
            return Err(Error::InvalidCursor(format!(
                "decoded mempool cursor must be {CURSOR_BYTE_LENGTH} bytes"
            )));
        }

        Ok(Self {
            position: MempoolPosition {
                first_seen: i64::from_be_bytes(bytes[..8].try_into().map_err(|_| {
                    Error::InvalidCursor("cursor timestamp must be 8 bytes".to_string())
                })?),
                txid: TransactionHash(bytes[8..40].try_into().map_err(|_| {
                    Error::InvalidCursor("cursor transaction ID must be 32 bytes".to_string())
                })?),
            },
            filter_tags: bytes[40..].try_into().map_err(|_| {
                Error::InvalidCursor("cursor filter selector must be 4 bytes".to_string())
            })?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_round_trips_position_and_filters() {
        let cursor = MempoolCursor::new(
            MempoolPosition {
                first_seen: 1_790_605_520,
                txid: TransactionHash([0x42; 32]),
            },
            [1, 2, 3, 0],
        );

        assert_eq!(MempoolCursor::decode(&cursor.encode()).unwrap(), cursor);
    }

    #[test]
    fn empty_mempool_uses_the_shared_pagination_contract() {
        let response = transactions_page(
            Vec::new(),
            &TransactionDependencies::default(),
            &GetTransactionsRequest::default(),
        )
        .unwrap();

        assert_eq!(response.summary, MempoolTransactionSummary::default());
        assert!(response.transactions.is_empty());
        assert_eq!(response.pagination.limit, DEFAULT_QUERY_LIMIT);
        assert!(!response.pagination.has_next);
        assert!(!response.pagination.has_prev);
        assert_eq!(response.pagination.next_cursor, None);
        assert_eq!(response.pagination.prev_cursor, None);
    }

    #[test]
    fn previous_mempool_page_requires_a_cursor() {
        let request = GetTransactionsRequest {
            direction: PageDirection::Previous,
            ..Default::default()
        };

        let result = transactions_page(Vec::new(), &TransactionDependencies::default(), &request);

        assert!(matches!(result, Err(Error::InvalidCursor(_))));
    }

    #[test]
    fn summary_counts_pending_transaction_kinds() {
        let mut summary = MempoolTransactionSummary::default();

        record_summary_transaction(&mut summary, TransactionKind::Shielded).unwrap();
        record_summary_transaction(&mut summary, TransactionKind::Transparent).unwrap();
        record_summary_transaction(&mut summary, TransactionKind::Shielded).unwrap();

        assert_eq!(summary.total, 3);
        assert_eq!(summary.shielded, 2);
        assert_eq!(summary.transparent, 1);
        assert!(record_summary_transaction(&mut summary, TransactionKind::Coinbase).is_err());
        assert_eq!(summary.total, 3);
    }
}
