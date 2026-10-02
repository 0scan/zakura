//! Finalized explorer transaction, classification, and address-activity indexes.

use std::{
    collections::HashMap,
    ops::Bound::{Excluded, Included},
};

use zakura_chain::{
    parameters::Network,
    serialization::ZcashSerialize,
    transaction::{transaction_value_endpoints, Transaction, TransactionValueEndpoints},
    transparent::{OrderedUtxo, OutPoint, Utxo},
};

use crate::{
    explorer::{
        ExplorerAmountFilter, ExplorerPageDirection, ExplorerShieldedFlowFilter,
        ExplorerShieldedPoolFilter, ExplorerTransactionKindFilter, ExplorerTransactionQuery,
    },
    request::FinalizedBlock,
    service::finalized_state::{DiskWriteBatch, TransactionLocation, ZakuraDb},
};

use super::disk_format::{
    ExplorerAddressRecord, ExplorerAmountBucket, ExplorerShieldedClassLocation,
    ExplorerShieldedFlow, ExplorerShieldedPool, ExplorerTransactionKind,
    ExplorerTransactionKindLocation, ExplorerTransactionRecord,
};

pub(crate) type PendingExplorerAddressRecords =
    HashMap<zakura_chain::transparent::Address, Option<ExplorerAddressRecord>>;

impl ZakuraDb {
    /// Returns compact canonical activity metadata for `address`.
    pub fn explorer_address_record(
        &self,
        address: zakura_chain::transparent::Address,
    ) -> Option<ExplorerAddressRecord> {
        self.explorer_address_meta_cf().zs_get(&address.into())
    }

    /// Returns compact metadata for one finalized transaction location.
    pub fn explorer_transaction_record(
        &self,
        location: TransactionLocation,
    ) -> Option<ExplorerTransactionRecord> {
        self.explorer_transaction_meta_cf().zs_get(&location)
    }

    /// Returns the total non-coinbase fees indexed for one finalized block.
    pub fn explorer_block_total_fees(&self, height: zakura_chain::block::Height) -> u64 {
        self.explorer_transaction_meta_cf()
            .zs_forward_range_iter(
                TransactionLocation::min_for_height(height)
                    ..=TransactionLocation::max_for_height(height),
            )
            .filter(|(location, _)| location.index.as_usize() > 0)
            .try_fold(0_u64, |total, (_, record)| {
                total.checked_add(record.fee_zat)
            })
            .expect("verified block transaction fees fit in u64")
    }

    /// Returns cursor-adjacent finalized transaction locations matching `query`.
    pub fn explorer_transaction_locations(
        &self,
        query: ExplorerTransactionQuery,
        cursor: Option<TransactionLocation>,
        direction: ExplorerPageDirection,
        from_height: zakura_chain::block::Height,
        to_height: zakura_chain::block::Height,
        limit: usize,
    ) -> Vec<TransactionLocation> {
        if limit == 0 || !query.is_valid() || from_height > to_height {
            return Vec::new();
        }

        let mut locations = if query.uses_exact_shielded_index() {
            self.explorer_shielded_locations(
                query,
                cursor,
                direction,
                from_height,
                to_height,
                limit,
            )
        } else {
            self.explorer_kind_locations(
                query.kind,
                cursor,
                direction,
                from_height,
                to_height,
                limit,
            )
        };
        locations.sort_unstable();
        locations.dedup();

        match direction {
            ExplorerPageDirection::Older => {
                locations.reverse();
                locations.truncate(limit);
            }
            ExplorerPageDirection::Newer => {
                locations.truncate(limit);
                locations.reverse();
            }
        }
        locations
    }

