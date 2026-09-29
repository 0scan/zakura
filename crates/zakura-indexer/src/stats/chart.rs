//! Date-range chart snapshot queries.

use zakura_chain::{block::Height, parameters::NetworkUpgrade};

use super::disk_format::{date_to_day, day_to_date};
use crate::{
    database::DatabaseColumn,
    models::DailyStatsRecord,
    stats::day_key,
    types::{ChartDataEntry, ChartDataRequest, ChartDataResponse},
    Error, Indexer,
};

const DEFAULT_CHART_DATA_RESULTS: usize = 10_000;
const MAX_CHART_DATA_RESULTS: usize = 10_000;

impl Indexer {
    /// Returns daily chart snapshots in ascending UTC date order.
    pub async fn chart_data(&self, request: ChartDataRequest) -> Result<ChartDataResponse, Error> {
        let indexer = self.clone();
        tokio::task::spawn_blocking(move || indexer.chart_data_blocking(request))
            .await
            .map_err(|error| Error::Task(error.to_string()))?
    }

    fn chart_data_blocking(&self, request: ChartDataRequest) -> Result<ChartDataResponse, Error> {
        let limit = request.limit.unwrap_or(DEFAULT_CHART_DATA_RESULTS);
        if !(1..=MAX_CHART_DATA_RESULTS).contains(&limit) {
            return Err(Error::InvalidQuery(format!(
                "chart limit must be between 1 and {MAX_CHART_DATA_RESULTS}"
            )));
        }
        let start = request
            .start_date
            .as_deref()
            .map(date_to_day)
            .transpose()?
            .unwrap_or(u32::MIN);
        let end = request
            .end_date
            .as_deref()
            .map(date_to_day)
            .transpose()?
            .unwrap_or(u32::MAX);
        if start > end {
            return Err(Error::InvalidQuery(
                "chart start_date must not be after end_date".to_string(),
            ));
        }

        let mut rows = self.database.scan_range_forward(
            DatabaseColumn::DailyStats,
            &day_key(start),
            &day_key(end),
            limit.saturating_add(1),
        )?;
        let next_start_date = if rows.len() > limit {
            let key: [u8; 4] = rows[limit].0.as_slice().try_into().map_err(|_| {
                Error::CorruptData("daily stats key must be four bytes".to_string())
            })?;
            rows.truncate(limit);
            Some(day_to_date(u32::from_be_bytes(key))?)
        } else {
            None
        };

        let entries = rows
            .into_iter()
            .map(|(key, value)| {
                let key: [u8; 4] = key.as_slice().try_into().map_err(|_| {
                    Error::CorruptData("daily stats key must be four bytes".to_string())
                })?;
                let record: DailyStatsRecord = serde_json::from_slice(&value)?;
                self.chart_entry(u32::from_be_bytes(key), record)
            })
            .collect::<Result<Vec<_>, Error>>()?;

        Ok(ChartDataResponse {
            entries,
            next_start_date,
        })
    }

