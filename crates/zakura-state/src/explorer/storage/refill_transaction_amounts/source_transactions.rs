//! Batched historical source-transaction loading for explorer metadata refill.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
};

use rayon::{prelude::*, ThreadPool};
use zakura_chain::{
    parameters::Network,
    transaction::{self, primary_value_endpoints, Transaction},
    transparent::{OutPoint, Output},
};

use crate::service::finalized_state::{RawBytes, TransactionLocation, TypedColumnFamily, ZakuraDb};

use super::RefillTransactionAmountsError;
use crate::explorer::storage::disk_format::upgraded_explorer_transaction_record_bytes;

/// One schema-v1 transaction metadata row and its retained raw transaction.
pub(super) struct LegacyTransactionEntry {
    pub(super) location: TransactionLocation,
    pub(super) record: RawBytes,
    pub(super) transaction: Transaction,
}

/// One schema-v2 transaction metadata row ready for an atomic write batch.
pub(super) struct UpgradedTransactionEntry {
    pub(super) location: TransactionLocation,
    pub(super) record: RawBytes,
}

/// A bounded FIFO cache of decoded outputs referenced by transparent inputs.
///
/// Retaining only the requested outputs prevents large shielded source transactions from remaining
/// resident between batches. FIFO eviction keeps cache operations constant-time without a shared
/// lock in the parallel derivation path.
pub(super) struct SourceOutputCache {
    capacity: usize,
    outputs: HashMap<OutPoint, Arc<Output>>,
    insertion_order: VecDeque<OutPoint>,
}

impl SourceOutputCache {
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            capacity,
            outputs: HashMap::with_capacity(capacity),
            insertion_order: VecDeque::with_capacity(capacity),
        }
    }

    fn get(&self, outpoint: OutPoint) -> Option<Arc<Output>> {
        self.outputs.get(&outpoint).cloned()
    }

    fn insert(&mut self, outpoint: OutPoint, output: Arc<Output>) {
        if self.capacity == 0 || self.outputs.contains_key(&outpoint) {
            return;
        }

        while self.outputs.len() >= self.capacity {
            let evicted_outpoint = self
                .insertion_order
                .pop_front()
                .expect("a full source output cache has an insertion-order entry");
            self.outputs.remove(&evicted_outpoint);
        }

        self.outputs.insert(outpoint, output);
        self.insertion_order.push_back(outpoint);
    }
}

/// Upgrades a chunk after loading each distinct transparent source transaction at most once.
pub(super) fn upgrade_transaction_entries(
    db: &ZakuraDb,
    transaction_cf: &TypedColumnFamily<'_, TransactionLocation, Transaction>,
    network: &Network,
    worker_pool: &ThreadPool,
    source_cache: &mut SourceOutputCache,
    entries: &[LegacyTransactionEntry],
) -> Result<Vec<UpgradedTransactionEntry>, RefillTransactionAmountsError> {
    let source_outpoints = distinct_source_outpoints(entries);
    let mut source_outputs = HashMap::with_capacity(source_outpoints.len());
    let mut missing_outpoints_by_hash = HashMap::<transaction::Hash, Vec<OutPoint>>::new();
    for outpoint in source_outpoints {
        if let Some(output) = source_cache.get(outpoint) {
            source_outputs.insert(outpoint, output);
        } else {
            missing_outpoints_by_hash
                .entry(outpoint.hash)
                .or_default()
                .push(outpoint);
        }
    }

    let loaded_outputs = worker_pool.install(|| {
        missing_outpoints_by_hash
            .into_par_iter()
            .map(|(hash, outpoints)| load_source_outputs(db, transaction_cf, hash, &outpoints))
            .collect::<Vec<_>>()
    });
    for loaded_outputs_for_transaction in loaded_outputs {
        for (outpoint, output) in loaded_outputs_for_transaction? {
            source_cache.insert(outpoint, output.clone());
            source_outputs.insert(outpoint, output);
        }
    }

    worker_pool.install(|| {
        entries
            .par_iter()
            .map(|entry| upgrade_transaction_entry(entry, network, &source_outputs))
            .collect()
    })
}