    fn explorer_kind_locations(
        &self,
        filter: ExplorerTransactionKindFilter,
        cursor: Option<TransactionLocation>,
        direction: ExplorerPageDirection,
        from_height: zakura_chain::block::Height,
        to_height: zakura_chain::block::Height,
        limit: usize,
    ) -> Vec<TransactionLocation> {
        let kinds: &[ExplorerTransactionKind] = match filter {
            ExplorerTransactionKindFilter::All => &[
                ExplorerTransactionKind::Shielded,
                ExplorerTransactionKind::Transparent,
                ExplorerTransactionKind::Coinbase,
            ],
            ExplorerTransactionKindFilter::Shielded => &[ExplorerTransactionKind::Shielded],
            ExplorerTransactionKindFilter::Transparent => &[ExplorerTransactionKind::Transparent],
            ExplorerTransactionKindFilter::Coinbase => &[ExplorerTransactionKind::Coinbase],
        };
        let cf = self.explorer_transaction_kind_cf();
        let mut locations = Vec::with_capacity(kinds.len().saturating_mul(limit));

        for &kind in kinds {
            let minimum = ExplorerTransactionKindLocation {
                kind,
                location: TransactionLocation::min_for_height(from_height),
            };
            let maximum = ExplorerTransactionKindLocation {
                kind,
                location: TransactionLocation::max_for_height(to_height),
            };
            match direction {
                ExplorerPageDirection::Older => {
                    let upper = cursor.map_or(Included(maximum), |location| {
                        Excluded(ExplorerTransactionKindLocation { kind, location })
                    });
                    locations.extend(
                        cf.zs_reverse_range_iter((Included(minimum), upper))
                            .take(limit)
                            .map(|(entry, ())| entry.location),
                    );
                }
                ExplorerPageDirection::Newer => {
                    let lower = cursor.map_or(Included(minimum), |location| {
                        Excluded(ExplorerTransactionKindLocation { kind, location })
                    });
                    locations.extend(
                        cf.zs_forward_range_iter((lower, Included(maximum)))
                            .take(limit)
                            .map(|(entry, ())| entry.location),
                    );
                }
            }
        }
        locations
    }

