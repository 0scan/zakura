//! Opaque address-history cursor bound to an address and canonical block.

use std::str::FromStr;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use zakura_chain::{
    block::{Hash, Height},
    transparent::Address,
};

use crate::{height_range::TransactionHeightRange, models::TransactionPosition, Error};

const POSITION_AND_HASH_BYTES: usize = 40;
const POSITION_HASH_AND_RANGE_BYTES: usize = 48;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct AddressTransactionCursor {
    pub(super) address: Address,
    pub(super) position: TransactionPosition,
    pub(super) block_hash: Hash,
    height_range: TransactionHeightRange,
}

impl AddressTransactionCursor {
    pub(super) fn new(
        address: Address,
        position: TransactionPosition,
        block_hash: Hash,
        height_range: TransactionHeightRange,
    ) -> Self {
        Self {
            address,
            position,
            block_hash,
            height_range,
        }
    }

    pub(super) fn matches(self, address: Address, height_range: TransactionHeightRange) -> bool {
        self.address == address && self.height_range == height_range
    }

    pub(super) fn encode(self) -> String {
        let address = self.address.to_string();
        let address_length = u8::try_from(address.len())
            .expect("transparent address encodings are shorter than 256 bytes");
        let mut bytes = Vec::with_capacity(1 + address.len() + POSITION_HASH_AND_RANGE_BYTES);
        bytes.push(address_length);
        bytes.extend_from_slice(address.as_bytes());
        bytes.extend_from_slice(&self.position.height.0.to_be_bytes());
        bytes.extend_from_slice(&self.position.transaction_index.to_be_bytes());
        bytes.extend_from_slice(&self.block_hash.0);
        bytes.extend_from_slice(&self.height_range.from.0.to_be_bytes());
        bytes.extend_from_slice(&self.height_range.to.0.to_be_bytes());
        URL_SAFE_NO_PAD.encode(bytes)
    }

    pub(super) fn decode(encoded: &str) -> Result<Self, Error> {
        let bytes = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| Error::InvalidCursor("cursor is not valid URL-safe base64".to_string()))?;
        let Some(address_length) = bytes.first().copied().map(usize::from) else {
            return Err(Error::InvalidCursor("address cursor is empty".to_string()));
        };
        let legacy_length = 1_usize
            .checked_add(address_length)
            .and_then(|length| length.checked_add(POSITION_AND_HASH_BYTES))
            .ok_or_else(|| Error::InvalidCursor("address cursor length overflow".to_string()))?;
        let expected_length = legacy_length
            .checked_add(POSITION_HASH_AND_RANGE_BYTES - POSITION_AND_HASH_BYTES)
            .ok_or_else(|| Error::InvalidCursor("address cursor length overflow".to_string()))?;
        if bytes.len() != legacy_length && bytes.len() != expected_length {
            return Err(Error::InvalidCursor(
                "address cursor has an invalid length".to_string(),
            ));
        }

        let address_end = 1 + address_length;
        let address = std::str::from_utf8(&bytes[1..address_end])
            .map_err(|_| Error::InvalidCursor("cursor address is not UTF-8".to_string()))?;
        let address = Address::from_str(address)
            .map_err(|_| Error::InvalidCursor("cursor address is invalid".to_string()))?;
        let position_end = address_end + 8;
        let block_hash_end = position_end + 32;
        let from_height_end = block_hash_end + 4;
        let position = TransactionPosition {
            height: Height(u32::from_be_bytes(
                bytes[address_end..address_end + 4]
                    .try_into()
                    .map_err(|_| Error::InvalidCursor("cursor height is invalid".to_string()))?,
            )),
            transaction_index: u32::from_be_bytes(
                bytes[address_end + 4..position_end]
                    .try_into()
                    .map_err(|_| {
                        Error::InvalidCursor("cursor transaction index is invalid".to_string())
                    })?,
            ),
        };
        let block_hash = Hash(
            bytes[position_end..block_hash_end]
                .try_into()
                .map_err(|_| {
                    Error::InvalidCursor("cursor block hash must be 32 bytes".to_string())
                })?,
        );
        let height_range = if bytes.len() == legacy_length {
            TransactionHeightRange {
                from: Height::MIN,
                to: Height::MAX,
            }
        } else {
            TransactionHeightRange {
                from: Height(u32::from_be_bytes(
                    bytes[block_hash_end..from_height_end]
                        .try_into()
                        .map_err(|_| {
                            Error::InvalidCursor("cursor from height is invalid".to_string())
                        })?,
                )),
                to: Height(u32::from_be_bytes(
                    bytes[from_height_end..].try_into().map_err(|_| {
                        Error::InvalidCursor("cursor to height is invalid".to_string())
                    })?,
                )),
            }
        };

        Ok(Self {
            address,
            position,
            block_hash,
            height_range,
        })
    }
}

#[cfg(test)]
mod tests {
    use zakura_chain::parameters::NetworkKind;

    use super::*;

    #[test]
    fn cursor_round_trips_address_position_and_hash() {
        let cursor = AddressTransactionCursor::new(
            Address::from_pub_key_hash(NetworkKind::Mainnet, [7; 20]),
            TransactionPosition {
                height: Height(42),
                transaction_index: 3,
            },
            Hash([9; 32]),
            TransactionHeightRange::new(10..=100).unwrap(),
        );

        let decoded = AddressTransactionCursor::decode(&cursor.encode()).unwrap();
        assert_eq!(decoded, cursor);
        assert!(decoded.matches(
            Address::from_pub_key_hash(NetworkKind::Mainnet, [7; 20]),
            TransactionHeightRange::new(10..=100).unwrap()
        ));
        assert!(!decoded.matches(
            Address::from_pub_key_hash(NetworkKind::Mainnet, [7; 20]),
            TransactionHeightRange::new(11..=100).unwrap()
        ));
    }

    #[test]
    fn legacy_cursor_uses_the_complete_height_range() {
        let cursor = AddressTransactionCursor::new(
            Address::from_pub_key_hash(NetworkKind::Mainnet, [7; 20]),
            TransactionPosition {
                height: Height(42),
                transaction_index: 3,
            },
            Hash([9; 32]),
            TransactionHeightRange::new(Height::MIN.0..=Height::MAX.0).unwrap(),
        );
        let mut bytes = URL_SAFE_NO_PAD.decode(cursor.encode()).unwrap();
        bytes.truncate(1 + cursor.address.to_string().len() + POSITION_AND_HASH_BYTES);

        let decoded = AddressTransactionCursor::decode(&URL_SAFE_NO_PAD.encode(bytes)).unwrap();
        assert_eq!(decoded, cursor);
    }
}
