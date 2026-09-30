//! Explorer column-family registration and typed accessors.

use crate::service::finalized_state::{
    DiskWriteBatch, TransactionLocation, TypedColumnFamily, ZakuraDb,
};

use super::disk_format::{
    ExplorerAddressKey, ExplorerAddressRecord, ExplorerBalanceKey, ExplorerDayKey,
    ExplorerSchemaVersion, ExplorerShieldedClassLocation, ExplorerTransactionKindLocation,
    ExplorerTransactionRecord,
};

/// Fixed-width metadata keyed once per finalized transaction.
pub const EXPLORER_TRANSACTION_META_BY_LOC: &str = "explorer_tx_meta_by_loc";
/// Explorer-owned schema marker, independent from the canonical state format version.
pub const EXPLORER_SCHEMA: &str = "explorer_schema";
/// One chain-ordered key per finalized transaction, partitioned by kind.
pub const EXPLORER_TRANSACTION_BY_KIND_LOC: &str = "explorer_tx_by_kind_loc";
/// One exact flow/pool/amount-bucket key per finalized shielded transaction.
pub const EXPLORER_SHIELDED_TRANSACTION_BY_CLASS_LOC: &str = "explorer_shielded_tx_by_class_loc";
/// Compact address activity positions keyed by transparent address.
pub const EXPLORER_ADDRESS_META: &str = "explorer_address_meta";
/// Analytics facts keyed by canonical block height.
pub const EXPLORER_BLOCK_STATS: &str = "explorer_block_stats";
/// One canonical all-time aggregate value.
pub const EXPLORER_CHAIN_STATS: &str = "explorer_chain_stats";
/// UTC daily analytics snapshots.
pub const EXPLORER_DAILY_STATS: &str = "explorer_daily_stats";
/// Funded transparent addresses ordered by descending balance.
pub const EXPLORER_BALANCE_ORDER: &str = "explorer_balance_order";

pub(super) type ExplorerTransactionMetaCf<'cf> =
    TypedColumnFamily<'cf, TransactionLocation, ExplorerTransactionRecord>;
type ExplorerSchemaCf<'cf> = TypedColumnFamily<'cf, (), ExplorerSchemaVersion>;
pub(super) type ExplorerTransactionKindCf<'cf> =
    TypedColumnFamily<'cf, ExplorerTransactionKindLocation, ()>;
pub(super) type ExplorerShieldedClassCf<'cf> =
    TypedColumnFamily<'cf, ExplorerShieldedClassLocation, ()>;
pub(super) type ExplorerAddressMetaCf<'cf> =
    TypedColumnFamily<'cf, ExplorerAddressKey, ExplorerAddressRecord>;
pub(super) type ExplorerBlockStatsCf<'cf> =
    TypedColumnFamily<'cf, zakura_chain::block::Height, crate::ExplorerBlockStats>;
pub(super) type ExplorerChainStatsCf<'cf> = TypedColumnFamily<'cf, (), crate::ExplorerChainStats>;
pub(super) type ExplorerDailyStatsCf<'cf> =
    TypedColumnFamily<'cf, ExplorerDayKey, crate::ExplorerDailyStats>;
pub(super) type ExplorerBalanceOrderCf<'cf> = TypedColumnFamily<'cf, ExplorerBalanceKey, ()>;