    fn explorer_shielded_locations(
        &self,
        query: ExplorerTransactionQuery,
        cursor: Option<TransactionLocation>,
        direction: ExplorerPageDirection,
        from_height: zakura_chain::block::Height,
        to_height: zakura_chain::block::Height,
        limit: usize,
    ) -> Vec<TransactionLocation> {
        let flows: &[ExplorerShieldedFlow] = match query.flow {
            ExplorerShieldedFlowFilter::All => &[
                ExplorerShieldedFlow::Shield,
                ExplorerShieldedFlow::Deshield,
                ExplorerShieldedFlow::FullyShielded,
                ExplorerShieldedFlow::Complex,
            ],
            ExplorerShieldedFlowFilter::Shield => &[ExplorerShieldedFlow::Shield],
            ExplorerShieldedFlowFilter::Deshield => &[ExplorerShieldedFlow::Deshield],
            ExplorerShieldedFlowFilter::FullyShielded => &[ExplorerShieldedFlow::FullyShielded],
            ExplorerShieldedFlowFilter::Complex => &[ExplorerShieldedFlow::Complex],
        };
        let pools: &[ExplorerShieldedPool] = match query.pool {
            ExplorerShieldedPoolFilter::All => &[
                ExplorerShieldedPool::Sprout,
                ExplorerShieldedPool::Sapling,
                ExplorerShieldedPool::Orchard,
                ExplorerShieldedPool::Ironwood,
                ExplorerShieldedPool::Mixed,
            ],
            ExplorerShieldedPoolFilter::Sprout => &[ExplorerShieldedPool::Sprout],
            ExplorerShieldedPoolFilter::Sapling => &[ExplorerShieldedPool::Sapling],
            ExplorerShieldedPoolFilter::Orchard => &[ExplorerShieldedPool::Orchard],
            ExplorerShieldedPoolFilter::Ironwood => &[ExplorerShieldedPool::Ironwood],
            ExplorerShieldedPoolFilter::Mixed => &[ExplorerShieldedPool::Mixed],
        };
        let buckets: &[ExplorerAmountBucket] = match query.amount {
            ExplorerAmountFilter::Any => &[
                ExplorerAmountBucket::BelowOneBillion,
                ExplorerAmountBucket::AtLeastOneBillion,
                ExplorerAmountBucket::AtLeastTenBillion,
                ExplorerAmountBucket::AtLeastOneHundredBillion,
            ],
            ExplorerAmountFilter::AtLeastOneBillion => &[
                ExplorerAmountBucket::AtLeastOneBillion,
                ExplorerAmountBucket::AtLeastTenBillion,
                ExplorerAmountBucket::AtLeastOneHundredBillion,
            ],
            ExplorerAmountFilter::AtLeastTenBillion => &[
                ExplorerAmountBucket::AtLeastTenBillion,
                ExplorerAmountBucket::AtLeastOneHundredBillion,
            ],
            ExplorerAmountFilter::AtLeastOneHundredBillion => {
                &[ExplorerAmountBucket::AtLeastOneHundredBillion]
            }
        };
        let cf = self.explorer_shielded_class_cf();
        let source_count = flows
            .len()
            .saturating_mul(pools.len())
            .saturating_mul(buckets.len());
        let mut locations = Vec::with_capacity(source_count.saturating_mul(limit));

        for &flow in flows {
            for &pool in pools {
                for &amount_bucket in buckets {
                    let minimum = ExplorerShieldedClassLocation {
                        flow,
                        pool,
                        amount_bucket,
                        location: TransactionLocation::min_for_height(from_height),
                    };
                    let maximum = ExplorerShieldedClassLocation {
                        flow,
                        pool,
                        amount_bucket,
                        location: TransactionLocation::max_for_height(to_height),
                    };
                    match direction {
                        ExplorerPageDirection::Older => {
                            let upper = cursor.map_or(Included(maximum), |location| {
                                Excluded(ExplorerShieldedClassLocation {
                                    flow,
                                    pool,
                                    amount_bucket,
                                    location,
                                })
                            });
                            locations.extend(
                                cf.zs_reverse_range_iter((Included(minimum), upper))
                                    .take(limit)
                                    .map(|(entry, ())| entry.location),
                            );
                        }
                        ExplorerPageDirection::Newer => {
                            let lower = cursor.map_or(Included(minimum), |location| {
                                Excluded(ExplorerShieldedClassLocation {
                                    flow,
                                    pool,
                                    amount_bucket,
                                    location,
                                })
                            });
                            locations.extend(
                                cf.zs_forward_range_iter((lower, Included(maximum)))
                                    .take(limit)
                                    .map(|(entry, ())| entry.location),
                            );
                        }
                    }
                }
            }
        }
        locations
    }
}

impl DiskWriteBatch {
    /// Adds compact explorer facts for every transaction in `finalized` to this atomic block batch.
    pub(crate) fn prepare_explorer_transaction_batch(
        &mut self,
        zakura_db: &ZakuraDb,
        network: &Network,
        finalized: &FinalizedBlock,
        spent_utxos: &HashMap<OutPoint, Utxo>,
    ) {
        let mut address_records = HashMap::new();
        for (transaction_index, transaction) in finalized.block.transactions.iter().enumerate() {
            let location = TransactionLocation::from_usize(finalized.height, transaction_index);
            let facts = ExplorerTransactionFacts::from_verified_transaction(
                transaction,
                transaction_index,
                network,
                spent_utxos,
            );

            let _ = zakura_db
                .explorer_transaction_meta_cf()
                .with_batch_for_writing(self)
                .zs_insert(&location, &facts.record);
            let _ = zakura_db
                .explorer_transaction_kind_cf()
                .with_batch_for_writing(self)
                .zs_insert(
                    &ExplorerTransactionKindLocation {
                        kind: facts.kind,
                        location,
                    },
                    &(),
                );

            if let Some((flow, pool, amount_bucket)) = facts.shielded_classification {
                let _ = zakura_db
                    .explorer_shielded_class_cf()
                    .with_batch_for_writing(self)
                    .zs_insert(
                        &ExplorerShieldedClassLocation {
                            flow,
                            pool,
                            amount_bucket,
                            location,
                        },
                        &(),
                    );
            }

            if !finalized.height.is_min() {
                for (address, received_value_zat) in facts
                    .value_endpoints
                    .into_transparent_address_received_values()
                {
                    let record = address_records.entry(address).or_insert_with(|| {
                        zakura_db.explorer_address_record(address).unwrap_or(
                            ExplorerAddressRecord {
                                transaction_count: 0,
                                first_location: location,
                                last_location: location,
                                first_funding_location: None,
                            },
                        )
                    });
                    record.transaction_count = record
                        .transaction_count
                        .checked_add(1)
                        .expect("address transaction count fits in u64");
                    record.last_location = location;
                    if received_value_zat > 0 && record.first_funding_location.is_none() {
                        record.first_funding_location = Some(location);
                    }
                }
            }
        }

        let address_cf = zakura_db
            .explorer_address_meta_cf()
            .with_batch_for_writing(self);
        let _ = address_records
            .into_iter()
            .fold(address_cf, |address_cf, (address, record)| {
                address_cf.zs_insert(&address.into(), &record)
            });
    }

