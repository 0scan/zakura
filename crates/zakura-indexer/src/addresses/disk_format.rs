//! Address-domain keys and persisted value encodings.

use std::str::FromStr;

use zakura_chain::transparent::Address;

use crate::{
    database::{transaction_position_bytes, TRANSACTION_POSITION_BYTES},
    models::{AddressRecord, TransactionAddressEffects, TransactionPosition},
    Error,
};

const ADDRESS_ORDER_SEPARATOR: u8 = 0;
const BALANCE_BYTES: usize = std::mem::size_of::<u64>();

pub(super) fn address_record_key(address: Address) -> Vec<u8> {
    address.to_string().into_bytes()
}

/// Sorts funded addresses by descending balance, then ascending address.
pub(super) fn address_balance_order_key(address: Address, balance_zat: u64) -> Vec<u8> {
    let mut key = Vec::with_capacity(BALANCE_BYTES + address.to_string().len());
    key.extend_from_slice(&(u64::MAX - balance_zat).to_be_bytes());
    key.extend_from_slice(address.to_string().as_bytes());
    key
}

pub(super) fn decode_address_balance_order_key(key: &[u8]) -> Result<(Address, u64), Error> {
    if key.len() <= BALANCE_BYTES {
        return Err(Error::CorruptData(
            "address balance key is truncated".to_string(),
        ));
    }
    let balance_bytes = key[..BALANCE_BYTES].try_into().map_err(|_| {
        Error::CorruptData("address balance key must start with 8 balance bytes".to_string())
    })?;
    let address = std::str::from_utf8(&key[BALANCE_BYTES..])
        .map_err(|_| Error::CorruptData("address balance key is not UTF-8".to_string()))?;
    let address = Address::from_str(address).map_err(|_| {
        Error::CorruptData("address balance key has an invalid address".to_string())
    })?;
    Ok((address, u64::MAX - u64::from_be_bytes(balance_bytes)))
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

#[cfg(test)]
mod tests {
    use zakura_chain::{parameters::NetworkKind, transparent::Address};

    use super::{address_balance_order_key, decode_address_balance_order_key};

    #[test]
    fn address_balance_keys_sort_largest_balance_first() {
        let first = Address::from_pub_key_hash(NetworkKind::Mainnet, [1; 20]);
        let second = Address::from_pub_key_hash(NetworkKind::Mainnet, [2; 20]);
        let largest = address_balance_order_key(first, 20);
        let smaller = address_balance_order_key(second, 10);

        assert!(largest < smaller);
        assert_eq!(
            decode_address_balance_order_key(&largest).unwrap(),
            (first, 20)
        );
    }
}
