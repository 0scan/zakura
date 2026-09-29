//! Atomic canonical transaction writes and rollback preparation.

use std::collections::HashMap;

use rocksdb::WriteBatch;
use zakura_chain::{
    block::Height,
    serialization::ZcashSerialize,
    transaction::{Hash as TransactionHash, Transaction},
    transparent::{OutPoint, Utxo},
};

use crate::{
    database::DatabaseColumn,
    models::{TransactionPosition, TransactionRecord},
    types::{ShieldedFlow, ShieldedPool, TransactionKind},
    Error, Indexer,
};

use super::{
    classify::{public_flow_amount, shielded_flow, shielded_pool, transaction_kind},
    disk_format::{
        decode_ordered_position, decode_transaction_hash, decode_transaction_record,
        encode_transaction_hash, encode_transaction_record, shielded_order_key,
        transaction_height_prefix, transaction_kind_order_key, transaction_position_key,
        transaction_record_key,
    },
    filter::{AmountFilter, ShieldedFlowFilter, ShieldedPoolFilter},
};

impl Indexer {
    /// Adds one validated transaction and every list index derived from it to `batch`.
    ///
    /// Returns the non-negative transaction fee for aggregation into the block record.
    pub(crate) fn prepare_transaction(
        &self,
        batch: &mut WriteBatch,
        height: Height,
        transaction_index: usize,
        transaction: &Transaction,
        spent_utxos: &HashMap<OutPoint, Utxo>,
    ) -> Result<u64, Error> {
        let is_coinbase = transaction.is_coinbase();
        if is_coinbase != (transaction_index == 0) {
            return Err(Error::Calculation(
                "a canonical block must contain exactly one coinbase at transaction index zero"
                    .to_string(),
            ));
        }

        let (fee_zat, transparent_value_balance_zat) = if is_coinbase {
            (0, 0)
        } else {
            let value_balance = transaction
                .value_balance(spent_utxos)
                .map_err(|error| Error::Calculation(error.to_string()))?;
            let fee = value_balance
                .remaining_transaction_value()
                .map_err(|error| Error::Calculation(error.to_string()))?
                .zatoshis();
            let fee_zat = u64::try_from(fee).map_err(|_| {
                Error::Calculation("transaction fee must be non-negative".to_string())
            })?;
            (fee_zat, value_balance.transparent_amount().zatoshis())
        };

        let record = TransactionRecord {
            position: TransactionPosition {
                height,
                transaction_index: count_u32(transaction_index, "transaction index")?,
            },
            serialized_size: count_u32(
                transaction.zcash_serialized_size(),
                "serialized transaction size",
            )?,
            fee_zat,
            transparent_value_balance_zat,
            sapling_value_balance_zat: transaction
                .sapling_value_balance()
                .sapling_amount()
                .zatoshis(),
            orchard_value_balance_zat: transaction
                .orchard_value_balance()
                .orchard_amount()
                .zatoshis(),
            ironwood_value_balance_zat: transaction
                .ironwood_value_balance()
                .ironwood_amount()
                .zatoshis(),
            transparent_input_count: if is_coinbase {
                0
            } else {
                count_u32(transaction.inputs().len(), "transparent input count")?
            },
            transparent_output_count: count_u32(
                transaction.outputs().len(),
                "transparent output count",
            )?,
            joinsplit_count: count_u32(transaction.joinsplit_count(), "Sprout JoinSplit count")?,
            sapling_spend_count: count_u32(
                transaction.sapling_spends_per_anchor().count(),
                "Sapling spend count",
            )?,
            sapling_output_count: count_u32(
                transaction.sapling_outputs().count(),
                "Sapling output count",
            )?,
            orchard_action_count: count_u32(
                transaction.orchard_actions().count(),
                "Orchard action count",
            )?,
            ironwood_action_count: count_u32(
                transaction.ironwood_actions().count(),
                "Ironwood action count",
            )?,
        };

        self.insert_transaction(batch, transaction.hash(), record)?;
        Ok(fee_zat)
    }

    /// Removes all canonical transaction mappings for `height` from `batch`.
    pub(crate) fn prepare_transaction_rollback(
        &self,
        batch: &mut WriteBatch,
        height: Height,
    ) -> Result<(), Error> {
        let entries = self.database.scan_prefix(
            DatabaseColumn::CanonicalTransactionPositions,
            &transaction_height_prefix(height),
        )?;

        for (position_key, txid_bytes) in entries {
            let position = decode_ordered_position(&position_key)?;
            let txid = decode_transaction_hash(&txid_bytes)?;
            let record = self.transaction_record(txid)?.ok_or_else(|| {
                Error::CorruptData(format!(
                    "missing transaction record for canonical txid {txid}"
                ))
            })?;
            if record.position != position {
                return Err(Error::CorruptData(format!(
                    "transaction {txid} record position does not match its canonical position"
                )));
            }

            self.delete_transaction_indexes(batch, record)?;
            self.database.delete(
                batch,
                DatabaseColumn::CanonicalTransactionPositions,
                position_key,
            );
            self.database.delete(
                batch,
                DatabaseColumn::TransactionRecords,
                transaction_record_key(txid),
            );
        }

        Ok(())
    }

