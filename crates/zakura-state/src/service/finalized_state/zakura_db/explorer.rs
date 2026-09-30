//! Finalized explorer transaction facts and compact ordered indexes.

use std::{
    collections::{BTreeSet, HashMap},
    ops::Bound::{Excluded, Included},
};

use zakura_chain::amount::NegativeAllowed;
use zakura_chain::{
    parameters::Network,
    serialization::ZcashSerialize,
    transaction::Transaction,
    transparent::{OrderedUtxo, OutPoint, Utxo},
    value_balance::ValueBalance,
};

use crate::service::finalized_state::disk_format::transparent::AddressBalanceLocationUpdates;
use crate::{
    explorer::{
        ExplorerAmountFilter, ExplorerBalanceRankEntry, ExplorerPageDirection,
        ExplorerShieldedFlowFilter, ExplorerShieldedPoolFilter, ExplorerTransactionKindFilter,
        ExplorerTransactionQuery,
    },
    request::FinalizedBlock,
    service::finalized_state::{
        disk_format::explorer::{
            ExplorerAddressKey, ExplorerAddressRecord, ExplorerAmountBucket, ExplorerBalanceKey,
            ExplorerDayKey, ExplorerSchemaVersion, ExplorerShieldedClassLocation,
            ExplorerShieldedFlow, ExplorerShieldedPool, ExplorerTransactionKind,
            ExplorerTransactionKindLocation, ExplorerTransactionRecord,
        },
        DiskWriteBatch, TransactionLocation, TypedColumnFamily,
    },
};

use super::ZakuraDb;

pub(crate) type PendingExplorerAddressRecords =
    HashMap<zakura_chain::transparent::Address, Option<ExplorerAddressRecord>>;

/// Explorer facts carried between the existing transparent and chain commit phases.
///
/// Keeping this feature-only context in the explorer module lets the core write methods retain
/// their original return values while still reusing facts they have already calculated.
pub struct ExplorerBlockCommitContext {
    funded_transparent_address_count: u64,
}

impl ExplorerBlockCommitContext {
    pub(crate) fn new(db: &ZakuraDb) -> Self {
        Self {
            funded_transparent_address_count: db
                .explorer_chain_stats()
                .funded_transparent_address_count,
        }
    }

    pub(crate) fn set_funded_transparent_address_count(&mut self, count: u64) {
        self.funded_transparent_address_count = count;
    }

    pub(crate) fn funded_transparent_address_count(&self) -> u64 {
        self.funded_transparent_address_count
    }
}

/// Fixed-width metadata keyed once per finalized transaction.
pub const EXPLORER_TRANSACTION_META_BY_LOC: &str = "explorer_tx_meta_by_loc";
/// Explorer-owned schema marker, independent from the canonical state format version.
pub const EXPLORER_SCHEMA: &str = "explorer_schema";
/// One chain-ordered key per finalized transaction, partitioned by kind.
pub const EXPLORER_TRANSACTION_BY_KIND_LOC: &str = "explorer_tx_by_kind_loc";
/// One exact flow/pool/amount-bucket key per finalized shielded transaction.
pub const EXPLORER_SHIELDED_TRANSACTION_BY_CLASS_LOC: &str = "explorer_shielded_tx_by_class_loc";
/// Compact address activity positions keyed by transparent address.
pub const EXPLORER_ADDRESS_META: &str = "explorer_address_meta";
/// Analytics facts keyed by canonical block height.
pub const EXPLORER_BLOCK_STATS: &str = "explorer_block_stats";
/// One canonical all-time aggregate value.
pub const EXPLORER_CHAIN_STATS: &str = "explorer_chain_stats";
/// UTC daily analytics snapshots.
pub const EXPLORER_DAILY_STATS: &str = "explorer_daily_stats";
/// Funded transparent addresses ordered by descending balance.
pub const EXPLORER_BALANCE_ORDER: &str = "explorer_balance_order";