    /// Removes one finalized transaction from the compact explorer indexes.
    pub(crate) fn prepare_explorer_transaction_rollback(
        &mut self,
        zakura_db: &ZakuraDb,
        location: TransactionLocation,
    ) {
        let record = zakura_db
            .explorer_transaction_record(location)
            .expect("rolled-back transactions have explorer metadata");
        let kind = record.kind(location);
        let _ = zakura_db
            .explorer_transaction_meta_cf()
            .with_batch_for_writing(self)
            .zs_delete(&location);
        let _ = zakura_db
            .explorer_transaction_kind_cf()
            .with_batch_for_writing(self)
            .zs_delete(&ExplorerTransactionKindLocation { kind, location });
        if let (Some(flow), Some(pool)) = (record.flow(location), record.pool()) {
            let amount_bucket =
                ExplorerAmountBucket::from_amount(record.public_flow_amount(location));
            let _ = zakura_db
                .explorer_shielded_class_cf()
                .with_batch_for_writing(self)
                .zs_delete(&ExplorerShieldedClassLocation {
                    flow,
                    pool,
                    amount_bucket,
                    location,
                });
        }
    }

    /// Updates pending address metadata while rolling back a transaction suffix.
    pub(crate) fn prepare_explorer_address_rollback(
        zakura_db: &ZakuraDb,
        pending: &mut PendingExplorerAddressRecords,
        address: zakura_chain::transparent::Address,
        location: TransactionLocation,
    ) {
        let current = pending
            .get(&address)
            .copied()
            .unwrap_or_else(|| zakura_db.explorer_address_record(address));
        let mut record = current.expect("rolled-back address transactions have explorer metadata");
        record.transaction_count = record
            .transaction_count
            .checked_sub(1)
            .expect("rolled-back address transaction count is nonzero");
        if record.transaction_count == 0 {
            pending.insert(address, None);
            return;
        }
        record.last_location = zakura_db
            .explorer_address_transaction_locations(
                address,
                Some(location),
                ExplorerPageDirection::Older,
                zakura_chain::block::Height::MIN,
                zakura_chain::block::Height::MAX,
                1,
            )
            .into_iter()
            .next()
            .expect("a retained address transaction exists when its count is nonzero");
        if record.first_funding_location == Some(location) {
            record.first_funding_location = None;
        }
        pending.insert(address, Some(record));
    }

    /// Writes address metadata accumulated during finalized-state rollback.
    pub(crate) fn prepare_explorer_address_rollback_writes(
        &mut self,
        zakura_db: &ZakuraDb,
        pending: PendingExplorerAddressRecords,
    ) {
        let address_cf = zakura_db
            .explorer_address_meta_cf()
            .with_batch_for_writing(self);
        let _ =
            pending
                .into_iter()
                .fold(address_cf, |address_cf, (address, record)| match record {
                    Some(record) => address_cf.zs_insert(&address.into(), &record),
                    None => address_cf.zs_delete(&address.into()),
                });
    }
}

struct ExplorerTransactionFacts {
    record: ExplorerTransactionRecord,
    kind: ExplorerTransactionKind,
    shielded_classification: Option<(
        ExplorerShieldedFlow,
        ExplorerShieldedPool,
        ExplorerAmountBucket,
    )>,
    value_endpoints: TransactionValueEndpoints,
}