    pub(super) fn transaction_record(
        &self,
        txid: TransactionHash,
    ) -> Result<Option<TransactionRecord>, Error> {
        self.database
            .get(
                DatabaseColumn::TransactionRecords,
                transaction_record_key(txid),
            )?
            .map(|bytes| decode_transaction_record(&bytes))
            .transpose()
    }

    pub(super) fn insert_transaction(
        &self,
        batch: &mut WriteBatch,
        txid: TransactionHash,
        record: TransactionRecord,
    ) -> Result<(), Error> {
        self.database.insert(
            batch,
            DatabaseColumn::TransactionRecords,
            transaction_record_key(txid),
            encode_transaction_record(record),
        );
        self.database.insert(
            batch,
            DatabaseColumn::CanonicalTransactionPositions,
            transaction_position_key(record.position),
            encode_transaction_hash(txid),
        );
        self.database.insert(
            batch,
            DatabaseColumn::TransactionKindOrder,
            transaction_kind_order_key(transaction_kind(&record), record.position),
            b"",
        );
        self.insert_shielded_indexes(batch, record)
    }

    fn insert_shielded_indexes(
        &self,
        batch: &mut WriteBatch,
        record: TransactionRecord,
    ) -> Result<(), Error> {
        if transaction_kind(&record) != TransactionKind::Shielded {
            return Ok(());
        }

        for flow in flow_filters(record)? {
            for pool in pool_filters(record)? {
                for amount in amount_filters(record)?.into_iter().flatten() {
                    self.database.insert(
                        batch,
                        DatabaseColumn::ShieldedTransactionOrder,
                        shielded_order_key(flow, pool, amount, record.position),
                        b"",
                    );
                }
            }
        }
        Ok(())
    }

    fn delete_transaction_indexes(
        &self,
        batch: &mut WriteBatch,
        record: TransactionRecord,
    ) -> Result<(), Error> {
        self.database.delete(
            batch,
            DatabaseColumn::TransactionKindOrder,
            transaction_kind_order_key(transaction_kind(&record), record.position),
        );
        if transaction_kind(&record) != TransactionKind::Shielded {
            return Ok(());
        }

        for flow in flow_filters(record)? {
            for pool in pool_filters(record)? {
                for amount in amount_filters(record)?.into_iter().flatten() {
                    self.database.delete(
                        batch,
                        DatabaseColumn::ShieldedTransactionOrder,
                        shielded_order_key(flow, pool, amount, record.position),
                    );
                }
            }
        }
        Ok(())
    }
}

fn flow_filters(record: TransactionRecord) -> Result<[ShieldedFlowFilter; 2], Error> {
    let flow = match shielded_flow(&record)?.ok_or_else(|| {
        Error::Calculation("shielded transaction is missing a flow classification".to_string())
    })? {
        ShieldedFlow::Shield => ShieldedFlowFilter::Shield,
        ShieldedFlow::Deshield => ShieldedFlowFilter::Deshield,
        ShieldedFlow::FullyShielded => ShieldedFlowFilter::FullyShielded,
        ShieldedFlow::Complex => ShieldedFlowFilter::Complex,
    };
    Ok([ShieldedFlowFilter::All, flow])
}

fn pool_filters(record: TransactionRecord) -> Result<[ShieldedPoolFilter; 2], Error> {
    let pool = match shielded_pool(&record).ok_or_else(|| {
        Error::Calculation("shielded transaction is missing a pool classification".to_string())
    })? {
        ShieldedPool::Sprout => ShieldedPoolFilter::Sprout,
        ShieldedPool::Sapling => ShieldedPoolFilter::Sapling,
        ShieldedPool::Orchard => ShieldedPoolFilter::Orchard,
        ShieldedPool::Ironwood => ShieldedPoolFilter::Ironwood,
        ShieldedPool::Mixed => ShieldedPoolFilter::Mixed,
    };
    Ok([ShieldedPoolFilter::All, pool])
}

fn amount_filters(record: TransactionRecord) -> Result<[Option<AmountFilter>; 4], Error> {
    let mut filters = [Some(AmountFilter::Any), None, None, None];
    let Some(amount) = public_flow_amount(&record)? else {
        return Ok(filters);
    };
    for (index, filter) in [
        AmountFilter::AtLeastTenZec,
        AmountFilter::AtLeastOneHundredZec,
        AmountFilter::AtLeastOneThousandZec,
    ]
    .into_iter()
    .enumerate()
    {
        if amount
            >= filter
                .minimum_zat()
                .expect("non-any amount filters have a threshold")
        {
            filters[index + 1] = Some(filter);
        }
    }
    Ok(filters)
}

fn count_u32(value: usize, name: &str) -> Result<u32, Error> {
    u32::try_from(value).map_err(|_| Error::Calculation(format!("{name} exceeds u32")))
}
