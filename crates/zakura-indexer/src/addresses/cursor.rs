//! Opaque address-history cursor bound to an address and canonical block.

use std::str::FromStr;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use zakura_chain::{
    block::{Hash, Height},
    transparent::Address,
};

use crate::{models::TransactionPosition, Error};

const POSITION_AND_HASH_BYTES: usize = 40;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct AddressTransactionCursor {
    pub(super) address: Address,
    pub(super) position: TransactionPosition,
    pub(super) block_hash: Hash,
}

impl AddressTransactionCursor {
    pub(super) fn new(address: Address, position: TransactionPosition, block_hash: Hash) -> Self {
        Self {
            address,
            position,
            block_hash,
        }
    }

    pub(super) fn encode(self) -> String {
        let address = self.address.to_string();
        let address_length = u8::try_from(address.len())
            .expect("transparent address encodings are shorter than 256 bytes");
        let mut bytes = Vec::with_capacity(1 + address.len() + POSITION_AND_HASH_BYTES);
        bytes.push(address_length);
        bytes.extend_from_slice(address.as_bytes());
        bytes.extend_from_slice(&self.position.height.0.to_be_bytes());
        bytes.extend_from_slice(&self.position.transaction_index.to_be_bytes());
        bytes.extend_from_slice(&self.block_hash.0);
        URL_SAFE_NO_PAD.encode(bytes)
    }

    pub(super) fn decode(encoded: &str) -> Result<Self, Error> {
        let bytes = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| Error::InvalidCursor("cursor is not valid URL-safe base64".to_string()))?;
        let Some(address_length) = bytes.first().copied().map(usize::from) else {
            return Err(Error::InvalidCursor("address cursor is empty".to_string()));
        };
        let expected_length = 1_usize
            .checked_add(address_length)
            .and_then(|length| length.checked_add(POSITION_AND_HASH_BYTES))
            .ok_or_else(|| Error::InvalidCursor("address cursor length overflow".to_string()))?;
        if bytes.len() != expected_length {
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
        let block_hash =
            Hash(bytes[position_end..].try_into().map_err(|_| {
                Error::InvalidCursor("cursor block hash must be 32 bytes".to_string())
            })?);

        Ok(Self {
            address,
            position,
            block_hash,
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
        );

        assert_eq!(
            AddressTransactionCursor::decode(&cursor.encode()).unwrap(),
            cursor
        );
    }
}
