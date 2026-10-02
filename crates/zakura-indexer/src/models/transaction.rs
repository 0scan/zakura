//! Compact transaction facts used while building explorer responses.

use zakura_chain::{block::Height, transaction::TransactionValueEndpoint};
#[cfg(feature = "state-index")]
use zakura_state::{ExplorerTransactionRecord, TransactionLocation};

/// A transaction's canonical position.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TransactionPosition {
    pub(crate) height: Height,
    pub(crate) transaction_index: u32,
}

/// Transaction facts used to derive explorer list and detail fields.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TransactionRecord {
    pub(crate) position: TransactionPosition,
    pub(crate) serialized_size: u32,
    pub(crate) fee_zat: u64,
    pub(crate) transparent_value_balance_zat: i64,
    pub(crate) sapling_value_balance_zat: i64,
    pub(crate) orchard_value_balance_zat: i64,
    pub(crate) ironwood_value_balance_zat: i64,
    pub(crate) transparent_input_count: u32,
    pub(crate) transparent_output_count: u32,
    pub(crate) joinsplit_count: u32,
    pub(crate) sapling_spend_count: u32,
    pub(crate) sapling_output_count: u32,
    pub(crate) orchard_action_count: u32,
    pub(crate) ironwood_action_count: u32,
    pub(crate) transparent_output_total_zat: i64,
    pub(crate) primary_from: Option<TransactionValueEndpoint>,
    pub(crate) primary_to: Option<TransactionValueEndpoint>,
}

#[cfg(feature = "state-index")]
impl TransactionRecord {
    pub(crate) fn from_state(
        location: TransactionLocation,
        record: ExplorerTransactionRecord,
    ) -> Self {
        Self {
            position: TransactionPosition {
                height: location.height,
                transaction_index: u32::from(location.index.index()),
            },
            serialized_size: record.serialized_size,
            fee_zat: record.fee_zat,
            transparent_value_balance_zat: record.transparent_value_balance_zat,
            sapling_value_balance_zat: record.sapling_value_balance_zat,
            orchard_value_balance_zat: record.orchard_value_balance_zat,
            ironwood_value_balance_zat: record.ironwood_value_balance_zat,
            transparent_input_count: record.transparent_input_count,
            transparent_output_count: record.transparent_output_count,
            joinsplit_count: record.joinsplit_count,
            sapling_spend_count: record.sapling_spend_count,
            sapling_output_count: record.sapling_output_count,
            orchard_action_count: record.orchard_action_count,
            ironwood_action_count: record.ironwood_action_count,
            transparent_output_total_zat: record.transparent_output_total_zat,
            primary_from: record.primary_from,
            primary_to: record.primary_to,
        }
    }
}