type ExplorerTransactionMetaCf<'cf> =
    TypedColumnFamily<'cf, TransactionLocation, ExplorerTransactionRecord>;
type ExplorerSchemaCf<'cf> = TypedColumnFamily<'cf, (), ExplorerSchemaVersion>;
type ExplorerTransactionKindCf<'cf> = TypedColumnFamily<'cf, ExplorerTransactionKindLocation, ()>;
type ExplorerShieldedClassCf<'cf> = TypedColumnFamily<'cf, ExplorerShieldedClassLocation, ()>;
type ExplorerAddressMetaCf<'cf> = TypedColumnFamily<'cf, ExplorerAddressKey, ExplorerAddressRecord>;
type ExplorerBlockStatsCf<'cf> =
    TypedColumnFamily<'cf, zakura_chain::block::Height, crate::ExplorerBlockStats>;
type ExplorerChainStatsCf<'cf> = TypedColumnFamily<'cf, (), crate::ExplorerChainStats>;
type ExplorerDailyStatsCf<'cf> = TypedColumnFamily<'cf, ExplorerDayKey, crate::ExplorerDailyStats>;
type ExplorerBalanceOrderCf<'cf> = TypedColumnFamily<'cf, ExplorerBalanceKey, ()>;

impl ZakuraDb {
    fn explorer_schema_cf(&self) -> ExplorerSchemaCf<'_> {
        ExplorerSchemaCf::new(&self.db, EXPLORER_SCHEMA)
            .expect("explorer schema column family is registered")
    }

    /// Validates an existing explorer schema or initializes a new empty database.
    pub(crate) fn ensure_explorer_schema(
        &self,
        read_only: bool,
    ) -> Result<(), crate::StateInitError> {
        match self.explorer_schema_cf().zs_get(&()) {
            Some(version) if version == ExplorerSchemaVersion::CURRENT => Ok(()),
            Some(version) => Err(crate::StateInitError::ExplorerSchema {
                path: self.path().to_owned(),
                reason: format!(
                    "found schema version {}, but this build requires {}",
                    version.0,
                    ExplorerSchemaVersion::CURRENT.0
                ),
            }),
            None if self.tip().is_some() => Err(crate::StateInitError::ExplorerSchema {
                path: self.path().to_owned(),
                reason: "canonical state already contains blocks but has no explorer indexes; use a fresh state database and resync"
                    .to_string(),
            }),
            None if read_only => Err(crate::StateInitError::ExplorerSchema {
                path: self.path().to_owned(),
                reason: "the read-only primary database has no explorer schema marker".to_string(),
            }),
            None => {
                let mut batch = DiskWriteBatch::new();
                let _ = self
                    .explorer_schema_cf()
                    .with_batch_for_writing(&mut batch)
                    .zs_insert(&(), &ExplorerSchemaVersion::CURRENT);
                self.write_batch(batch)
                    .map_err(|error| crate::StateInitError::ExplorerSchema {
                        path: self.path().to_owned(),
                        reason: format!("could not initialize the explorer schema: {error}"),
                    })
            }
        }
    }

    fn explorer_transaction_meta_cf(&self) -> ExplorerTransactionMetaCf<'_> {
        ExplorerTransactionMetaCf::new(&self.db, EXPLORER_TRANSACTION_META_BY_LOC)
            .expect("explorer transaction metadata column family is registered")
    }

    fn explorer_transaction_kind_cf(&self) -> ExplorerTransactionKindCf<'_> {
        ExplorerTransactionKindCf::new(&self.db, EXPLORER_TRANSACTION_BY_KIND_LOC)
            .expect("explorer transaction kind column family is registered")
    }

    fn explorer_shielded_class_cf(&self) -> ExplorerShieldedClassCf<'_> {
        ExplorerShieldedClassCf::new(&self.db, EXPLORER_SHIELDED_TRANSACTION_BY_CLASS_LOC)
            .expect("explorer shielded classification column family is registered")
    }

    fn explorer_address_meta_cf(&self) -> ExplorerAddressMetaCf<'_> {
        ExplorerAddressMetaCf::new(&self.db, EXPLORER_ADDRESS_META)
            .expect("explorer address metadata column family is registered")
    }

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
        limit: usize,
    ) -> Vec<TransactionLocation> {
        if limit == 0 || !query.is_valid() {
            return Vec::new();
        }

        let mut locations = if query.uses_exact_shielded_index() {
            self.explorer_shielded_locations(query, cursor, direction, limit)
        } else {
            self.explorer_kind_locations(query.kind, cursor, direction, limit)
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
                location: TransactionLocation::MIN,
            };
            let maximum = ExplorerTransactionKindLocation {
                kind,
                location: TransactionLocation::MAX,
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
                        location: TransactionLocation::MIN,
                    };
                    let maximum = ExplorerShieldedClassLocation {
                        flow,
                        pool,
                        amount_bucket,
                        location: TransactionLocation::MAX,
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

    fn explorer_block_stats_cf(&self) -> ExplorerBlockStatsCf<'_> {
        ExplorerBlockStatsCf::new(&self.db, EXPLORER_BLOCK_STATS)
            .expect("explorer block analytics column family is registered")
    }

    fn explorer_chain_stats_cf(&self) -> ExplorerChainStatsCf<'_> {
        ExplorerChainStatsCf::new(&self.db, EXPLORER_CHAIN_STATS)
            .expect("explorer chain analytics column family is registered")
    }

    fn explorer_daily_stats_cf(&self) -> ExplorerDailyStatsCf<'_> {
        ExplorerDailyStatsCf::new(&self.db, EXPLORER_DAILY_STATS)
            .expect("explorer daily analytics column family is registered")
    }

    fn explorer_balance_order_cf(&self) -> ExplorerBalanceOrderCf<'_> {
        ExplorerBalanceOrderCf::new(&self.db, EXPLORER_BALANCE_ORDER)
            .expect("explorer balance order column family is registered")
    }

    /// Returns analytics for one finalized canonical block.
    pub fn explorer_block_stats(
        &self,
        height: zakura_chain::block::Height,
    ) -> Option<crate::ExplorerBlockStats> {
        self.explorer_block_stats_cf().zs_get(&height)
    }

    /// Returns finalized all-time explorer aggregates.
    pub fn explorer_chain_stats(&self) -> crate::ExplorerChainStats {
        self.explorer_chain_stats_cf()
            .zs_get(&())
            .unwrap_or_default()
    }

    /// Returns finalized daily snapshots in ascending UTC-day order.
    pub fn explorer_daily_stats(&self) -> Vec<crate::ExplorerDailyStats> {
        self.explorer_daily_stats_cf()
            .zs_forward_range_iter(..)
            .map(|(_, stats)| stats)
            .collect()
    }

    /// Returns the richest finalized transparent addresses, optionally starting at a key.
    pub fn explorer_balance_entries(
        &self,
        start: Option<(zakura_chain::transparent::Address, u64)>,
        limit: usize,
    ) -> Vec<ExplorerBalanceRankEntry> {
        let cf = self.explorer_balance_order_cf();
        let mut entries = match start {
            Some((address, balance_zat)) => cf
                .zs_forward_range_iter(ExplorerBalanceKey::new(address, balance_zat)..)
                .skip(1)
                .take(limit)
                .collect::<Vec<_>>(),
            None => cf.zs_forward_range_iter(..).take(limit).collect::<Vec<_>>(),
        };
        entries
            .drain(..)
            .map(|(key, ())| ExplorerBalanceRankEntry {
                address: key.address(),
                balance_zat: key.balance_zat(),
            })
            .collect()
    }

    /// Returns whether a funded finalized address occupies this exact ranking key.
    pub fn explorer_contains_balance_entry(
        &self,
        address: zakura_chain::transparent::Address,
        balance_zat: u64,
    ) -> bool {
        self.explorer_balance_order_cf()
            .zs_contains(&ExplorerBalanceKey::new(address, balance_zat))
    }
}

