//! Typed transaction-list selectors shared by storage and queries.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::types::{ShieldedFlow, ShieldedPool, TransactionClassification, TransactionKind};

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
    #[cfg(feature = "state-index")]
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
    #[cfg(feature = "state-index")]
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
    #[cfg(feature = "state-index")]
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
    /// Include public flows of at least 1,000,000,000 zatoshis.
    AtLeastOneBillionZat,
    /// Include public flows of at least 10,000,000,000 zatoshis.
    AtLeastTenBillionZat,
    /// Include public flows of at least 100,000,000,000 zatoshis.
    AtLeastOneHundredBillionZat,
}

impl AmountFilter {
    #[cfg(feature = "state-index")]
    pub(super) const fn tag(self) -> u8 {
        match self {
            Self::Any => 0,
            Self::AtLeastOneBillionZat => 1,
            Self::AtLeastTenBillionZat => 2,
            Self::AtLeastOneHundredBillionZat => 3,
        }
    }

    pub(super) const fn minimum_zat(self) -> Option<u64> {
        match self {
            Self::Any => None,
            Self::AtLeastOneBillionZat => Some(1_000_000_000),
            Self::AtLeastTenBillionZat => Some(10_000_000_000),
            Self::AtLeastOneHundredBillionZat => Some(100_000_000_000),
        }
    }

    /// Converts an RPC-facing zatoshi threshold into an indexed amount bucket.
    pub fn from_minimum_zat(minimum_zat: u64) -> Result<Self, String> {
        match minimum_zat {
            0 => Ok(Self::Any),
            1_000_000_000 => Ok(Self::AtLeastOneBillionZat),
            10_000_000_000 => Ok(Self::AtLeastTenBillionZat),
            100_000_000_000 => Ok(Self::AtLeastOneHundredBillionZat),
            _ => Err("min_zat must be 0, 1000000000, 10000000000, or 100000000000".to_string()),
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

    /// Returns whether a derived transaction classification satisfies this query.
    pub fn matches(self, classification: TransactionClassification) -> bool {
        let kind_matches = match self.kind {
            TransactionKindFilter::All => true,
            TransactionKindFilter::Shielded => classification.kind == TransactionKind::Shielded,
            TransactionKindFilter::Transparent => {
                classification.kind == TransactionKind::Transparent
            }
            TransactionKindFilter::Coinbase => classification.kind == TransactionKind::Coinbase,
        };
        let flow_matches = match self.flow {
            ShieldedFlowFilter::All => true,
            ShieldedFlowFilter::Shield => classification.flow == Some(ShieldedFlow::Shield),
            ShieldedFlowFilter::Deshield => classification.flow == Some(ShieldedFlow::Deshield),
            ShieldedFlowFilter::FullyShielded => {
                classification.flow == Some(ShieldedFlow::FullyShielded)
            }
            ShieldedFlowFilter::Complex => classification.flow == Some(ShieldedFlow::Complex),
        };
        let pool_matches = match self.pool {
            ShieldedPoolFilter::All => true,
            ShieldedPoolFilter::Sprout => classification.pool == Some(ShieldedPool::Sprout),
            ShieldedPoolFilter::Sapling => classification.pool == Some(ShieldedPool::Sapling),
            ShieldedPoolFilter::Orchard => classification.pool == Some(ShieldedPool::Orchard),
            ShieldedPoolFilter::Ironwood => classification.pool == Some(ShieldedPool::Ironwood),
            ShieldedPoolFilter::Mixed => classification.pool == Some(ShieldedPool::Mixed),
        };
        let amount_matches = self.amount.minimum_zat().is_none_or(|minimum| {
            classification
                .flow_amount_zat
                .is_some_and(|amount| amount >= minimum)
        });

        kind_matches && flow_matches && pool_matches && amount_matches
    }

    pub(crate) fn validate(self) -> Result<Self, String> {
        let has_shielded_filter = self.flow != ShieldedFlowFilter::All
            || self.pool != ShieldedPoolFilter::All
            || self.amount != AmountFilter::Any;
        if has_shielded_filter && self.kind != TransactionKindFilter::Shielded {
            return Err("flow_type, pool, and min_zat require type=shielded".to_string());
        }

        Ok(self)
    }

    #[cfg(feature = "state-index")]
    pub(super) const fn cursor_tags(self) -> [u8; 4] {
        [
            self.kind.tag(),
            self.flow.tag(),
            self.pool.tag(),
            self.amount.tag(),
        ]
    }
}
