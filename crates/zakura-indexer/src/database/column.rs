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
    Metadata,
}

impl DatabaseColumn {
    pub(super) const ALL: [Self; 8] = [
        Self::BlockRecords,
        Self::CanonicalBlockHashes,
        Self::TransparentOutputs,
        Self::TransactionRecords,
        Self::CanonicalTransactionPositions,
        Self::TransactionKindOrder,
        Self::ShieldedTransactionOrder,
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
            Self::Metadata => "metadata",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MetadataKey {
    FormatVersion,
    IndexedBlockTip,
}

impl MetadataKey {
    pub(crate) const fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::FormatVersion => b"format_version",
            Self::IndexedBlockTip => b"indexed_block_tip",
        }
    }
}
