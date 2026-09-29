//! Typed transaction-list selectors shared by storage and queries.

/// Top-level transaction kind selector.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum TransactionKindFilter {
    #[default]
    All,
    Shielded,
    Transparent,
    Coinbase,
}

impl TransactionKindFilter {
    pub(super) const fn tag(self) -> u8 {
        match self {
            Self::All => 0,
            Self::Shielded => 1,
            Self::Transparent => 2,
            Self::Coinbase => 3,
        }
    }
}

/// Observable shielded-flow selector.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum ShieldedFlowFilter {
    #[default]
    All,
    Shield,
    Deshield,
    FullyShielded,
    Complex,
}

impl ShieldedFlowFilter {
    pub(super) const fn tag(self) -> u8 {
        match self {
            Self::All => 0,
            Self::Shield => 1,
            Self::Deshield => 2,
            Self::FullyShielded => 3,
            Self::Complex => 4,
        }
    }
}

/// Shielded-pool selector.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum ShieldedPoolFilter {
    #[default]
    All,
    Sprout,
    Sapling,
    Orchard,
    Ironwood,
    Mixed,
}

impl ShieldedPoolFilter {
    pub(super) const fn tag(self) -> u8 {
        match self {
            Self::All => 0,
            Self::Sprout => 1,
            Self::Sapling => 2,
            Self::Orchard => 3,
            Self::Ironwood => 4,
            Self::Mixed => 5,
        }
    }
}

/// Fixed public-amount thresholds supported by the recent transaction index.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum AmountFilter {
    #[default]
    Any,
    AtLeastTenZec,
    AtLeastOneHundredZec,
    AtLeastOneThousandZec,
}

impl AmountFilter {
    const ZATOSHIS_PER_ZEC: u64 = 100_000_000;

    pub(super) const fn tag(self) -> u8 {
        match self {
            Self::Any => 0,
            Self::AtLeastTenZec => 1,
            Self::AtLeastOneHundredZec => 2,
            Self::AtLeastOneThousandZec => 3,
        }
    }

    pub(super) const fn minimum_zat(self) -> Option<u64> {
        match self {
            Self::Any => None,
            Self::AtLeastTenZec => Some(10 * Self::ZATOSHIS_PER_ZEC),
            Self::AtLeastOneHundredZec => Some(100 * Self::ZATOSHIS_PER_ZEC),
            Self::AtLeastOneThousandZec => Some(1_000 * Self::ZATOSHIS_PER_ZEC),
        }
    }
}

/// Validated selector set for a newest-first transaction query.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct TransactionQuery {
    pub(crate) kind: TransactionKindFilter,
    pub(crate) flow: ShieldedFlowFilter,
    pub(crate) pool: ShieldedPoolFilter,
    pub(crate) amount: AmountFilter,
}

impl TransactionQuery {
    pub(crate) fn validate(self) -> Result<Self, String> {
        let has_shielded_filter = self.flow != ShieldedFlowFilter::All
            || self.pool != ShieldedPoolFilter::All
            || self.amount != AmountFilter::Any;
        if has_shielded_filter && self.kind != TransactionKindFilter::Shielded {
            return Err("flow_type, pool, and min_zec require type=shielded".to_string());
        }

        Ok(self)
    }

    pub(super) fn uses_shielded_index(self) -> bool {
        self.kind == TransactionKindFilter::Shielded
            && (self.flow.tag() != 0 || self.pool.tag() != 0 || self.amount.tag() != 0)
    }

    pub(super) const fn cursor_tags(self) -> [u8; 4] {
        [
            self.kind.tag(),
            self.flow.tag(),
            self.pool.tag(),
            self.amount.tag(),
        ]
    }
}
