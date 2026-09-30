//! Explorer statistics adapters backed by canonical state.

use tower::ServiceExt;
use zakura_state::{
    ExplorerReadRequest, ExplorerReadResponse, ReadRequest, ReadResponse, ReadState,
};

use crate::{
    types::{ChainTotals, IndexerStats, RollingDayStats},
    Error,
};

/// Returns finalized all-time totals and a bounded trailing 24-hour snapshot.
pub async fn stats_from_state<State>(read_state: State) -> Result<IndexerStats, Error>
where
    State: ReadState,
{
    let response = read_state
        .oneshot(ReadRequest::Explorer(ExplorerReadRequest::StatsSnapshot))
        .await
        .map_err(|error| Error::StateRequest(error.to_string()))?;
    let ReadResponse::Explorer(ExplorerReadResponse::StatsSnapshot(snapshot)) = response else {
        return Err(Error::StateResponse(
            "state returned the wrong response for explorer statistics".to_string(),
        ));
    };
    let rolling = snapshot.trailing_24h;
    let elapsed = rolling
        .window_end
        .zip(rolling.oldest_timestamp)
        .map(|(end, oldest)| end.saturating_sub(oldest).max(0))
        .and_then(|value| u128::try_from(value).ok())
        .unwrap_or_default();
    let intervals = rolling.totals.block_count.saturating_sub(1);

    Ok(IndexerStats {
        indexed_height: snapshot.best_tip.map(|(height, _)| height.0.to_string()),
        indexed_block_hash: snapshot.best_tip.map(|(_, hash)| hash.to_string()),
        totals: chain_totals(snapshot.totals),
        trailing_24h: RollingDayStats {
            complete: rolling.complete,
            window_start: rolling.window_start.map(|value| value.to_string()),
            window_end: rolling.window_end.map(|value| value.to_string()),
            block_count: rolling.totals.block_count.to_string(),
            transaction_count: rolling.totals.transaction_count.to_string(),
            transparent_transaction_count: rolling.totals.transparent_transaction_count.to_string(),
            shielded_transaction_count: rolling.totals.shielded_transaction_count.to_string(),
            coinbase_transaction_count: rolling.totals.coinbase_transaction_count.to_string(),
            block_bytes: rolling.totals.block_bytes.to_string(),
            total_fees_zat: rolling.totals.total_fees_zat.to_string(),
            average_block_time_seconds: ratio(elapsed, intervals),
            average_block_size_bytes: ratio(
                u128::from(rolling.totals.block_bytes),
                rolling.totals.block_count,
            ),
            average_transactions_per_block: ratio(
                u128::from(rolling.totals.transaction_count),
                rolling.totals.block_count,
            ),
        },
    })
}

fn chain_totals(record: zakura_state::ExplorerChainStats) -> ChainTotals {
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