pub(crate) fn explorer_transaction_record_with_ordered_utxos(
    transaction: &Transaction,
    transaction_index: usize,
    network: &Network,
    spent_utxos: &HashMap<OutPoint, OrderedUtxo>,
) -> ExplorerTransactionRecord {
    ExplorerTransactionFacts::from_verified_transaction_with_ordered_utxos(
        transaction,
        transaction_index,
        network,
        spent_utxos,
    )
    .record
}

pub(crate) fn explorer_transaction_record_with_utxos(
    transaction: &Transaction,
    transaction_index: usize,
    network: &Network,
    spent_utxos: &HashMap<OutPoint, Utxo>,
) -> ExplorerTransactionRecord {
    ExplorerTransactionFacts::from_verified_transaction(
        transaction,
        transaction_index,
        network,
        spent_utxos,
    )
    .record
}

impl ExplorerTransactionFacts {
    fn from_verified_transaction(
        transaction: &Transaction,
        transaction_index: usize,
        network: &Network,
        spent_utxos: &HashMap<OutPoint, Utxo>,
    ) -> Self {
        let is_coinbase = transaction.is_coinbase();
        debug_assert_eq!(is_coinbase, transaction_index == 0);

        let value_endpoints = transaction_value_endpoints(
            transaction,
            network,
            resolved_spent_outputs(transaction, spent_utxos),
        )
        .expect("verified transaction inputs have valid value balances");
        Self::from_value_endpoints(transaction, transaction_index, value_endpoints)
    }

    pub(crate) fn from_verified_transaction_with_ordered_utxos(
        transaction: &Transaction,
        transaction_index: usize,
        network: &Network,
        spent_utxos: &HashMap<OutPoint, OrderedUtxo>,
    ) -> Self {
        let is_coinbase = transaction.is_coinbase();
        debug_assert_eq!(is_coinbase, transaction_index == 0);
        let value_endpoints = transaction_value_endpoints(
            transaction,
            network,
            resolved_spent_outputs(transaction, spent_utxos),
        )
        .expect("verified transaction inputs have valid value balances");
        Self::from_value_endpoints(transaction, transaction_index, value_endpoints)
    }

    fn from_value_endpoints(
        transaction: &Transaction,
        transaction_index: usize,
        value_endpoints: TransactionValueEndpoints,
    ) -> Self {
        let is_coinbase = transaction.is_coinbase();
        let (primary_from, primary_to) = value_endpoints.primary_endpoints();
        let transaction_value_balance = value_endpoints.value_balance();
        let value_balance = (!is_coinbase).then_some(transaction_value_balance);
        let (fee_zat, transparent_value_balance_zat) = value_balance.map_or((0, 0), |balance| {
            let fee = balance
                .remaining_transaction_value()
                .expect("verified transaction has a nonnegative remaining fee")
                .zatoshis();
            (
                u64::try_from(fee).expect("verified transaction fee is nonnegative"),
                balance.transparent_amount().zatoshis(),
            )
        });
        let record = ExplorerTransactionRecord {
            serialized_size: u32::try_from(transaction.zcash_serialized_size())
                .expect("verified transaction size is bounded by the maximum block size"),
            fee_zat,
            transparent_value_balance_zat,
            sapling_value_balance_zat: transaction_value_balance.sapling_amount().zatoshis(),
            orchard_value_balance_zat: transaction_value_balance.orchard_amount().zatoshis(),
            ironwood_value_balance_zat: transaction_value_balance.ironwood_amount().zatoshis(),
            transparent_input_count: if is_coinbase {
                0
            } else {
                count_u32(transaction.inputs().len())
            },
            transparent_output_count: count_u32(transaction.outputs().len()),
            joinsplit_count: count_u32(transaction.joinsplit_count()),
            sapling_spend_count: count_u32(transaction.sapling_spends_per_anchor().count()),
            sapling_output_count: count_u32(transaction.sapling_outputs().count()),
            orchard_action_count: count_u32(transaction.orchard_actions().count()),
            ironwood_action_count: count_u32(transaction.ironwood_actions().count()),
            transparent_output_total_zat: value_endpoints.transparent_output_total_zat(),
            primary_from,
            primary_to,
        };
        let kind = transaction_kind(transaction_index, &record);
        let shielded_classification = (kind == ExplorerTransactionKind::Shielded).then(|| {
            let pool = shielded_pool(&record)
                .expect("a transaction classified as shielded touches a shielded pool");
            let shielded_balance = shielded_value_balance(&record);
            let flow =
                if record.transparent_input_count == 0 && record.transparent_output_count == 0 {
                    ExplorerShieldedFlow::FullyShielded
                } else if shielded_balance < 0 {
                    ExplorerShieldedFlow::Shield
                } else if shielded_balance > 0 {
                    ExplorerShieldedFlow::Deshield
                } else {
                    ExplorerShieldedFlow::Complex
                };
            let public_amount = matches!(
                flow,
                ExplorerShieldedFlow::Shield | ExplorerShieldedFlow::Deshield
            )
            .then_some(shielded_balance.unsigned_abs());

            (flow, pool, ExplorerAmountBucket::from_amount(public_amount))
        });

        Self {
            record,
            kind,
            shielded_classification,
            value_endpoints,
        }
    }
}

