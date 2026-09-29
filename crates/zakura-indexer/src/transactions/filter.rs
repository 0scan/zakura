//! Typed transaction-list selectors shared by storage and queries.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Top-level transaction kind selector.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TransactionKindFilter {
    /// Include every transaction kind.
    #[default]
    All,
    /// Include transactions that touch at least one shielded pool.
    Shielded,
    /// Include non-coinbase transactions with transparent components only.
    Transparent,
    /// Include coinbase transactions.
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
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ShieldedFlowFilter {
    /// Include every observable shielded flow.
    #[default]
    All,
    /// Include public value entering a shielded pool.
    Shield,
    /// Include shielded value becoming public.
    Deshield,
    /// Include transactions with shielded components and no transparent inputs or outputs.
    FullyShielded,
    /// Include transactions whose public boundary flow cannot be represented by one direction.
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
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ShieldedPoolFilter {
    /// Include every shielded pool.
    #[default]
    All,
    /// Include Sprout transactions.
    Sprout,
    /// Include Sapling transactions.
    Sapling,
    /// Include Orchard transactions.
    Orchard,
    /// Include Ironwood transactions.
    Ironwood,
    /// Include transactions that touch multiple shielded pools.
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
pub enum AmountFilter {
    /// Do not apply a public-amount threshold.
    #[default]
    Any,
    /// Include public flows of at least 10 ZEC.
    AtLeastTenZec,
    /// Include public flows of at least 100 ZEC.
    AtLeastOneHundredZec,
    /// Include public flows of at least 1,000 ZEC.
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

    /// Converts an RPC-facing ZEC threshold into an indexed amount bucket.
    pub fn from_minimum_zec(minimum_zec: u32) -> Result<Self, String> {
        match minimum_zec {
            0 => Ok(Self::Any),
            10 => Ok(Self::AtLeastTenZec),
            100 => Ok(Self::AtLeastOneHundredZec),
            1_000 => Ok(Self::AtLeastOneThousandZec),
            _ => Err("min_zec must be 0, 10, 100, or 1000".to_string()),
        }
    }
}

/// Validated selector set for a newest-first transaction query.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TransactionQuery {
    pub(crate) kind: TransactionKindFilter,
    pub(crate) flow: ShieldedFlowFilter,
    pub(crate) pool: ShieldedPoolFilter,
    pub(crate) amount: AmountFilter,
}

impl TransactionQuery {
    /// Creates and validates a transaction filter set.
    pub fn new(
        kind: TransactionKindFilter,
        flow: ShieldedFlowFilter,
        pool: ShieldedPoolFilter,
        amount: AmountFilter,
    ) -> Result<Self, String> {
        Self {
            kind,
            flow,
            pool,
            amount,
        }
        .validate()
    }

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