    fn chart_entry(&self, day: u32, record: DailyStatsRecord) -> Result<ChartDataEntry, Error> {
        let interval = record.interval;
        let elapsed = record
            .block_timestamp
            .saturating_sub(record.interval_anchor_timestamp)
            .max(0);
        let elapsed = u128::try_from(elapsed)
            .map_err(|_| Error::Calculation("chart block interval exceeds u128".to_string()))?;
        let average_block_time =
            decimal_ratio(elapsed, u128::from(record.block_time_interval_count), 3);
        let average_block_fee_zat = interval
            .total_fees_zat
            .checked_div(u128::from(interval.block_count))
            .unwrap_or_default();
        let average_block_size = interval
            .total_block_size_bytes
            .checked_div(interval.block_count)
            .map(u32::try_from)
            .transpose()
            .map_err(|_| Error::Calculation("average block size exceeds u32".to_string()))?
            .unwrap_or_default();
        let blocks_per_year = if NetworkUpgrade::Blossom
            .activation_height(&self.network)
            .is_some_and(|height| Height(record.end_height) >= height)
        {
            420_480_u128
        } else {
            210_240_u128
        };
        let annual_percent_numerator = u128::from(record.end_block_subsidy_zat)
            .checked_mul(blocks_per_year)
            .and_then(|value| value.checked_mul(100))
            .ok_or_else(|| Error::Calculation("inflation rate exceeds u128".to_string()))?;
        let inflation_rate_percent = decimal_ratio_rounded(
            annual_percent_numerator,
            u128::from(record.total_issuance),
            2,
        );
        let header_elapsed = record
            .max_header_timestamp
            .saturating_sub(record.min_header_timestamp);
        let header_elapsed = u64::try_from(header_elapsed.max(0))
            .map_err(|_| Error::Calculation("header timestamp range exceeds u64".to_string()))?;

        Ok(ChartDataEntry {
            date_key: day_to_date(day)?,
            height: record.end_height,
            funded_transparent_address_count: record.funded_transparent_address_count,
            pool_transparent: record.pool_transparent.to_string(),
            pool_sprout: record.pool_sprout.to_string(),
            pool_sapling: record.pool_sapling.to_string(),
            pool_orchard: record.pool_orchard.to_string(),
            pool_deferred: record.pool_deferred.to_string(),
            pool_ironwood: record.pool_ironwood.to_string(),
            difficulty: record.difficulty,
            total_issuance: record.total_issuance.to_string(),
            inflation_rate_percent,
            block_timestamp: record.block_timestamp,
            transparent_tx_count: count_u32(interval.transparent_tx_count, "transparent")?,
            transparent_coinbase_tx_count: count_u32(
                interval.transparent_coinbase_tx_count,
                "transparent coinbase",
            )?,
            shielded_coinbase_migration_tx_count: count_u32(
                interval.shielded_coinbase_migration_tx_count,
                "shielded coinbase migration",
            )?,
            sprout_tx_count: count_u32(interval.sprout_tx_count, "Sprout")?,
            sapling_tx_count: count_u32(interval.sapling_tx_count, "Sapling")?,
            orchard_tx_count: count_u32(interval.orchard_tx_count, "Orchard")?,
            ironwood_tx_count: count_u32(interval.ironwood_tx_count, "Ironwood")?,
            transparent_inflow: interval.transparent_inflow.to_string(),
            transparent_outflow: interval.transparent_outflow.to_string(),
            sprout_inflow: interval.sprout_inflow.to_string(),
            sprout_outflow: interval.sprout_outflow.to_string(),
            sapling_inflow: interval.sapling_inflow.to_string(),
            sapling_outflow: interval.sapling_outflow.to_string(),
            orchard_inflow: interval.orchard_inflow.to_string(),
            orchard_outflow: interval.orchard_outflow.to_string(),
            ironwood_inflow: interval.ironwood_inflow.to_string(),
            ironwood_outflow: interval.ironwood_outflow.to_string(),
            average_block_time,
            average_block_fee_zat: average_block_fee_zat.to_string(),
            average_block_size,
            interval_block_count: interval.block_count,
            interval_transaction_count: interval.transaction_count,
            interval_empty_block_count: interval.empty_block_count,
            interval_min_header_timestamp: record.min_header_timestamp,
            interval_max_header_timestamp: record.max_header_timestamp,
            interval_elapsed_header_time_seconds: header_elapsed,
            interval_accepted_work: interval.accepted_work.to_string(),
            interval_total_fees_zat: interval.total_fees_zat.to_string(),
            interval_total_block_size_bytes: interval.total_block_size_bytes,
            interval_total_subsidy_zat: interval.total_subsidy_zat.to_string(),
            interval_miner_subsidy_zat: interval.miner_subsidy_zat.to_string(),
            interval_founders_reward_zat: interval.founders_reward_zat.to_string(),
            interval_funding_streams_zat: interval.funding_streams_zat.to_string(),
            interval_deferred_subsidy_zat: interval.deferred_subsidy_zat.to_string(),
            interval_lockbox_disbursement_zat: interval.lockbox_disbursement_zat.to_string(),
            coinbase_output_transparent_zat: interval.coinbase_output_transparent_zat.to_string(),
            coinbase_output_sapling_zat: interval.coinbase_output_sapling_zat.to_string(),
            coinbase_output_orchard_zat: interval.coinbase_output_orchard_zat.to_string(),
            coinbase_output_ironwood_zat: interval.coinbase_output_ironwood_zat.to_string(),
            coinbase_unclaimed_zat: interval.coinbase_unclaimed_zat.to_string(),
            interval_v6_transaction_count: interval.v6_transaction_count,
            interval_ironwood_bundle_transaction_count: interval.ironwood_bundle_transaction_count,
            interval_orchard_bundle_transaction_count: interval.orchard_bundle_transaction_count,
            interval_orchard_ironwood_transaction_count: interval
                .orchard_ironwood_transaction_count,
            interval_orchard_action_count: interval.orchard_action_count,
            interval_ironwood_action_count: interval.ironwood_action_count,
            interval_ironwood_active_block_count: interval.ironwood_active_block_count,
            interval_observable_orchard_to_ironwood_transaction_count: interval
                .observable_orchard_to_ironwood_transaction_count,
            interval_observable_orchard_to_ironwood_value_zat: interval
                .observable_orchard_to_ironwood_value_zat
                .to_string(),
            interval_zip318_action_shape_transaction_count: interval
                .zip318_action_shape_transaction_count,
            interval_zip318_denomination_transaction_count: interval
                .zip318_denomination_transaction_count,
            interval_zip318_fee_transaction_count: interval.zip318_fee_transaction_count,
            interval_zip318_schedule_transaction_count: interval.zip318_schedule_transaction_count,
            interval_ironwood_canonical_denomination_counts: interval
                .ironwood_canonical_denomination_counts
                .to_vec(),
        })
    }
}

