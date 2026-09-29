//! Bounded statistics queries over persisted aggregates and recent blocks.

use crate::{
    models::ChainStatsRecord,
    types::{ChainTotals, IndexerStats, RollingDayStats},
    Error, Indexer,
};

const ROLLING_DAY_SECONDS: i64 = 24 * 60 * 60;
const MAX_ROLLING_WINDOW_BLOCKS: u32 = 10_000;

impl Indexer {
    /// Returns the highest canonical block covered by the index without scanning statistics.
    pub async fn indexed_tip(
        &self,
    ) -> Result<Option<(zakura_chain::block::Height, zakura_chain::block::Hash)>, Error> {
        let indexer = self.clone();
        tokio::task::spawn_blocking(move || indexer.indexed_block_tip())
            .await
            .map_err(|error| Error::Task(error.to_string()))?
    }

    /// Returns all-time canonical totals and a bounded trailing 24-hour snapshot.
    pub async fn stats(&self) -> Result<IndexerStats, Error> {
        let indexer = self.clone();
        tokio::task::spawn_blocking(move || indexer.stats_blocking())
            .await
            .map_err(|error| Error::Task(error.to_string()))?
    }

    fn stats_blocking(&self) -> Result<IndexerStats, Error> {
        let indexed_tip = self.indexed_block_tip()?;
        let totals = chain_totals(self.chain_stats_record()?);
        let trailing_24h = self.trailing_day_stats(indexed_tip.map(|(height, _)| height))?;

        Ok(IndexerStats {
            indexed_height: indexed_tip.map(|(height, _)| height.0.to_string()),
            indexed_block_hash: indexed_tip.map(|(_, hash)| hash.to_string()),
            totals,
            trailing_24h,
        })
    }

    fn trailing_day_stats(
        &self,
        tip_height: Option<zakura_chain::block::Height>,
    ) -> Result<RollingDayStats, Error> {
        let Some(tip_height) = tip_height else {
            return Ok(empty_rolling_day());
        };
        let tip_hash = self.canonical_block_hash(tip_height)?.ok_or_else(|| {
            Error::CorruptData("indexed tip is missing its canonical block hash".to_string())
        })?;
        let tip = self.indexed_block_record(tip_hash)?.ok_or_else(|| {
            Error::CorruptData("indexed tip is missing its block record".to_string())
        })?;
        let window_end = tip.timestamp;
        let window_start = window_end.saturating_sub(ROLLING_DAY_SECONDS);
        let mut height = tip_height;
        let mut block_count = 0_u64;
        let mut transaction_count = 0_u64;
        let mut block_bytes = 0_u64;
        let mut total_fees_zat = 0_u64;
        let mut oldest_timestamp = window_end;
        let mut complete = false;

        for _ in 0..MAX_ROLLING_WINDOW_BLOCKS {
            let hash = self.canonical_block_hash(height)?.ok_or_else(|| {
                Error::CorruptData(format!(
                    "missing canonical block hash at height {}",
                    height.0
                ))
            })?;
            let block = self.indexed_block_record(hash)?.ok_or_else(|| {
                Error::CorruptData(format!("missing indexed block record for {hash}"))
            })?;
            if block.timestamp < window_start {
                complete = true;
                break;
            }

            block_count = checked_add(block_count, 1, "rolling block count")?;
            transaction_count = checked_add(
                transaction_count,
                u64::from(block.transaction_count),
                "rolling transaction count",
            )?;
            block_bytes = checked_add(
                block_bytes,
                u64::from(block.serialized_size),
                "rolling block bytes",
            )?;
            total_fees_zat =
                checked_add(total_fees_zat, block.total_fees_zat, "rolling total fees")?;
            oldest_timestamp = oldest_timestamp.min(block.timestamp);

            match height.previous() {
                Ok(previous) => height = previous,
                Err(_) => {
                    complete = true;
                    break;
                }
            }
        }

        let elapsed = u64::try_from(window_end.saturating_sub(oldest_timestamp).max(0))
            .map_err(|_| Error::Calculation("rolling block duration exceeds u64".to_string()))?;
        let intervals = block_count.saturating_sub(1);
        Ok(RollingDayStats {
            complete,
            window_start: Some(window_start.to_string()),
            window_end: Some(window_end.to_string()),
            block_count: block_count.to_string(),
            transaction_count: transaction_count.to_string(),
            block_bytes: block_bytes.to_string(),
            total_fees_zat: total_fees_zat.to_string(),
            average_block_time_seconds: ratio(u128::from(elapsed), intervals),
            average_block_size_bytes: ratio(u128::from(block_bytes), block_count),
            average_transactions_per_block: ratio(u128::from(transaction_count), block_count),
        })
    }
}

fn chain_totals(record: ChainStatsRecord) -> ChainTotals {
    ChainTotals {
        block_count: record.block_count.to_string(),
        transaction_count: record.transaction_count.to_string(),
        block_bytes: record.block_bytes.to_string(),
        total_fees_zat: record.total_fees_zat.to_string(),
        transparent_transaction_count: record.transparent_transaction_count.to_string(),
        shielded_transaction_count: record.shielded_transaction_count.to_string(),
        coinbase_transaction_count: record.coinbase_transaction_count.to_string(),
        fully_shielded_transaction_count: record.fully_shielded_transaction_count.to_string(),
        mixed_pool_transaction_count: record.mixed_pool_transaction_count.to_string(),
    }
}

fn empty_rolling_day() -> RollingDayStats {
    RollingDayStats {
        complete: true,
        window_start: None,
        window_end: None,
        block_count: "0".to_string(),
        transaction_count: "0".to_string(),
        block_bytes: "0".to_string(),
        total_fees_zat: "0".to_string(),
        average_block_time_seconds: None,
        average_block_size_bytes: None,
        average_transactions_per_block: None,
    }
}

fn checked_add(current: u64, value: u64, field: &str) -> Result<u64, Error> {
    current
        .checked_add(value)
        .ok_or_else(|| Error::Calculation(format!("{field} exceeds u64")))
}

fn ratio(numerator: u128, denominator: u64) -> Option<String> {
    (denominator > 0).then(|| {
        let denominator = u128::from(denominator);
        let tenths = numerator.saturating_mul(10) / denominator;
        format!("{}.{:01}", tenths / 10, tenths % 10)
    })
}

#[cfg(test)]
mod tests {
    use super::ratio;

    #[test]
    fn ratio_formats_one_decimal_without_floating_point() {
        assert_eq!(ratio(10, 4), Some("2.5".to_string()));
        assert_eq!(ratio(1, 0), None);
    }
}
