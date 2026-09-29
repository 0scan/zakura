//! Address-domain keys and persisted value encodings.

use zakura_chain::transparent::Address;

use crate::{
    database::{transaction_position_bytes, TRANSACTION_POSITION_BYTES},
    models::{AddressRecord, TransactionAddressEffects, TransactionPosition},
    Error,
};

const ADDRESS_ORDER_SEPARATOR: u8 = 0;

pub(super) fn address_record_key(address: Address) -> Vec<u8> {
    address.to_string().into_bytes()
}

pub(super) fn address_order_prefix(address: Address) -> Vec<u8> {
    let mut prefix = address_record_key(address);
    prefix.push(ADDRESS_ORDER_SEPARATOR);
    prefix
}

pub(super) fn address_order_key(address: Address, position: TransactionPosition) -> Vec<u8> {
    let mut key = address_order_prefix(address);
    key.extend_from_slice(&transaction_position_bytes(position));
    key
}

pub(super) fn newest_address_order_key(address: Address) -> Vec<u8> {
    let mut key = address_order_prefix(address);
    key.extend_from_slice(&[u8::MAX; TRANSACTION_POSITION_BYTES]);
    key
}

pub(super) fn transaction_address_effects_key(
    position: TransactionPosition,
) -> [u8; TRANSACTION_POSITION_BYTES] {
    transaction_position_bytes(position)
}

pub(super) fn encode_address_record(record: AddressRecord) -> Result<Vec<u8>, Error> {
    Ok(serde_json::to_vec(&record)?)
}

pub(super) fn decode_address_record(bytes: &[u8]) -> Result<AddressRecord, Error> {
    Ok(serde_json::from_slice(bytes)?)
}

pub(super) fn encode_transaction_address_effects(
    effects: &TransactionAddressEffects,
) -> Result<Vec<u8>, Error> {
    Ok(serde_json::to_vec(effects)?)
}

pub(super) fn decode_transaction_address_effects(
    bytes: &[u8],
) -> Result<TransactionAddressEffects, Error> {
    Ok(serde_json::from_slice(bytes)?)
}