fn resolved_spent_outputs<'a, U>(
    transaction: &'a Transaction,
    spent_utxos: &'a HashMap<OutPoint, U>,
) -> impl Iterator<Item = &'a zakura_chain::transparent::Output>
where
    U: AsRef<Utxo>,
{
    transaction
        .inputs()
        .iter()
        .filter_map(|input| input.outpoint())
        .map(|outpoint| {
            &spent_utxos
                .get(&outpoint)
                .expect("verified transaction inputs have resolved transparent outputs")
                .as_ref()
                .output
        })
}

fn transaction_kind(
    transaction_index: usize,
    record: &ExplorerTransactionRecord,
) -> ExplorerTransactionKind {
    if transaction_index == 0 {
        ExplorerTransactionKind::Coinbase
    } else if shielded_pool(record).is_some() {
        ExplorerTransactionKind::Shielded
    } else {
        ExplorerTransactionKind::Transparent
    }
}

fn shielded_pool(record: &ExplorerTransactionRecord) -> Option<ExplorerShieldedPool> {
    let pools = [
        (record.joinsplit_count > 0, ExplorerShieldedPool::Sprout),
        (
            record.sapling_spend_count > 0 || record.sapling_output_count > 0,
            ExplorerShieldedPool::Sapling,
        ),
        (
            record.orchard_action_count > 0,
            ExplorerShieldedPool::Orchard,
        ),
        (
            record.ironwood_action_count > 0,
            ExplorerShieldedPool::Ironwood,
        ),
    ];
    let mut active = pools
        .into_iter()
        .filter_map(|(active, pool)| active.then_some(pool));
    let first = active.next()?;
    Some(if active.next().is_some() {
        ExplorerShieldedPool::Mixed
    } else {
        first
    })
}

fn shielded_value_balance(record: &ExplorerTransactionRecord) -> i64 {
    let balance = i128::from(record.fee_zat) - i128::from(record.transparent_value_balance_zat);
    i64::try_from(balance).expect("verified transaction value balances fit in the money range")
}

fn count_u32(count: usize) -> u32 {
    u32::try_from(count).expect("transaction component count is bounded by the maximum block size")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amount_buckets_are_disjoint() {
        assert_eq!(
            ExplorerAmountBucket::from_amount(Some(999_999_999)),
            ExplorerAmountBucket::BelowOneBillion
        );
        assert_eq!(
            ExplorerAmountBucket::from_amount(Some(10_000_000_000)),
            ExplorerAmountBucket::AtLeastTenBillion
        );
        assert_eq!(
            ExplorerAmountBucket::from_amount(Some(100_000_000_000)),
            ExplorerAmountBucket::AtLeastOneHundredBillion
        );
    }
}