fn distinct_source_outpoints(entries: &[LegacyTransactionEntry]) -> Vec<OutPoint> {
    let mut seen = HashSet::new();
    entries
        .iter()
        .flat_map(|entry| entry.transaction.spent_outpoints())
        .filter(|outpoint| seen.insert(*outpoint))
        .collect()
}

fn load_source_outputs(
    db: &ZakuraDb,
    transaction_cf: &TypedColumnFamily<'_, TransactionLocation, Transaction>,
    hash: transaction::Hash,
    outpoints: &[OutPoint],
) -> Result<Vec<(OutPoint, Arc<Output>)>, RefillTransactionAmountsError> {
    let location = db
        .transaction_location(hash)
        .ok_or(RefillTransactionAmountsError::MissingSpentTransactionLocation(hash))?;
    let transaction = transaction_cf
        .zs_get(&location)
        .ok_or(RefillTransactionAmountsError::MissingSpentTransaction { hash, location })?;

    outpoints
        .iter()
        .map(|outpoint| {
            let output_index = usize::try_from(outpoint.index)
                .expect("u32 output indexes fit in usize on supported targets");
            let output = transaction
                .outputs()
                .get(output_index)
                .ok_or(RefillTransactionAmountsError::MissingSpentOutput(*outpoint))?;
            Ok((*outpoint, Arc::new(output.clone())))
        })
        .collect()
}

fn upgrade_transaction_entry(
    entry: &LegacyTransactionEntry,
    network: &Network,
    source_outputs: &HashMap<OutPoint, Arc<Output>>,
) -> Result<UpgradedTransactionEntry, RefillTransactionAmountsError> {
    let spent_outputs = entry
        .transaction
        .spent_outpoints()
        .map(|outpoint| {
            source_outputs
                .get(&outpoint)
                .map(AsRef::as_ref)
                .ok_or(RefillTransactionAmountsError::MissingSpentOutput(outpoint))
        })
        .collect::<Result<Vec<&Output>, _>>()?;
    let (primary_from, primary_to) =
        primary_value_endpoints(&entry.transaction, network, &spent_outputs).map_err(|source| {
            RefillTransactionAmountsError::PrimaryEndpoints {
                location: entry.location,
                source,
            }
        })?;
    let transparent_output_total_zat = entry
        .transaction
        .outputs()
        .iter()
        .try_fold(0_i64, |total, output| {
            total.checked_add(output.value().zatoshis())
        })
        .ok_or(RefillTransactionAmountsError::OutputTotalOverflow(
            entry.location,
        ))?;
    let upgraded = upgraded_explorer_transaction_record_bytes(
        entry.record.raw_bytes(),
        transparent_output_total_zat,
        primary_from,
        primary_to,
    );

    Ok(UpgradedTransactionEntry {
        location: entry.location,
        record: RawBytes::new_raw_bytes(upgraded.to_vec()),
    })
}

#[cfg(test)]
mod tests {
    use zakura_chain::{block::genesis::regtest_genesis_block, transaction::Hash};

    use super::*;

    #[test]
    fn source_output_cache_evicts_oldest_insertions() {
        let output = Arc::new(
            regtest_genesis_block()
                .transactions
                .first()
                .expect("genesis block has a coinbase transaction")
                .outputs()
                .first()
                .expect("genesis coinbase has an output")
                .clone(),
        );
        let first = Hash([1; 32]);
        let second = Hash([2; 32]);
        let third = Hash([3; 32]);
        let first = OutPoint::from_usize(first, 0);
        let second = OutPoint::from_usize(second, 0);
        let third = OutPoint::from_usize(third, 0);
        let mut cache = SourceOutputCache::new(2);

        cache.insert(first, output.clone());
        cache.insert(second, output.clone());
        assert!(cache.get(first).is_some());
        cache.insert(third, output);

        assert!(cache.get(first).is_none());
        assert!(cache.get(second).is_some());
        assert!(cache.get(third).is_some());
    }
}
