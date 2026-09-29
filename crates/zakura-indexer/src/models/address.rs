//! Persisted address summaries and per-transaction transparent value effects.

use serde::{Deserialize, Serialize};
use zakura_chain::transparent::Address;

use super::TransactionPosition;

/// Compact aggregate facts for one indexed transparent address.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub(crate) struct AddressRecord {
    pub(crate) total_received_zat: u64,
    pub(crate) total_sent_zat: u64,
    pub(crate) transaction_count: u64,
    pub(crate) first_position: TransactionPosition,
    pub(crate) last_position: TransactionPosition,
    pub(crate) first_funding_position: Option<TransactionPosition>,
}

/// One address's public value movement within a canonical transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub(crate) struct AddressEffect {
    pub(crate) address: Address,
    pub(crate) received_zat: u64,
    pub(crate) sent_zat: u64,
}

/// All recognizable transparent address effects produced by one transaction.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub(crate) struct TransactionAddressEffects {
    pub(crate) effects: Vec<AddressEffect>,
}
