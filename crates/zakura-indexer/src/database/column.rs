//! Column families and metadata keys owned by the indexer database.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DatabaseColumn {
    BlockRecords,
    CanonicalBlockHashes,
    TransparentOutputs,
    TransactionRecords,
    CanonicalTransactionPositions,
    TransactionKindOrder,
    ShieldedTransactionOrder,
    AddressRecords,
    AddressBalanceOrder,
    AddressTransactionOrder,
    TransactionAddressEffects,
    DailyStats,
    Metadata,
}

impl DatabaseColumn {
    pub(super) const ALL: [Self; 13] = [
        Self::BlockRecords,
        Self::CanonicalBlockHashes,
        Self::TransparentOutputs,
        Self::TransactionRecords,
        Self::CanonicalTransactionPositions,
        Self::TransactionKindOrder,
        Self::ShieldedTransactionOrder,
        Self::AddressRecords,
        Self::AddressBalanceOrder,
        Self::AddressTransactionOrder,
        Self::TransactionAddressEffects,
        Self::DailyStats,
        Self::Metadata,
    ];

    pub(super) const fn name(self) -> &'static str {
        match self {
            Self::BlockRecords => "block_records",
            Self::CanonicalBlockHashes => "canonical_block_hashes",
            Self::TransparentOutputs => "transparent_outputs",
            Self::TransactionRecords => "transaction_records",
            Self::CanonicalTransactionPositions => "canonical_transaction_positions",
            Self::TransactionKindOrder => "transaction_kind_order",
            Self::ShieldedTransactionOrder => "shielded_transaction_order",
            Self::AddressRecords => "address_records",
            Self::AddressBalanceOrder => "address_balance_order",
            Self::AddressTransactionOrder => "address_transaction_order",
            Self::TransactionAddressEffects => "transaction_address_effects",
            Self::DailyStats => "daily_stats",
            Self::Metadata => "metadata",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MetadataKey {
    FormatVersion,
    IndexedBlockTip,
    ChainStats,
}

impl MetadataKey {
    pub(crate) const fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::FormatVersion => b"format_version",
            Self::IndexedBlockTip => b"indexed_block_tip",
            Self::ChainStats => b"chain_stats",
        }
    }
}
