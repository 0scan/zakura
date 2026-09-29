//! Canonical newest-first transaction queries over materialized filter indexes.

use std::collections::HashMap;

use zakura_chain::{
    block::{Hash, Height},
    transaction::Hash as TransactionHash,
};

use crate::{
    database::DatabaseColumn,
    models::{TransactionPosition, TransactionRecord},
    types::{
        PageDirection, TransactionKind, TransactionListItem, TransactionsPagination,
        TransactionsResponse,
    },
    Error, Indexer,
};

use super::{
    classify::{
        public_flow_amount, shielded_flow, shielded_pool, shielded_value_balance, transaction_kind,
    },
    cursor::TransactionCursor,
    disk_format::{
        decode_ordered_position, decode_transaction_hash, newest_shielded_order_key,
        newest_transaction_kind_order_key, newest_transaction_position_key, shielded_order_key,
        shielded_order_prefix, transaction_kind_order_key, transaction_kind_order_prefix,
        transaction_position_key,
    },
    filter::{TransactionKindFilter, TransactionQuery},
};

const DEFAULT_QUERY_LIMIT: u32 = 25;
const MAX_QUERY_LIMIT: u32 = 100;

impl Indexer {
    /// Returns the canonical transaction hash stored at `position`.
    pub(crate) fn canonical_transaction_hash(
        &self,
        position: TransactionPosition,
    ) -> Result<Option<TransactionHash>, Error> {
        self.database
            .get(
                DatabaseColumn::CanonicalTransactionPositions,
                transaction_position_key(position),
            )?
            .map(|bytes| decode_transaction_hash(&bytes))
            .transpose()
    }

    /// Returns canonical transaction summaries from newest to oldest.
    pub async fn transactions_page(
        &self,
        query: TransactionQuery,
        limit: Option<u32>,
        cursor: Option<String>,
        direction: PageDirection,
    ) -> Result<TransactionsResponse, Error> {
        let query = query.validate().map_err(Error::InvalidQuery)?;
        let indexer = self.clone();
        tokio::task::spawn_blocking(move || {
            indexer.recent_transactions_blocking(query, limit, cursor, direction)
        })
        .await
        .map_err(|error| Error::Task(error.to_string()))?
    }

    fn recent_transactions_blocking(
        &self,
        query: TransactionQuery,
        limit: Option<u32>,
        cursor: Option<String>,
        direction: PageDirection,
    ) -> Result<TransactionsResponse, Error> {
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
            if !cursor.matches(query) {
                return Err(Error::InvalidCursor(
                    "transaction cursor was created for different filters".to_string(),
                ));
            }
            if self.canonical_block_hash(cursor.position.height)? != Some(cursor.block_hash) {
                return Err(Error::InvalidCursor(
                    "cursor block is no longer on the indexed canonical chain".to_string(),
                ));
            }
        }

        let source = QuerySource::new(query, cursor.map(|cursor| cursor.position));
        if cursor.is_some()
            && self
                .database
                .get(source.column, &source.start_key)?
                .is_none()
        {
            return Err(Error::InvalidCursor(
                "cursor transaction no longer matches the requested filters".to_string(),
            ));
        }

        let requested = usize::try_from(limit)
            .map_err(|_| Error::Calculation("query limit exceeds usize".to_string()))?;
        let scan_limit = requested
            .checked_add(2)
            .ok_or_else(|| Error::Calculation("transaction scan limit overflow".to_string()))?;
        let entries = match direction {
            PageDirection::Next => self.database.scan_prefix_reverse_from(
                source.column,
                &source.prefix,
                &source.start_key,
                scan_limit,
            )?,
            PageDirection::Previous => self.database.scan_prefix_forward_from(
                source.column,
                &source.prefix,
                &source.start_key,
                scan_limit,
            )?,
        };

        let mut entries: Vec<_> = entries
            .into_iter()
            .filter(|(key, _)| cursor.is_none() || key.as_slice() != source.start_key.as_slice())
            .collect();
        let has_more_in_direction = entries.len() > requested;
        entries.truncate(requested);
        if direction == PageDirection::Previous {
            entries.reverse();
        }

