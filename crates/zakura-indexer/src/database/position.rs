//! Shared ordered encodings for canonical transaction positions.

use zakura_chain::block::Height;

use crate::{models::TransactionPosition, Error};

pub(crate) const TRANSACTION_POSITION_BYTES: usize = 8;

pub(crate) fn transaction_position_bytes(
    position: TransactionPosition,
) -> [u8; TRANSACTION_POSITION_BYTES] {
    let mut bytes = [0; TRANSACTION_POSITION_BYTES];
    bytes[..4].copy_from_slice(&position.height.0.to_be_bytes());
    bytes[4..].copy_from_slice(&position.transaction_index.to_be_bytes());
    bytes
}

pub(crate) fn decode_transaction_position(bytes: &[u8]) -> Result<TransactionPosition, Error> {
    if bytes.len() != TRANSACTION_POSITION_BYTES {
        return Err(Error::CorruptData(format!(
            "stored transaction position must be {TRANSACTION_POSITION_BYTES} bytes"
        )));
    }

    Ok(TransactionPosition {
        height: Height(u32::from_be_bytes(bytes[..4].try_into().map_err(|_| {
            Error::CorruptData("stored transaction height must be 4 bytes".to_string())
        })?)),
        transaction_index: u32::from_be_bytes(bytes[4..].try_into().map_err(|_| {
            Error::CorruptData("stored transaction index must be 4 bytes".to_string())
        })?),
    })
}

pub(crate) fn decode_trailing_transaction_position(
    bytes: &[u8],
) -> Result<TransactionPosition, Error> {
    if bytes.len() < TRANSACTION_POSITION_BYTES {
        return Err(Error::CorruptData(
            "stored transaction order key is truncated".to_string(),
        ));
    }

    decode_transaction_position(&bytes[bytes.len() - TRANSACTION_POSITION_BYTES..])
}