impl DiskWriteBatch {
    /// Updates the funded-address ranking from the final balances already prepared for a block.
    pub(crate) fn prepare_explorer_balance_order_batch(
        &mut self,
        zakura_db: &ZakuraDb,
        updates: &AddressBalanceLocationUpdates,
        previous_balance_zat: &HashMap<zakura_chain::transparent::Address, u64>,
    ) -> u64 {
        let mut funded = zakura_db
            .explorer_chain_stats()
            .funded_transparent_address_count;
        let mut apply = |address, old_balance_zat: u64, new_balance_zat: u64| {
            // A block can spend and recreate the same balance for an address. Its address
            // aggregates still change, but its ordered ranking key does not. Avoid generating
            // a redundant tombstone and insertion for that common case.
            if old_balance_zat == new_balance_zat {
                return;
            }

            let ranking = zakura_db
                .explorer_balance_order_cf()
                .with_batch_for_writing(self);
            let ranking = if old_balance_zat > 0 {
                ranking.zs_delete(&ExplorerBalanceKey::new(address, old_balance_zat))
            } else {
                ranking
            };
            if new_balance_zat > 0 {
                let _ = ranking.zs_insert(&ExplorerBalanceKey::new(address, new_balance_zat), &());
            } else {
                drop(ranking);
            }
            match (old_balance_zat > 0, new_balance_zat > 0) {
                (false, true) => {
                    funded = funded
                        .checked_add(1)
                        .expect("funded address count fits in u64")
                }
                (true, false) => {
                    funded = funded
                        .checked_sub(1)
                        .expect("removed funded address was previously counted")
                }
                _ => {}
            }
        };

        match updates {
            AddressBalanceLocationUpdates::Insert(balances) => {
                for (&address, balance) in balances {
                    apply(
                        address,
                        previous_balance_zat.get(&address).copied().unwrap_or(0),
                        u64::try_from(balance.balance().zatoshis())
                            .expect("prepared transparent balances are nonnegative"),
                    );
                }
            }
            AddressBalanceLocationUpdates::Merge(changes) => {
                for (&address, change) in changes {
                    let old_balance_zat = previous_balance_zat.get(&address).copied().unwrap_or(0);
                    let new = i64::try_from(old_balance_zat)
                        .expect("transparent balances fit in i64")
                        .checked_add(change.balance().zatoshis())
                        .expect("verified transparent balance update stays in the money range");
                    apply(
                        address,
                        old_balance_zat,
                        u64::try_from(new)
                            .expect("verified transparent balance remains nonnegative"),
                    );
                }
            }
        }
        funded
    }

