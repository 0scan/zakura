//! Batched historical source-transaction loading for explorer metadata refill.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
};

use rayon::{prelude::*, ThreadPool};
use zakura_chain::{
    parameters::Network,
    transaction::{self, primary_value_endpoints, Transaction},
    transparent::Output,
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

/// A bounded FIFO cache of decoded transactions referenced by transparent inputs.
///
/// The refill scans spending transactions in chain order, so recently loaded sources are the most
/// useful across adjacent chunks. FIFO eviction keeps cache operations constant-time without a
/// shared lock in the parallel derivation path.
pub(super) struct SourceTransactionCache {
    capacity: usize,
    transactions: HashMap<transaction::Hash, Arc<Transaction>>,
    insertion_order: VecDeque<transaction::Hash>,
}

impl SourceTransactionCache {
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            capacity,
            transactions: HashMap::with_capacity(capacity),
            insertion_order: VecDeque::with_capacity(capacity),
        }
    }

    fn get(&self, hash: transaction::Hash) -> Option<Arc<Transaction>> {
        self.transactions.get(&hash).cloned()
    }

    fn insert(&mut self, hash: transaction::Hash, transaction: Arc<Transaction>) {
        if self.capacity == 0 || self.transactions.contains_key(&hash) {
            return;
        }

        while self.transactions.len() >= self.capacity {
            let evicted_hash = self
                .insertion_order
                .pop_front()
                .expect("a full source transaction cache has an insertion-order entry");
            self.transactions.remove(&evicted_hash);
        }

        self.transactions.insert(hash, transaction);
        self.insertion_order.push_back(hash);
    }
}

/// Upgrades a chunk after loading each distinct transparent source transaction at most once.
pub(super) fn upgrade_transaction_entries(
    db: &ZakuraDb,
    transaction_cf: &TypedColumnFamily<'_, TransactionLocation, Transaction>,
    network: &Network,
    worker_pool: &ThreadPool,
    source_cache: &mut SourceTransactionCache,
    entries: &[LegacyTransactionEntry],
) -> Result<Vec<UpgradedTransactionEntry>, RefillTransactionAmountsError> {
    let source_hashes = distinct_source_hashes(entries);
    let mut source_transactions = HashMap::with_capacity(source_hashes.len());
    let mut missing_hashes = Vec::new();
    for hash in source_hashes {
        if let Some(transaction) = source_cache.get(hash) {
            source_transactions.insert(hash, transaction);
        } else {
            missing_hashes.push(hash);
        }
    }

    let loaded_transactions = worker_pool.install(|| {
        missing_hashes
            .par_iter()
            .map(|hash| load_source_transaction(db, transaction_cf, *hash))
            .collect::<Vec<_>>()
    });
    for loaded_transaction in loaded_transactions {
        let (hash, transaction) = loaded_transaction?;
        source_cache.insert(hash, transaction.clone());
        source_transactions.insert(hash, transaction);
    }

    worker_pool.install(|| {
        entries
            .par_iter()
            .map(|entry| upgrade_transaction_entry(entry, network, &source_transactions))
            .collect()
    })
}

fn distinct_source_hashes(entries: &[LegacyTransactionEntry]) -> Vec<transaction::Hash> {
    let mut seen = HashSet::new();
    entries
        .iter()
        .flat_map(|entry| entry.transaction.spent_outpoints())
        .map(|outpoint| outpoint.hash)
        .filter(|hash| seen.insert(*hash))
        .collect()
}

fn load_source_transaction(
    db: &ZakuraDb,
    transaction_cf: &TypedColumnFamily<'_, TransactionLocation, Transaction>,
    hash: transaction::Hash,
) -> Result<(transaction::Hash, Arc<Transaction>), RefillTransactionAmountsError> {
    let location = db
        .transaction_location(hash)
        .ok_or(RefillTransactionAmountsError::MissingSpentTransactionLocation(hash))?;
    let transaction = transaction_cf
        .zs_get(&location)
        .ok_or(RefillTransactionAmountsError::MissingSpentTransaction { hash, location })?;

    Ok((hash, Arc::new(transaction)))
}

fn upgrade_transaction_entry(
    entry: &LegacyTransactionEntry,
    network: &Network,
    source_transactions: &HashMap<transaction::Hash, Arc<Transaction>>,
) -> Result<UpgradedTransactionEntry, RefillTransactionAmountsError> {
    let spent_outputs = entry
        .transaction
        .spent_outpoints()
        .map(|outpoint| {
            let source_transaction = source_transactions
                .get(&outpoint.hash)
                .expect("every distinct transparent source was loaded for this chunk");
            let output_index = usize::try_from(outpoint.index)
                .expect("u32 output indexes fit in usize on supported targets");
            source_transaction
                .outputs()
                .get(output_index)
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
    fn source_transaction_cache_evicts_oldest_insertions() {
        let transaction = Arc::new(
            regtest_genesis_block()
                .transactions
                .first()
                .expect("genesis block has a coinbase transaction")
                .as_ref()
                .clone(),
        );
        let first = Hash([1; 32]);
        let second = Hash([2; 32]);
        let third = Hash([3; 32]);
        let mut cache = SourceTransactionCache::new(2);

        cache.insert(first, transaction.clone());
        cache.insert(second, transaction.clone());
        assert!(cache.get(first).is_some());
        cache.insert(third, transaction);

        assert!(cache.get(first).is_none());
        assert!(cache.get(second).is_some());
        assert!(cache.get(third).is_some());
    }
}
