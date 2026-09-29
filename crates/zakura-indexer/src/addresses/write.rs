//! Atomic address summary and ordered-history updates.

use std::collections::HashMap;

use rocksdb::WriteBatch;
use zakura_chain::{
    block::Height,
    transaction::Transaction,
    transparent::{Address, OutPoint, Utxo},
};

use crate::{
    database::{decode_trailing_transaction_position, DatabaseColumn},
    models::{AddressEffect, AddressRecord, TransactionAddressEffects, TransactionPosition},
    Error, Indexer,
};

use super::disk_format::{
    address_order_key, address_order_prefix, address_record_key, decode_address_record,
    decode_transaction_address_effects, encode_address_record, encode_transaction_address_effects,
    transaction_address_effects_key,
};

pub(crate) type PendingAddressRecords = HashMap<Address, Option<AddressRecord>>;

impl Indexer {
    pub(crate) fn prepare_address_transaction(
        &self,
        batch: &mut WriteBatch,
        pending_records: &mut PendingAddressRecords,
        position: TransactionPosition,
        transaction: &Transaction,
        spent_utxos: &HashMap<OutPoint, Utxo>,
    ) -> Result<(), Error> {
        let effects = self.derive_address_effects(transaction, spent_utxos)?;
        if effects.effects.is_empty() {
            return Ok(());
        }

        self.database.insert(
            batch,
            DatabaseColumn::TransactionAddressEffects,
            transaction_address_effects_key(position),
            encode_transaction_address_effects(&effects)?,
        );

        for effect in effects.effects {
            let current = self.pending_address_record(pending_records, effect.address)?;
            let updated = match current {
                Some(mut record) => {
                    record.total_received_zat = record
                        .total_received_zat
                        .checked_add(effect.received_zat)
                        .ok_or_else(|| {
                            Error::Calculation("address received total exceeds u64".to_string())
                        })?;
                    record.total_sent_zat = record
                        .total_sent_zat
                        .checked_add(effect.sent_zat)
                        .ok_or_else(|| {
                            Error::Calculation("address sent total exceeds u64".to_string())
                        })?;
                    record.transaction_count =
                        record.transaction_count.checked_add(1).ok_or_else(|| {
                            Error::Calculation("address transaction count exceeds u64".to_string())
                        })?;
                    record.last_position = position;
                    if record.first_funding_position.is_none() && effect.received_zat > 0 {
                        record.first_funding_position = Some(position);
                    }
                    record
                }
                None => AddressRecord {
                    total_received_zat: effect.received_zat,
                    total_sent_zat: effect.sent_zat,
                    transaction_count: 1,
                    first_position: position,
                    last_position: position,
                    first_funding_position: (effect.received_zat > 0).then_some(position),
                },
            };

            self.database.insert(
                batch,
                DatabaseColumn::AddressRecords,
                address_record_key(effect.address),
                encode_address_record(updated)?,
            );
            self.database.insert(
                batch,
                DatabaseColumn::AddressTransactionOrder,
                address_order_key(effect.address, position),
                b"",
            );
            pending_records.insert(effect.address, Some(updated));
        }

        Ok(())
    }

    pub(crate) fn prepare_address_transaction_rollback(
        &self,
        batch: &mut WriteBatch,
        pending_records: &mut PendingAddressRecords,
        position: TransactionPosition,
        retained_tip: Option<Height>,
    ) -> Result<(), Error> {
        let Some(effects) = self.transaction_address_effects(position)? else {
            return Ok(());
        };

        for effect in effects.effects {
            let mut record = self
                .pending_address_record(pending_records, effect.address)?
                .ok_or_else(|| {
                    Error::CorruptData(format!(
                        "missing address record while rolling back {}",
                        effect.address
                    ))
                })?;
            record.total_received_zat = record
                .total_received_zat
                .checked_sub(effect.received_zat)
                .ok_or_else(|| {
                    Error::CorruptData("address received total underflowed on rollback".to_string())
                })?;
            record.total_sent_zat = record
                .total_sent_zat
                .checked_sub(effect.sent_zat)
                .ok_or_else(|| {
                    Error::CorruptData("address sent total underflowed on rollback".to_string())
                })?;
            record.transaction_count =
                record.transaction_count.checked_sub(1).ok_or_else(|| {
                    Error::CorruptData(
                        "address transaction count underflowed on rollback".to_string(),
                    )
                })?;

            self.database.delete(
                batch,
                DatabaseColumn::AddressTransactionOrder,
                address_order_key(effect.address, position),
            );

            if record.transaction_count == 0 {
                self.database.delete(
                    batch,
                    DatabaseColumn::AddressRecords,
                    address_record_key(effect.address),
                );
                pending_records.insert(effect.address, None);
                continue;
            }

            if record.last_position == position {
                record.last_position = self
                    .latest_retained_address_position(effect.address, retained_tip)?
                    .ok_or_else(|| {
                        Error::CorruptData(
                            "address count is non-zero but no retained transaction exists"
                                .to_string(),
                        )
                    })?;
            }
            if retained_tip.is_some_and(|tip| record.first_position.height > tip) {
                return Err(Error::CorruptData(
                    "address first position is above the retained chain tip".to_string(),
                ));
            }
            if retained_tip.is_some_and(|tip| {
                record
                    .first_funding_position
                    .is_some_and(|funding| funding.height > tip)
            }) {
                record.first_funding_position =
                    self.first_retained_funding_position(effect.address, retained_tip)?;
            }

            self.database.insert(
                batch,
                DatabaseColumn::AddressRecords,
                address_record_key(effect.address),
                encode_address_record(record)?,
            );
            pending_records.insert(effect.address, Some(record));
        }

        self.database.delete(
            batch,
            DatabaseColumn::TransactionAddressEffects,
            transaction_address_effects_key(position),
        );
        Ok(())
    }