    /// Rewrites balance-order entries for addresses changed by an offline rollback.
    pub(crate) fn prepare_explorer_balance_order_rollback(
        &mut self,
        zakura_db: &ZakuraDb,
        balances: &HashMap<
            zakura_chain::transparent::Address,
            Option<
                crate::service::finalized_state::disk_format::transparent::AddressBalanceLocation,
            >,
        >,
    ) -> u64 {
        let mut funded = zakura_db
            .explorer_chain_stats()
            .funded_transparent_address_count;
        for (&address, balance) in balances {
            let old_balance_zat = zakura_db
                .address_balance_location(&address)
                .map(|record| {
                    u64::try_from(record.balance().zatoshis())
                        .expect("finalized transparent balances are nonnegative")
                })
                .unwrap_or(0);
            let new_balance_zat = balance
                .as_ref()
                .map(|record| {
                    u64::try_from(record.balance().zatoshis())
                        .expect("rolled-back transparent balances are nonnegative")
                })
                .unwrap_or(0);
            if old_balance_zat == new_balance_zat {
                continue;
            }
            let ranking = zakura_db
                .explorer_balance_order_cf()
                .with_batch_for_writing(self);
            let ranking = if old_balance_zat > 0 {
                ranking.zs_delete(&ExplorerBalanceKey::new(address, old_balance_zat))
            } else {
                ranking
            };
            if new_balance_zat > 0 {
                let _ = ranking.zs_insert(&ExplorerBalanceKey::new(address, new_balance_zat), &());
            } else {
                drop(ranking);
            }
            match (old_balance_zat > 0, new_balance_zat > 0) {
                (false, true) => {
                    funded = funded
                        .checked_add(1)
                        .expect("funded address count fits in u64")
                }
                (true, false) => {
                    funded = funded
                        .checked_sub(1)
                        .expect("removed funded address was previously counted")
                }
                _ => {}
            }
        }
        funded
    }