fn count_u32(value: u64, name: &str) -> Result<u32, Error> {
    u32::try_from(value)
        .map_err(|_| Error::Calculation(format!("daily {name} transaction count exceeds u32")))
}

fn decimal_ratio(numerator: u128, denominator: u128, precision: u32) -> String {
    if denominator == 0 {
        return "0".to_string();
    }
    let scale = 10_u128.pow(precision);
    let scaled = numerator.saturating_mul(scale) / denominator;
    let whole = scaled / scale;
    let fraction = scaled % scale;
    if precision == 0 {
        whole.to_string()
    } else {
        let width = usize::try_from(precision).expect("decimal precision fits usize");
        format!("{whole}.{fraction:0width$}")
    }
}

fn decimal_ratio_rounded(numerator: u128, denominator: u128, precision: u32) -> String {
    if denominator == 0 {
        return "0".to_string();
    }
    let scale = 10_u128.pow(precision);
    let scaled = numerator
        .saturating_mul(scale)
        .saturating_add(denominator / 2)
        / denominator;
    let whole = scaled / scale;
    let fraction = scaled % scale;
    if precision == 0 {
        whole.to_string()
    } else {
        let width = usize::try_from(precision).expect("decimal precision fits usize");
        format!("{whole}.{fraction:0width$}")
    }
}

#[cfg(test)]
mod tests {
    use rocksdb::WriteBatch;
    use zakura_chain::parameters::{testnet::RegtestParameters, Network};

    use super::*;

    #[tokio::test]
    async fn chart_dates_are_inclusive_and_paginated_with_a_stable_cursor() {
        let indexer = Indexer::open_ephemeral(Network::new_regtest(RegtestParameters::default()))
            .expect("ephemeral index should open");
        let mut batch = WriteBatch::default();
        for (day, height) in [(19_000, 10), (19_001, 20), (19_002, 30)] {
            let record = DailyStatsRecord {
                end_height: height,
                interval: crate::models::IntervalStatsRecord {
                    block_count: 1,
                    ..Default::default()
                },
                ..Default::default()
            };
            indexer
                .prepare_daily_stats_write(&mut batch, day, &record)
                .expect("daily test record should serialize");
        }
        indexer
            .database
            .write(batch)
            .expect("daily test records should commit");

        let start_date = day_to_date(19_000).unwrap();
        let end_date = day_to_date(19_002).unwrap();
        let first = indexer
            .chart_data(ChartDataRequest {
                start_date: Some(start_date),
                end_date: Some(end_date.clone()),
                limit: Some(2),
            })
            .await
            .expect("first chart page should succeed");
        assert_eq!(
            first
                .entries
                .iter()
                .map(|entry| entry.height)
                .collect::<Vec<_>>(),
            [10, 20]
        );
        assert_eq!(
            first.next_start_date.as_deref(),
            Some(day_to_date(19_002).unwrap().as_str())
        );

        let second = indexer
            .chart_data(ChartDataRequest {
                start_date: first.next_start_date,
                end_date: Some(end_date),
                limit: Some(2),
            })
            .await
            .expect("second chart page should succeed");
        assert_eq!(second.entries.len(), 1);
        assert_eq!(second.entries[0].height, 30);
        assert!(second.next_start_date.is_none());
    }
}
