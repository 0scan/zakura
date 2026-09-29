//! Transparent balance ranking adapter backed by canonical state.

use tower::ServiceExt;
use zakura_state::{ExplorerBalanceRankCursor, ReadRequest, ReadResponse, ReadState};

use crate::{
    types::{
        TopBalanceEntry, TopBalancesPagination, TopBalancesRequest, TopBalancesResponse,
        TopBalancesSummary,
    },
    Error,
};

use super::top_balances_cursor::TopBalancesCursor;

const DEFAULT_TOP_BALANCES_LIMIT: u32 = 100;
const MAX_TOP_BALANCES_LIMIT: u32 = 100;

/// Returns a stable cursor page ordered by finalized transparent balance.
pub async fn top_balances_from_state<State>(
    read_state: State,
    request: TopBalancesRequest,
) -> Result<TopBalancesResponse, Error>
where
    State: ReadState,
{
    let limit = request.limit.unwrap_or(DEFAULT_TOP_BALANCES_LIMIT);
    if !(1..=MAX_TOP_BALANCES_LIMIT).contains(&limit) {
        return Err(Error::InvalidQuery(format!(
            "top-balances limit must be between 1 and {MAX_TOP_BALANCES_LIMIT}"
        )));
    }
    let cursor = request
        .cursor
        .as_deref()
        .map(TopBalancesCursor::decode)
        .transpose()?;
    let state_cursor = cursor.map(|cursor| ExplorerBalanceRankCursor {
        address: cursor.address,
        balance_zat: cursor.balance_zat,
        rank: cursor.rank,
        block_hash: cursor.indexed_block_hash,
    });
    let response = read_state
        .oneshot(ReadRequest::ExplorerBalanceRankPage {
            limit,
            cursor: state_cursor,
        })
        .await
        .map_err(|error| Error::StateRequest(error.to_string()))?;
    let ReadResponse::ExplorerBalanceRankPage(page) = response else {
        return Err(Error::StateResponse(
            "state returned the wrong response for top balances".to_string(),
        ));
    };
    if !page.cursor_valid {
        return Err(Error::InvalidCursor(
            "top-balances ranking changed; restart from the first page".to_string(),
        ));
    }

    let first_rank = cursor.map_or(1, |cursor| {
        cursor
            .rank
            .checked_add(1)
            .expect("validated top-balances rank fits in u64")
    });
    let entries = page
        .entries
        .iter()
        .enumerate()
        .map(|(offset, entry)| TopBalanceEntry {
            rank: first_rank
                .checked_add(u64::try_from(offset).expect("page offset fits in u64"))
                .expect("top-balances rank fits in u64"),
            address: entry.address.to_string(),
            balance_zat: entry.balance_zat.to_string(),
        })
        .collect::<Vec<_>>();
    let next_cursor = if page.has_more {
        let last_entry = page
            .entries
            .last()
            .expect("a page with more rows contains a last returned entry");
        let last_rank = entries
            .last()
            .expect("a page with more rows contains a ranked entry")
            .rank;
        let block_hash = page
            .best_tip
            .map(|(_, hash)| hash)
            .expect("a funded balance ranking has a finalized tip");
        Some(
            TopBalancesCursor::new(
                last_entry.address,
                last_entry.balance_zat,
                last_rank,
                block_hash,
            )
            .encode(),
        )
    } else {
        None
    };

    Ok(TopBalancesResponse {
        entries,
        summary: TopBalancesSummary {
            funded_transparent_address_count: page.funded_transparent_address_count,
            transparent_supply_zat: page.transparent_supply_zat.to_string(),
            top_10_balance_zat: page.top_10_balance_zat.to_string(),
            top_10_concentration_percent: concentration_percent(
                page.top_10_balance_zat,
                page.transparent_supply_zat,
            ),
            top_100_balance_zat: page.top_100_balance_zat.to_string(),
            top_100_concentration_percent: concentration_percent(
                page.top_100_balance_zat,
                page.transparent_supply_zat,
            ),
        },
        pagination: TopBalancesPagination {
            limit,
            total: page.funded_transparent_address_count.to_string(),
            has_next: page.has_more,
            next_cursor,
        },
        indexed_height: page.best_tip.map(|(height, _)| height.0.to_string()),
        indexed_block_hash: page.best_tip.map(|(_, hash)| hash.to_string()),
    })
}

fn concentration_percent(balance: u64, transparent_supply: u64) -> String {
    if transparent_supply == 0 {
        return "0.00".to_string();
    }
    let scaled = u128::from(balance)
        .saturating_mul(10_000)
        .saturating_add(u128::from(transparent_supply) / 2)
        / u128::from(transparent_supply);
    format!("{}.{:02}", scaled / 100, scaled % 100)
}

#[cfg(test)]
mod tests {
    use super::concentration_percent;

    #[test]
    fn concentration_is_rounded_to_two_decimal_places() {
        assert_eq!(concentration_percent(2_580_000, 12_000_000), "21.50");
        assert_eq!(concentration_percent(0, 0), "0.00");
    }
}
