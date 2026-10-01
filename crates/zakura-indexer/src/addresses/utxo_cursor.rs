//! Opaque address UTXO cursor bound to an address and canonical output block.

use std::str::FromStr;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use zakura_chain::{
    block::{Hash, Height},
    transparent::Address,
};
use zakura_state::OutputLocation;

use crate::Error;

const POSITION_AND_HASH_BYTES: usize = 42;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct AddressUtxoCursor {
    pub(super) address: Address,
    pub(super) position: OutputLocation,
    pub(super) block_hash: Hash,
}

impl AddressUtxoCursor {
    pub(super) fn new(address: Address, position: OutputLocation, block_hash: Hash) -> Self {
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
        bytes.extend_from_slice(&self.position.height().0.to_be_bytes());
        bytes.extend_from_slice(&self.position.transaction_index().index().to_be_bytes());
        bytes.extend_from_slice(&self.position.output_index().index().to_be_bytes());
        bytes.extend_from_slice(&self.block_hash.0);
        URL_SAFE_NO_PAD.encode(bytes)
    }

    pub(super) fn decode(encoded: &str) -> Result<Self, Error> {
        let bytes = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| Error::InvalidCursor("cursor is not valid URL-safe base64".to_string()))?;
        let Some(address_length) = bytes.first().copied().map(usize::from) else {
            return Err(Error::InvalidCursor(
                "address UTXO cursor is empty".to_string(),
            ));
        };
        let expected_length = 1_usize
            .checked_add(address_length)
            .and_then(|length| length.checked_add(POSITION_AND_HASH_BYTES))
            .ok_or_else(|| {
                Error::InvalidCursor("address UTXO cursor length overflow".to_string())
            })?;
        if bytes.len() != expected_length {
            return Err(Error::InvalidCursor(
                "address UTXO cursor has an invalid length".to_string(),
            ));
        }

        let address_end = 1 + address_length;
        let address = std::str::from_utf8(&bytes[1..address_end])
            .map_err(|_| Error::InvalidCursor("cursor address is not UTF-8".to_string()))?;
        let address = Address::from_str(address)
            .map_err(|_| Error::InvalidCursor("cursor address is invalid".to_string()))?;
        let output_height_end = address_end + 4;
        let transaction_index_end = output_height_end + 2;
        let output_index_end = transaction_index_end + 4;

        let output_height = Height(u32::from_be_bytes(
            bytes[address_end..output_height_end]
                .try_into()
                .map_err(|_| Error::InvalidCursor("cursor output height is invalid".to_string()))?,
        ));
        let transaction_index = u16::from_be_bytes(
            bytes[output_height_end..transaction_index_end]
                .try_into()
                .map_err(|_| {
                    Error::InvalidCursor("cursor transaction index is invalid".to_string())
                })?,
        );
        let output_index = u32::from_be_bytes(
            bytes[transaction_index_end..output_index_end]
                .try_into()
                .map_err(|_| Error::InvalidCursor("cursor output index is invalid".to_string()))?,
        );
        let block_hash = Hash(
            bytes[output_index_end..]
                .try_into()
                .map_err(|_| Error::InvalidCursor("cursor block hash is invalid".to_string()))?,
        );
        let output_index = usize::try_from(output_index)
            .map_err(|_| Error::InvalidCursor("cursor output index is too large".to_string()))?;

        Ok(Self {
            address,
            position: OutputLocation::from_usize(
                output_height,
                usize::from(transaction_index),
                output_index,
            ),
            block_hash,
        })
    }
}

#[cfg(test)]
mod tests {
    use zakura_chain::parameters::NetworkKind;

    use super::*;

    #[test]
    fn cursor_round_trips_address_output_position_and_block_hash() {
        let cursor = AddressUtxoCursor::new(
            Address::from_pub_key_hash(NetworkKind::Mainnet, [7; 20]),
            OutputLocation::from_usize(Height(42), 3, 2),
            Hash([9; 32]),
        );

        assert_eq!(AddressUtxoCursor::decode(&cursor.encode()).unwrap(), cursor);
    }

    #[test]
    fn cursor_rejects_malformed_input() {
        assert!(AddressUtxoCursor::decode("not-a-cursor").is_err());
    }
}