        let mut block_cache = HashMap::new();
        let mut matched = Vec::with_capacity(entries.len());
        for (key, value) in entries {
            let position = decode_ordered_position(&key)?;
            let txid = if source.column == DatabaseColumn::CanonicalTransactionPositions {
                decode_transaction_hash(&value)?
            } else {
                let txid = self
                    .database
                    .get(
                        DatabaseColumn::CanonicalTransactionPositions,
                        transaction_position_key(position),
                    )?
                    .ok_or_else(|| {
                        Error::CorruptData(format!(
                            "missing canonical transaction at height {}, index {}",
                            position.height.0, position.transaction_index
                        ))
                    })?;
                decode_transaction_hash(&txid)?
            };
            let record = self.transaction_record(txid)?.ok_or_else(|| {
                Error::CorruptData(format!(
                    "missing transaction record for canonical txid {txid}"
                ))
            })?;
            if record.position != position {
                return Err(Error::CorruptData(format!(
                    "transaction {txid} record position does not match its order index"
                )));
            }

            let (block_hash, block_time) =
                self.transaction_block_fields(position.height, &mut block_cache)?;
            let item = transaction_list_item(txid, block_hash, block_time, record)?;
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
                TransactionCursor::new(*position, *block_hash, query).encode()
            });
        let prev_cursor = matched
            .first()
            .filter(|_| has_prev)
            .map(|(position, block_hash, _)| {
                TransactionCursor::new(*position, *block_hash, query).encode()
            });
        let transactions = matched.into_iter().map(|(_, _, item)| item).collect();

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

    fn transaction_block_fields(
        &self,
        height: Height,
        cache: &mut HashMap<u32, (Hash, String)>,
    ) -> Result<(Hash, String), Error> {
        if let Some((hash, timestamp)) = cache.get(&height.0) {
            return Ok((*hash, timestamp.clone()));
        }

        let hash = self.canonical_block_hash(height)?.ok_or_else(|| {
            Error::CorruptData(format!(
                "missing canonical block hash at transaction height {}",
                height.0
            ))
        })?;
        let block = self.block_record(hash)?.ok_or_else(|| {
            Error::CorruptData(format!("missing block record for canonical hash {hash}"))
        })?;
        cache.insert(height.0, (hash, block.timestamp.clone()));
        Ok((hash, block.timestamp))
    }
}

struct QuerySource {
    column: DatabaseColumn,
    prefix: Vec<u8>,
    start_key: Vec<u8>,
}

impl QuerySource {
    fn new(query: TransactionQuery, cursor: Option<TransactionPosition>) -> Self {
        if query.uses_shielded_index() {
            let prefix = shielded_order_prefix(query.flow, query.pool, query.amount).to_vec();
            let start_key = cursor.map_or_else(
                || newest_shielded_order_key(query.flow, query.pool, query.amount).to_vec(),
                |position| {
                    shielded_order_key(query.flow, query.pool, query.amount, position).to_vec()
                },
            );
            return Self {
                column: DatabaseColumn::ShieldedTransactionOrder,
                prefix,
                start_key,
            };
        }

        match query.kind {
            TransactionKindFilter::All => Self {
                column: DatabaseColumn::CanonicalTransactionPositions,
                prefix: Vec::new(),
                start_key: cursor.map_or_else(
                    || newest_transaction_position_key().to_vec(),
                    |position| transaction_position_key(position).to_vec(),
                ),
            },
            kind => {
                let kind = transaction_kind_from_filter(kind);
                Self {
                    column: DatabaseColumn::TransactionKindOrder,
                    prefix: transaction_kind_order_prefix(kind).to_vec(),
                    start_key: cursor.map_or_else(
                        || newest_transaction_kind_order_key(kind).to_vec(),
                        |position| transaction_kind_order_key(kind, position).to_vec(),
                    ),
                }
            }
        }
    }
}

fn transaction_kind_from_filter(filter: TransactionKindFilter) -> TransactionKind {
    match filter {
        TransactionKindFilter::Shielded => TransactionKind::Shielded,
        TransactionKindFilter::Transparent => TransactionKind::Transparent,
        TransactionKindFilter::Coinbase => TransactionKind::Coinbase,
        TransactionKindFilter::All => {
            unreachable!("all transactions use the canonical position index")
        }
    }
}