impl ZakuraDb {
    fn explorer_schema_cf(&self) -> ExplorerSchemaCf<'_> {
        ExplorerSchemaCf::new(self.disk_db(), EXPLORER_SCHEMA)
            .expect("explorer schema column family is registered")
    }

    /// Validates an existing explorer schema or initializes a new empty database.
    pub(crate) fn ensure_explorer_schema(
        &self,
        read_only: bool,
    ) -> Result<(), crate::StateInitError> {
        match self.explorer_schema_cf().zs_get(&()) {
            Some(version) if version == ExplorerSchemaVersion::CURRENT => Ok(()),
            Some(version) => Err(crate::StateInitError::ExplorerSchema {
                path: self.path().to_owned(),
                reason: format!(
                    "found schema version {}, but this build requires {}",
                    version.0,
                    ExplorerSchemaVersion::CURRENT.0
                ),
            }),
            None if self.tip().is_some() => Err(crate::StateInitError::ExplorerSchema {
                path: self.path().to_owned(),
                reason: "canonical state already contains blocks but has no explorer indexes; use a fresh state database and resync"
                    .to_string(),
            }),
            None if read_only => Err(crate::StateInitError::ExplorerSchema {
                path: self.path().to_owned(),
                reason: "the read-only primary database has no explorer schema marker".to_string(),
            }),
            None => {
                let mut batch = DiskWriteBatch::new();
                let _ = self
                    .explorer_schema_cf()
                    .with_batch_for_writing(&mut batch)
                    .zs_insert(&(), &ExplorerSchemaVersion::CURRENT);
                self.write_batch(batch)
                    .map_err(|error| crate::StateInitError::ExplorerSchema {
                        path: self.path().to_owned(),
                        reason: format!("could not initialize the explorer schema: {error}"),
                    })
            }
        }
    }

    pub(super) fn explorer_transaction_meta_cf(&self) -> ExplorerTransactionMetaCf<'_> {
        ExplorerTransactionMetaCf::new(self.disk_db(), EXPLORER_TRANSACTION_META_BY_LOC)
            .expect("explorer transaction metadata column family is registered")
    }

    pub(super) fn explorer_transaction_kind_cf(&self) -> ExplorerTransactionKindCf<'_> {
        ExplorerTransactionKindCf::new(self.disk_db(), EXPLORER_TRANSACTION_BY_KIND_LOC)
            .expect("explorer transaction kind column family is registered")
    }

    pub(super) fn explorer_shielded_class_cf(&self) -> ExplorerShieldedClassCf<'_> {
        ExplorerShieldedClassCf::new(self.disk_db(), EXPLORER_SHIELDED_TRANSACTION_BY_CLASS_LOC)
            .expect("explorer shielded classification column family is registered")
    }

    pub(super) fn explorer_address_meta_cf(&self) -> ExplorerAddressMetaCf<'_> {
        ExplorerAddressMetaCf::new(self.disk_db(), EXPLORER_ADDRESS_META)
            .expect("explorer address metadata column family is registered")
    }

    pub(super) fn explorer_block_stats_cf(&self) -> ExplorerBlockStatsCf<'_> {
        ExplorerBlockStatsCf::new(self.disk_db(), EXPLORER_BLOCK_STATS)
            .expect("explorer block analytics column family is registered")
    }

    pub(super) fn explorer_chain_stats_cf(&self) -> ExplorerChainStatsCf<'_> {
        ExplorerChainStatsCf::new(self.disk_db(), EXPLORER_CHAIN_STATS)
            .expect("explorer chain analytics column family is registered")
    }

    pub(super) fn explorer_daily_stats_cf(&self) -> ExplorerDailyStatsCf<'_> {
        ExplorerDailyStatsCf::new(self.disk_db(), EXPLORER_DAILY_STATS)
            .expect("explorer daily analytics column family is registered")
    }

    pub(super) fn explorer_balance_order_cf(&self) -> ExplorerBalanceOrderCf<'_> {
        ExplorerBalanceOrderCf::new(self.disk_db(), EXPLORER_BALANCE_ORDER)
            .expect("explorer balance order column family is registered")
    }
}

#[cfg(test)]
mod tests {
    use zakura_chain::parameters::Network;

    use crate::{
        constants::{state_database_format_version_in_code, STATE_DATABASE_KIND},
        service::finalized_state::STATE_COLUMN_FAMILIES_IN_CODE,
        Config,
    };

    use super::*;

    #[test]
    fn explorer_schema_version_is_initialized_and_validated() {
        let db = ZakuraDb::new(
            &Config::ephemeral(),
            STATE_DATABASE_KIND,
            &state_database_format_version_in_code(),
            &Network::Mainnet,
            true,
            STATE_COLUMN_FAMILIES_IN_CODE
                .iter()
                .map(ToString::to_string),
            false,
        )
        .expect("opening an ephemeral database should succeed");

        assert_eq!(
            db.explorer_schema_cf().zs_get(&()),
            Some(ExplorerSchemaVersion::CURRENT)
        );

        let mut batch = DiskWriteBatch::new();
        let _ = db
            .explorer_schema_cf()
            .with_batch_for_writing(&mut batch)
            .zs_insert(
                &(),
                &ExplorerSchemaVersion(ExplorerSchemaVersion::CURRENT.0 + 1),
            );
        db.write_batch(batch)
            .expect("writing an unsupported test schema should succeed");

        let error = db
            .ensure_explorer_schema(false)
            .expect_err("an unsupported explorer schema must be rejected");
        assert!(matches!(
            error,
            crate::StateInitError::ExplorerSchema { .. }
        ));
    }
}
