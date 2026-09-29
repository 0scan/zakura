//! Atomic all-time aggregate transitions.

use rocksdb::WriteBatch;

use crate::{
    database::{DatabaseColumn, MetadataKey},
    models::{ChainStatsRecord, IndexedBlockRecord, TransactionRecord},
    transactions::{shielded_flow, shielded_pool, transaction_kind},
    types::{ShieldedFlow, ShieldedPool, TransactionKind},
    Error, Indexer,
};

/// Transaction classifications accumulated while one block is prepared.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct BlockTransactionStats {
    pub(crate) transparent: u32,
    pub(crate) shielded: u32,
    pub(crate) coinbase: u32,
    pub(crate) fully_shielded: u32,
    pub(crate) mixed_pool: u32,
}

impl BlockTransactionStats {
    pub(crate) fn record(&mut self, transaction: &TransactionRecord) -> Result<(), Error> {
        match transaction_kind(transaction) {
            TransactionKind::Transparent => increment(&mut self.transparent, "transparent")?,
            TransactionKind::Shielded => increment(&mut self.shielded, "shielded")?,
            TransactionKind::Coinbase => increment(&mut self.coinbase, "coinbase")?,
        }
        if shielded_flow(transaction)? == Some(ShieldedFlow::FullyShielded) {
            increment(&mut self.fully_shielded, "fully shielded")?;
        }
        if shielded_pool(transaction) == Some(ShieldedPool::Mixed) {
            increment(&mut self.mixed_pool, "mixed-pool")?;
        }
        Ok(())
    }
}

impl Indexer {
    pub(crate) fn chain_stats_record(&self) -> Result<ChainStatsRecord, Error> {
        self.database
            .get(DatabaseColumn::Metadata, MetadataKey::ChainStats.as_bytes())?
            .map(|bytes| serde_json::from_slice(&bytes).map_err(Error::from))
            .transpose()
            .map(|record| record.unwrap_or_default())
    }

    pub(crate) fn prepare_chain_stats_write(
        &self,
        batch: &mut WriteBatch,
        stats: ChainStatsRecord,
    ) -> Result<(), Error> {
        self.database.insert(
            batch,
            DatabaseColumn::Metadata,
            MetadataKey::ChainStats.as_bytes(),
            serde_json::to_vec(&stats)?,
        );
        Ok(())
    }

    pub(crate) fn add_block_to_chain_stats(
        &self,
        stats: &mut ChainStatsRecord,
        block: &IndexedBlockRecord,
    ) -> Result<(), Error> {
        apply_block(stats, block, AggregateDirection::Add)
    }

    pub(crate) fn remove_block_from_chain_stats(
        &self,
        stats: &mut ChainStatsRecord,
        block: &IndexedBlockRecord,
    ) -> Result<(), Error> {
        apply_block(stats, block, AggregateDirection::Remove)
    }
}

#[derive(Clone, Copy)]
enum AggregateDirection {
    Add,
    Remove,
}

fn apply_block(
    stats: &mut ChainStatsRecord,
    block: &IndexedBlockRecord,
    direction: AggregateDirection,
) -> Result<(), Error> {
    update(&mut stats.block_count, 1, "block count", direction)?;
    update(
        &mut stats.transaction_count,
        u64::from(block.transaction_count),
        "transaction count",
        direction,
    )?;
    update(
        &mut stats.block_bytes,
        u64::from(block.serialized_size),
        "block bytes",
        direction,
    )?;
    update(
        &mut stats.total_fees_zat,
        block.total_fees_zat,
        "total fees",
        direction,
    )?;
    update(
        &mut stats.transparent_transaction_count,
        u64::from(block.transparent_transaction_count),
        "transparent transaction count",
        direction,
    )?;
    update(
        &mut stats.shielded_transaction_count,
        u64::from(block.shielded_transaction_count),
        "shielded transaction count",
        direction,
    )?;
    update(
        &mut stats.coinbase_transaction_count,
        u64::from(block.coinbase_transaction_count),
        "coinbase transaction count",
        direction,
    )?;
    update(
        &mut stats.fully_shielded_transaction_count,
        u64::from(block.fully_shielded_transaction_count),
        "fully shielded transaction count",
        direction,
    )?;
    update(
        &mut stats.mixed_pool_transaction_count,
        u64::from(block.mixed_pool_transaction_count),
        "mixed-pool transaction count",
        direction,
    )
}

fn update(
    current: &mut u64,
    value: u64,
    field: &str,
    direction: AggregateDirection,
) -> Result<(), Error> {
    *current = match direction {
        AggregateDirection::Add => current
            .checked_add(value)
            .ok_or_else(|| Error::Calculation(format!("chain stats {field} exceeds u64")))?,
        AggregateDirection::Remove => current.checked_sub(value).ok_or_else(|| {
            Error::CorruptData(format!("chain stats {field} underflowed during rollback"))
        })?,
    };
    Ok(())
}

fn increment(value: &mut u32, category: &str) -> Result<(), Error> {
    *value = value.checked_add(1).ok_or_else(|| {
        Error::Calculation(format!("block {category} transaction count exceeds u32"))
    })?;
    Ok(())
}
