//! Finalized explorer analytics reads, writes, and rollback updates.

use std::collections::{BTreeSet, HashMap};

use zakura_chain::{
    parameters::Network,
    transparent::{OutPoint, Utxo},
    value_balance::ValueBalance,
};

use crate::{
    request::FinalizedBlock,
    service::finalized_state::{DiskWriteBatch, ZakuraDb},
};

use super::disk_format::ExplorerDayKey;

impl ZakuraDb {
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
}

impl DiskWriteBatch {
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
        let block = crate::explorer::analytics::derive_block_stats(
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
        crate::explorer::analytics::add_block_to_chain_stats(&mut chain, &block);

        let day = crate::explorer::analytics::day_number(block.timestamp);
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
        crate::explorer::analytics::add_block_to_daily_stats(
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
            crate::explorer::analytics::remove_block_from_chain_stats(&mut chain, &block);
            affected_days.insert(crate::explorer::analytics::day_number(block.timestamp));
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
                if crate::explorer::analytics::day_number(block.timestamp) == day {
                    crate::explorer::analytics::add_block_to_daily_stats(
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
}