fn transaction_list_item(
    txid: TransactionHash,
    block_hash: Hash,
    block_time: String,
    record: TransactionRecord,
) -> Result<TransactionListItem, Error> {
    let amount_zat = public_flow_amount(&record)?;
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
        amount_zat: amount_zat.map(|amount| amount.to_string()),
        fee: record.fee_zat.to_string(),
        vin_count: record.transparent_input_count,
        vout_count: record.transparent_output_count,
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

#[cfg(test)]
mod tests {
    use rocksdb::WriteBatch;
    use zakura_chain::{
        block::{Hash, Height},
        parameters::Network,
        transaction::Hash as TransactionHash,
    };

    use super::*;
    use crate::{
        database::DatabaseColumn,
        models::IndexedBlockRecord,
        transactions::{AmountFilter, ShieldedFlowFilter, ShieldedPoolFilter},
        types::PageDirection,
    };

    #[tokio::test]
    async fn paginates_and_filters_canonical_transaction_indexes() {
        let indexer = Indexer::open_ephemeral(Network::Mainnet)
            .expect("ephemeral transaction index should open");
        put_test_transaction(
            &indexer,
            coinbase_record(Height(1)),
            TransactionHash([1; 32]),
        );
        put_test_transaction(&indexer, shield_record(Height(1)), TransactionHash([2; 32]));
        put_test_transaction(
            &indexer,
            coinbase_record(Height(2)),
            TransactionHash([3; 32]),
        );

        let first = indexer
            .transactions_page(
                TransactionQuery::default(),
                Some(2),
                None,
                PageDirection::Next,
            )
            .await
            .expect("first transaction page should load");
        assert_eq!(first.transactions.len(), 2);
        assert_eq!(first.transactions[0].block_height, "2");
        assert_eq!(
            first.transactions[1].amount_zat.as_deref(),
            Some("2000000000")
        );
        assert!(first.pagination.has_next);
        assert!(!first.pagination.has_prev);

        let second = indexer
            .transactions_page(
                TransactionQuery::default(),
                Some(2),
                first.pagination.next_cursor.clone(),
                PageDirection::Next,
            )
            .await
            .expect("second transaction page should load");
        assert_eq!(second.transactions.len(), 1);
        assert_eq!(second.transactions[0].block_height, "1");
        assert_eq!(second.transactions[0].transaction_index, 0);
        assert!(!second.pagination.has_next);
        assert!(second.pagination.has_prev);

        let previous = indexer
            .transactions_page(
                TransactionQuery::default(),
                Some(2),
                second.pagination.prev_cursor,
                PageDirection::Previous,
            )
            .await
            .expect("newer transaction page should load");
        assert_eq!(previous.transactions, first.transactions);
        assert!(!previous.pagination.has_prev);
        assert!(previous.pagination.has_next);

        let shielded_query = TransactionQuery {
            kind: TransactionKindFilter::Shielded,
            flow: ShieldedFlowFilter::Shield,
            pool: ShieldedPoolFilter::Ironwood,
            amount: AmountFilter::AtLeastOneBillionZat,
        };
        let shielded = indexer
            .transactions_page(shielded_query, None, None, PageDirection::Next)
            .await
            .expect("shielded filter intersection should load");
        assert_eq!(shielded.transactions.len(), 1);
        assert_eq!(shielded.transactions[0].kind, TransactionKind::Shielded);
        assert_eq!(
            shielded.transactions[0].amount_zat.as_deref(),
            Some("2000000000")
        );

        let mismatched_cursor = indexer
            .transactions_page(
                shielded_query,
                None,
                first.pagination.next_cursor,
                PageDirection::Next,
            )
            .await
            .expect_err("a cursor must remain bound to its original filters");
        assert!(matches!(mismatched_cursor, Error::InvalidCursor(_)));
    }

    fn put_test_transaction(indexer: &Indexer, record: TransactionRecord, txid: TransactionHash) {
        let height = record.position.height;
        let hash_byte = u8::try_from(height.0).expect("test height fits in u8");
        let block_hash = Hash([hash_byte; 32]);
        let block = IndexedBlockRecord {
            height,
            timestamp: 1_700_000_000 + i64::from(height.0),
            transaction_count: record.position.transaction_index + 1,
            serialized_size: 1_000,
            difficulty: "1.000000".to_string(),
            miner_address: None,
            total_fees_zat: 0,
            miner_pool: "Unknown".to_string(),
        };
        let mut batch = WriteBatch::default();
        indexer.database.insert(
            &mut batch,
            DatabaseColumn::BlockRecords,
            block_hash.0,
            serde_json::to_vec(&block).expect("test block record should serialize"),
        );
        indexer.database.insert(
            &mut batch,
            DatabaseColumn::CanonicalBlockHashes,
            height.0.to_be_bytes(),
            block_hash.0,
        );
        indexer
            .insert_transaction(&mut batch, txid, record)
            .expect("test transaction should be indexed");
        indexer
            .database
            .write(batch)
            .expect("test transaction batch should commit");
    }

    fn coinbase_record(height: Height) -> TransactionRecord {
        TransactionRecord {
            position: TransactionPosition {
                height,
                transaction_index: 0,
            },
            serialized_size: 100,
            fee_zat: 0,
            transparent_value_balance_zat: 0,
            sapling_value_balance_zat: 0,
            orchard_value_balance_zat: 0,
            ironwood_value_balance_zat: 0,
            transparent_input_count: 0,
            transparent_output_count: 1,
            joinsplit_count: 0,
            sapling_spend_count: 0,
            sapling_output_count: 0,
            orchard_action_count: 0,
            ironwood_action_count: 0,
        }
    }

    fn shield_record(height: Height) -> TransactionRecord {
        TransactionRecord {
            position: TransactionPosition {
                height,
                transaction_index: 1,
            },
            serialized_size: 200,
            fee_zat: 10_000,
            transparent_value_balance_zat: 2_000_010_000,
            sapling_value_balance_zat: 0,
            orchard_value_balance_zat: 0,
            ironwood_value_balance_zat: -2_000_000_000,
            transparent_input_count: 1,
            transparent_output_count: 0,
            joinsplit_count: 0,
            sapling_spend_count: 0,
            sapling_output_count: 0,
            orchard_action_count: 0,
            ironwood_action_count: 1,
        }
    }
}
