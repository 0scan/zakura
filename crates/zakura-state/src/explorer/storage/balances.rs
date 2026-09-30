//! Finalized transparent balance ranking reads and writes.

use std::collections::HashMap;

use crate::{
    service::finalized_state::{
        AddressBalanceLocation, AddressBalanceLocationUpdates, DiskWriteBatch, ZakuraDb,
    },
    ExplorerBalanceRankEntry,
};

use super::disk_format::ExplorerBalanceKey;

impl ZakuraDb {
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
        balances: &HashMap<zakura_chain::transparent::Address, Option<AddressBalanceLocation>>,
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use zakura_chain::{
        amount::{Amount, NonNegative},
        block::Height,
        parameters::{Network, NetworkKind},
        transparent::Address,
    };

    use crate::{
        constants::{state_database_format_version_in_code, STATE_DATABASE_KIND},
        service::finalized_state::{OutputLocation, STATE_COLUMN_FAMILIES_IN_CODE},
        Config,
    };

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