    pub(crate) fn address_record(&self, address: Address) -> Result<Option<AddressRecord>, Error> {
        self.database
            .get(DatabaseColumn::AddressRecords, address_record_key(address))?
            .map(|bytes| decode_address_record(&bytes))
            .transpose()
    }

    pub(crate) fn transaction_address_effects(
        &self,
        position: TransactionPosition,
    ) -> Result<Option<TransactionAddressEffects>, Error> {
        self.database
            .get(
                DatabaseColumn::TransactionAddressEffects,
                transaction_address_effects_key(position),
            )?
            .map(|bytes| decode_transaction_address_effects(&bytes))
            .transpose()
    }

    fn pending_address_record(
        &self,
        pending_records: &PendingAddressRecords,
        address: Address,
    ) -> Result<Option<AddressRecord>, Error> {
        pending_records
            .get(&address)
            .copied()
            .map_or_else(|| self.address_record(address), Ok)
    }

    fn derive_address_effects(
        &self,
        transaction: &Transaction,
        spent_utxos: &HashMap<OutPoint, Utxo>,
    ) -> Result<TransactionAddressEffects, Error> {
        let mut changes = HashMap::<Address, (u64, u64)>::new();

        for outpoint in transaction
            .inputs()
            .iter()
            .filter_map(|input| input.outpoint())
        {
            let utxo = spent_utxos.get(&outpoint).ok_or_else(|| {
                Error::Calculation(format!(
                    "missing transparent output while indexing address effect {outpoint:?}"
                ))
            })?;
            let Some(address) = utxo.output.address(&self.network) else {
                continue;
            };
            let value = non_negative_zatoshis(utxo.output.value().zatoshis())?;
            let sent = &mut changes.entry(address).or_default().1;
            *sent = sent.checked_add(value).ok_or_else(|| {
                Error::Calculation("address sent value exceeds u64 in one transaction".to_string())
            })?;
        }

        for output in transaction.outputs() {
            let Some(address) = output.address(&self.network) else {
                continue;
            };
            let value = non_negative_zatoshis(output.value().zatoshis())?;
            let received = &mut changes.entry(address).or_default().0;
            *received = received.checked_add(value).ok_or_else(|| {
                Error::Calculation(
                    "address received value exceeds u64 in one transaction".to_string(),
                )
            })?;
        }

        let mut effects = changes
            .into_iter()
            .map(|(address, (received_zat, sent_zat))| AddressEffect {
                address,
                received_zat,
                sent_zat,
            })
            .collect::<Vec<_>>();
        effects.sort_by_key(|effect| effect.address.to_string());
        Ok(TransactionAddressEffects { effects })
    }

    fn latest_retained_address_position(
        &self,
        address: Address,
        retained_tip: Option<Height>,
    ) -> Result<Option<TransactionPosition>, Error> {
        let Some(retained_tip) = retained_tip else {
            return Ok(None);
        };
        let start = address_order_key(
            address,
            TransactionPosition {
                height: retained_tip,
                transaction_index: u32::MAX,
            },
        );
        let entries = self.database.scan_prefix_reverse_from(
            DatabaseColumn::AddressTransactionOrder,
            &address_order_prefix(address),
            &start,
            1,
        )?;
        entries
            .first()
            .map(|(key, _)| decode_trailing_transaction_position(key))
            .transpose()
    }

    fn first_retained_funding_position(
        &self,
        address: Address,
        retained_tip: Option<Height>,
    ) -> Result<Option<TransactionPosition>, Error> {
        let Some(retained_tip) = retained_tip else {
            return Ok(None);
        };
        for (key, _) in self.database.scan_prefix(
            DatabaseColumn::AddressTransactionOrder,
            &address_order_prefix(address),
        )? {
            let position = decode_trailing_transaction_position(&key)?;
            if position.height > retained_tip {
                break;
            }
            let effects = self.transaction_address_effects(position)?.ok_or_else(|| {
                Error::CorruptData(
                    "address order entry is missing transaction address effects".to_string(),
                )
            })?;
            if effects
                .effects
                .iter()
                .any(|effect| effect.address == address && effect.received_zat > 0)
            {
                return Ok(Some(position));
            }
        }
        Ok(None)
    }
}

fn non_negative_zatoshis(value: i64) -> Result<u64, Error> {
    u64::try_from(value).map_err(|_| {
        Error::Calculation("transparent output value must be non-negative".to_string())
    })
}
