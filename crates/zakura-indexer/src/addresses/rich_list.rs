//! Balance-ordered transparent-address ranking queries.

use zakura_chain::block::{Hash, Height};

use crate::{
    database::DatabaseColumn,
    types::{
        RichListEntry, RichListPagination, RichListRequest, RichListResponse, RichListSummary,
    },
    Error, Indexer,
};

use super::{
    disk_format::{address_balance_order_key, decode_address_balance_order_key},
    rich_list_cursor::RichListCursor,
};

const DEFAULT_RICH_LIST_LIMIT: u32 = 100;
const MAX_RICH_LIST_LIMIT: u32 = 100;
const SUMMARY_ADDRESS_COUNT: usize = 100;
const MAX_STABLE_READ_ATTEMPTS: usize = 3;

impl Indexer {
    /// Returns a stable cursor page ordered by descending transparent balance.
    pub async fn rich_list(&self, request: RichListRequest) -> Result<RichListResponse, Error> {
        let indexer = self.clone();
        tokio::task::spawn_blocking(move || indexer.rich_list_blocking(request))
            .await
            .map_err(|error| Error::Task(error.to_string()))?
    }

    fn rich_list_blocking(&self, request: RichListRequest) -> Result<RichListResponse, Error> {
        let limit = request.limit.unwrap_or(DEFAULT_RICH_LIST_LIMIT);
        if !(1..=MAX_RICH_LIST_LIMIT).contains(&limit) {
            return Err(Error::InvalidQuery(format!(
                "rich-list limit must be between 1 and {MAX_RICH_LIST_LIMIT}"
            )));
        }
        let cursor = request
            .cursor
            .as_deref()
            .map(RichListCursor::decode)
            .transpose()?;

        for _ in 0..MAX_STABLE_READ_ATTEMPTS {
            let indexed_tip = self.indexed_block_tip()?;
            if let Some(cursor) = cursor {
                let Some((_, indexed_hash)) = indexed_tip else {
                    return Err(Error::InvalidCursor(
                        "rich-list cursor belongs to a non-empty index".to_string(),
                    ));
                };
                if cursor.indexed_block_hash != indexed_hash {
                    return Err(Error::InvalidCursor(
                        "rich-list ranking changed; restart from the first page".to_string(),
                    ));
                }
            }

            let response = self.rich_list_at_tip(limit, cursor, indexed_tip)?;
            if self.indexed_block_tip()? == indexed_tip {
                return Ok(response);
            }
        }

        Err(Error::Task(
            "indexed chain changed repeatedly during rich-list query".to_string(),
        ))
    }

    fn rich_list_at_tip(
        &self,
        limit: u32,
        cursor: Option<RichListCursor>,
        indexed_tip: Option<(Height, Hash)>,
    ) -> Result<RichListResponse, Error> {
        let cursor_key =
            cursor.map(|cursor| address_balance_order_key(cursor.address, cursor.balance_zat));
        if let Some(cursor_key) = cursor_key.as_ref() {
            if self
                .database
                .get(DatabaseColumn::AddressBalanceOrder, cursor_key)?
                .is_none()
            {
                return Err(Error::InvalidCursor(
                    "rich-list cursor entry is no longer funded".to_string(),
                ));
            }
        }

        let limit_usize = usize::try_from(limit)
            .map_err(|_| Error::Calculation("rich-list limit exceeds usize".to_string()))?;
        let scan_limit = limit_usize
            .checked_add(2)
            .ok_or_else(|| Error::Calculation("rich-list scan limit overflow".to_string()))?;
        let mut rows = self.database.scan_forward_from(
            DatabaseColumn::AddressBalanceOrder,
            cursor_key.as_deref().unwrap_or_default(),
            scan_limit,
        )?;
        if let Some(cursor_key) = cursor_key.as_ref() {
            if rows.first().map(|(key, _)| key) != Some(cursor_key) {
                return Err(Error::InvalidCursor(
                    "rich-list cursor entry is missing from its ranking position".to_string(),
                ));
            }
            rows.remove(0);
        }

        let has_next = rows.len() > limit_usize;
        rows.truncate(limit_usize);
        let first_rank = cursor.map_or(Ok(1), |cursor| {
            cursor
                .rank
                .checked_add(1)
                .ok_or_else(|| Error::InvalidCursor("rich-list cursor rank overflow".to_string()))
        })?;
        let entries = rows
            .iter()
            .enumerate()
            .map(|(index, (key, _))| {
                let (address, balance_zat) = decode_address_balance_order_key(key)?;
                let offset = u64::try_from(index).map_err(|_| {
                    Error::Calculation("rich-list page offset exceeds u64".to_string())
                })?;
                let rank = first_rank
                    .checked_add(offset)
                    .ok_or_else(|| Error::Calculation("rich-list rank exceeds u64".to_string()))?;
                Ok(RichListEntry {
                    rank,
                    address: address.to_string(),
                    balance_zat: balance_zat.to_string(),
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;

        let chain_stats = self.chain_stats_record()?;
        let transparent_supply = match indexed_tip {
            Some((_, hash)) => {
                self.indexed_block_record(hash)?
                    .ok_or_else(|| {
                        Error::CorruptData(
                            "indexed tip is missing its rich-list supply record".to_string(),
                        )
                    })?
                    .pool_transparent
            }
            None => 0,
        };
        let summary_rows = self.database.scan_forward_from(
            DatabaseColumn::AddressBalanceOrder,
            &[],
            SUMMARY_ADDRESS_COUNT,
        )?;
        let mut top_10_balance = 0_u64;
        let mut top_100_balance = 0_u64;
        for (index, (key, _)) in summary_rows.iter().enumerate() {
            let (_, balance) = decode_address_balance_order_key(key)?;
            top_100_balance = top_100_balance.checked_add(balance).ok_or_else(|| {
                Error::Calculation("top-100 transparent balance exceeds u64".to_string())
            })?;
            if index < 10 {
                top_10_balance = top_10_balance.checked_add(balance).ok_or_else(|| {
                    Error::Calculation("top-10 transparent balance exceeds u64".to_string())
                })?;
            }
        }

        let next_cursor = if has_next {
            let last = entries.last().ok_or_else(|| {
                Error::CorruptData("non-empty rich-list page has no last entry".to_string())
            })?;
            let (address, balance_zat) = decode_address_balance_order_key(
                &rows
                    .last()
                    .expect("rich-list rows exist because the returned page is non-empty")
                    .0,
            )?;
            let indexed_hash = indexed_tip
                .map(|(_, hash)| hash)
                .ok_or_else(|| Error::CorruptData("funded index has no chain tip".to_string()))?;
            Some(RichListCursor::new(address, balance_zat, last.rank, indexed_hash).encode())
        } else {
            None
        };

        Ok(RichListResponse {
            entries,
            summary: RichListSummary {
                funded_transparent_address_count: chain_stats.funded_transparent_address_count,
                transparent_supply_zat: transparent_supply.to_string(),
                top_10_balance_zat: top_10_balance.to_string(),
                top_10_concentration_percent: concentration_percent(
                    top_10_balance,
                    transparent_supply,
                ),
                top_100_balance_zat: top_100_balance.to_string(),
                top_100_concentration_percent: concentration_percent(
                    top_100_balance,
                    transparent_supply,
                ),
            },
            pagination: RichListPagination {
                limit,
                total: chain_stats.funded_transparent_address_count.to_string(),
                has_next,
                next_cursor,
            },
            indexed_height: indexed_tip.map(|(height, _)| height.0.to_string()),
            indexed_block_hash: indexed_tip.map(|(_, hash)| hash.to_string()),
        })
    }
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