    /// Writes canonical analytics in the same atomic batch as the finalized block.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prepare_explorer_analytics_batch(
        &mut self,
        zakura_db: &ZakuraDb,
        network: &Network,
        finalized: &FinalizedBlock,
        serialized_size: u32,
        value_pools: ValueBalance<zakura_chain::amount::NonNegative>,
        spent_utxos: &HashMap<OutPoint, Utxo>,
        previous_pool_nsm: i64,
        funded_transparent_address_count: u64,
    ) {
        let block = crate::service::explorer_analytics::derive_block_stats(
            &finalized.block,
            finalized.height,
            serialized_size,
            value_pools,
            spent_utxos,
            previous_pool_nsm,
            funded_transparent_address_count,
            network,
        );
        let mut chain = zakura_db.explorer_chain_stats();
        crate::service::explorer_analytics::add_block_to_chain_stats(&mut chain, &block);

        let day = crate::service::explorer_analytics::day_number(block.timestamp);
        let mut daily = zakura_db
            .explorer_daily_stats_cf()
            .zs_get(&ExplorerDayKey(day))
            .unwrap_or_default();
        let previous_timestamp = finalized
            .height
            .previous()
            .ok()
            .and_then(|height| zakura_db.explorer_block_stats(height))
            .map(|record| record.timestamp);
        crate::service::explorer_analytics::add_block_to_daily_stats(
            &mut daily,
            &block,
            previous_timestamp,
        );

        let _ = zakura_db
            .explorer_block_stats_cf()
            .with_batch_for_writing(self)
            .zs_insert(&finalized.height, &block);
        let _ = zakura_db
            .explorer_chain_stats_cf()
            .with_batch_for_writing(self)
            .zs_insert(&(), &chain);
        let _ = zakura_db
            .explorer_daily_stats_cf()
            .with_batch_for_writing(self)
            .zs_insert(&ExplorerDayKey(day), &daily);
    }

    /// Removes analytics above an offline rollback target and rebuilds affected UTC days.
    pub(crate) fn prepare_explorer_analytics_rollback(
        &mut self,
        zakura_db: &ZakuraDb,
        removed_heights: &[zakura_chain::block::Height],
        target_height: zakura_chain::block::Height,
        funded_transparent_address_count: u64,
    ) {
        let mut chain = zakura_db.explorer_chain_stats();
        let mut affected_days = BTreeSet::new();
        for &height in removed_heights {
            let block = zakura_db
                .explorer_block_stats(height)
                .expect("rolled-back finalized blocks have explorer analytics");
            crate::service::explorer_analytics::remove_block_from_chain_stats(&mut chain, &block);
            affected_days.insert(crate::service::explorer_analytics::day_number(
                block.timestamp,
            ));
            let _ = zakura_db
                .explorer_block_stats_cf()
                .with_batch_for_writing(self)
                .zs_delete(&height);
        }
        chain.funded_transparent_address_count = funded_transparent_address_count;
        let _ = zakura_db
            .explorer_chain_stats_cf()
            .with_batch_for_writing(self)
            .zs_insert(&(), &chain);

        for day in affected_days {
            let existing = zakura_db
                .explorer_daily_stats_cf()
                .zs_get(&ExplorerDayKey(day))
                .expect("affected UTC days have explorer analytics");
            let retained_end = existing.end_height.min(target_height.0);
            if retained_end < existing.start_height {
                let _ = zakura_db
                    .explorer_daily_stats_cf()
                    .with_batch_for_writing(self)
                    .zs_delete(&ExplorerDayKey(day));
                continue;
            }

            let mut rebuilt = crate::ExplorerDailyStats::default();
            let mut previous_timestamp = existing
                .start_height
                .checked_sub(1)
                .and_then(|height| {
                    zakura_db.explorer_block_stats(zakura_chain::block::Height(height))
                })
                .map(|block| block.timestamp);
            for raw_height in existing.start_height..=retained_end {
                let block = zakura_db
                    .explorer_block_stats(zakura_chain::block::Height(raw_height))
                    .expect("retained finalized blocks have explorer analytics");
                if crate::service::explorer_analytics::day_number(block.timestamp) == day {
                    crate::service::explorer_analytics::add_block_to_daily_stats(
                        &mut rebuilt,
                        &block,
                        previous_timestamp,
                    );
                }
                previous_timestamp = Some(block.timestamp);
            }
            let _ = zakura_db
                .explorer_daily_stats_cf()
                .with_batch_for_writing(self)
                .zs_insert(&ExplorerDayKey(day), &rebuilt);
        }
    }

    /// Adds compact explorer facts for every transaction in `finalized` to this atomic block batch.
    pub(super) fn prepare_explorer_transaction_batch(
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
                let mut address_funding = HashMap::new();
                for outpoint in transaction
                    .inputs()
                    .iter()
                    .filter_map(|input| input.outpoint())
                {
                    if let Some(address) = spent_utxos
                        .get(&outpoint)
                        .and_then(|utxo| utxo.output.address(network))
                    {
                        address_funding.entry(address).or_insert(false);
                    }
                }
                for output in transaction.outputs() {
                    if let Some(address) = output.address(network) {
                        let received = output.value().zatoshis() > 0;
                        address_funding
                            .entry(address)
                            .and_modify(|funded| *funded |= received)
                            .or_insert(received);
                    }
                }
                for (address, received) in address_funding {
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
                    if received && record.first_funding_location.is_none() {
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
}

pub(crate) fn explorer_transaction_record_with_ordered_utxos(
    transaction: &Transaction,
    transaction_index: usize,
    spent_utxos: &HashMap<OutPoint, OrderedUtxo>,
) -> ExplorerTransactionRecord {
    ExplorerTransactionFacts::from_verified_transaction_with_ordered_utxos(
        transaction,
        transaction_index,
        spent_utxos,
    )
    .record
}

pub(crate) fn explorer_transaction_record_with_utxos(
    transaction: &Transaction,
    transaction_index: usize,
    spent_utxos: &HashMap<OutPoint, Utxo>,
) -> ExplorerTransactionRecord {
    ExplorerTransactionFacts::from_verified_transaction(transaction, transaction_index, spent_utxos)
        .record
}

impl ExplorerTransactionFacts {
    fn from_verified_transaction(
        transaction: &Transaction,
        transaction_index: usize,
        spent_utxos: &HashMap<OutPoint, Utxo>,
    ) -> Self {
        let is_coinbase = transaction.is_coinbase();
        debug_assert_eq!(is_coinbase, transaction_index == 0);

        let value_balance = (!is_coinbase).then(|| {
            transaction
                .value_balance(spent_utxos)
                .expect("verified transaction inputs have already-resolved value balances")
        });
        Self::from_value_balance(transaction, transaction_index, value_balance)
    }

    pub(crate) fn from_verified_transaction_with_ordered_utxos(
        transaction: &Transaction,
        transaction_index: usize,
        spent_utxos: &HashMap<OutPoint, OrderedUtxo>,
    ) -> Self {
        let is_coinbase = transaction.is_coinbase();
        debug_assert_eq!(is_coinbase, transaction_index == 0);
        let value_balance = (!is_coinbase).then(|| {
            transaction
                .value_balance_from_ordered_utxos(spent_utxos)
                .expect("verified transaction inputs have already-resolved value balances")
        });
        Self::from_value_balance(transaction, transaction_index, value_balance)
    }

    fn from_value_balance(
        transaction: &Transaction,
        transaction_index: usize,
        value_balance: Option<ValueBalance<NegativeAllowed>>,
    ) -> Self {
        let is_coinbase = transaction.is_coinbase();
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
                count_u32(transaction.inputs().len())
            },
            transparent_output_count: count_u32(transaction.outputs().len()),
            joinsplit_count: count_u32(transaction.joinsplit_count()),
            sapling_spend_count: count_u32(transaction.sapling_spends_per_anchor().count()),
            sapling_output_count: count_u32(transaction.sapling_outputs().count()),
            orchard_action_count: count_u32(transaction.orchard_actions().count()),
            ironwood_action_count: count_u32(transaction.ironwood_actions().count()),
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
        }
    }
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
    use zakura_chain::{
        amount::{Amount, NonNegative},
        block::Height,
        parameters::NetworkKind,
        transparent::Address,
    };

    use crate::{
        constants::{state_database_format_version_in_code, STATE_DATABASE_KIND},
        service::finalized_state::{
            disk_format::transparent::{AddressBalanceLocation, OutputLocation},
            STATE_COLUMN_FAMILIES_IN_CODE,
        },
        Config,
    };

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

    #[test]
    fn explorer_schema_version_is_initialized_and_validated() {
        let db = ZakuraDb::new(
            &Config::ephemeral(),
            STATE_DATABASE_KIND,
            &state_database_format_version_in_code(),
            &Network::Mainnet,
            true,
            STATE_COLUMN_FAMILIES_IN_CODE
                .iter()
                .map(ToString::to_string),
            false,
        )
        .expect("opening an ephemeral database should succeed");

        assert_eq!(
            db.explorer_schema_cf().zs_get(&()),
            Some(ExplorerSchemaVersion::CURRENT)
        );

        let mut batch = DiskWriteBatch::new();
        let _ = db
            .explorer_schema_cf()
            .with_batch_for_writing(&mut batch)
            .zs_insert(
                &(),
                &ExplorerSchemaVersion(ExplorerSchemaVersion::CURRENT.0 + 1),
            );
        db.write_batch(batch)
            .expect("writing an unsupported test schema should succeed");

        let error = db
            .ensure_explorer_schema(false)
            .expect_err("an unsupported explorer schema must be rejected");
        assert!(matches!(
            error,
            crate::StateInitError::ExplorerSchema { .. }
        ));
    }

    #[test]
    fn unchanged_balance_does_not_rewrite_the_ordered_index() {
        let network = Network::Mainnet;
        let db = ZakuraDb::new(
            &Config::ephemeral(),
            STATE_DATABASE_KIND,
            &state_database_format_version_in_code(),
            &network,
            true,
            STATE_COLUMN_FAMILIES_IN_CODE
                .iter()
                .map(ToString::to_string),
            false,
        )
        .expect("opening an ephemeral database should succeed");
        let address = Address::from_pub_key_hash(NetworkKind::Mainnet, [1; 20]);
        let mut balance = AddressBalanceLocation::new(OutputLocation::from_usize(Height(1), 0, 0));
        *balance.balance_mut() =
            Amount::<NonNegative>::try_from(10).expect("test balance is valid");
        let updates = AddressBalanceLocationUpdates::Insert(HashMap::from([(address, balance)]));
        let previous_balance_zat = HashMap::from([(address, 10)]);

        let mut batch = DiskWriteBatch::new();
        batch.prepare_explorer_balance_order_batch(&db, &updates, &previous_balance_zat);

        assert_eq!(batch, DiskWriteBatch::new());
    }
}
