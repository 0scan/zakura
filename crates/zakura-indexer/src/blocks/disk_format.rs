//! Block-domain key and value encodings used by the indexer database.

use std::io::Cursor;

use zakura_chain::{
    block::{Hash, Height},
    serialization::{ZcashDeserialize, ZcashSerialize},
    transparent::{OutPoint, Output, Utxo},
};

use crate::Error;

pub(super) fn block_height_key(height: Height) -> [u8; 4] {
    height.0.to_be_bytes()
}

pub(super) fn transparent_outpoint_key(outpoint: OutPoint) -> [u8; 36] {
    let mut key = [0; 36];
    key[..32].copy_from_slice(&outpoint.hash.0);
    key[32..].copy_from_slice(&outpoint.index.to_be_bytes());
    key
}

pub(super) fn indexed_block_tip_value(height: Height, hash: Hash) -> [u8; 36] {
    let mut value = [0; 36];
    value[..4].copy_from_slice(&height.0.to_be_bytes());
    value[4..].copy_from_slice(&hash.0);
    value
}

pub(super) fn decode_indexed_block_tip(bytes: &[u8]) -> Result<(Height, Hash), Error> {
    if bytes.len() != 36 {
        return Err(Error::CorruptData(
            "stored indexed block tip must contain a height and block hash".to_string(),
        ));
    }

    let height_bytes = bytes[..4].try_into().map_err(|_| {
        Error::CorruptData("stored indexed block tip height must be 4 bytes".to_string())
    })?;
    let hash_bytes = bytes[4..].try_into().map_err(|_| {
        Error::CorruptData("stored indexed block tip hash must be 32 bytes".to_string())
    })?;

    Ok((Height(u32::from_be_bytes(height_bytes)), Hash(hash_bytes)))
}

pub(super) fn encode_transparent_output(utxo: &Utxo) -> Result<Vec<u8>, Error> {
    let mut value = Vec::new();
    value.extend_from_slice(&utxo.height.0.to_be_bytes());
    value.push(u8::from(utxo.from_coinbase));
    utxo.output
        .zcash_serialize(&mut value)
        .map_err(|error| Error::TransparentOutput(error.to_string()))?;

    Ok(value)
}

pub(super) fn decode_transparent_output(bytes: &[u8]) -> Result<Utxo, Error> {
    if bytes.len() < 5 {
        return Err(Error::TransparentOutput(
            "stored transparent output metadata is truncated".to_string(),
        ));
    }

    let height_bytes = bytes[..4].try_into().map_err(|_| {
        Error::TransparentOutput("stored output height must be 4 bytes".to_string())
    })?;
    let height = Height(u32::from_be_bytes(height_bytes));
    let from_coinbase = match bytes[4] {
        0 => false,
        1 => true,
        _ => {
            return Err(Error::TransparentOutput(
                "stored coinbase marker must be zero or one".to_string(),
            ))
        }
    };
    let output = Output::zcash_deserialize(Cursor::new(&bytes[5..]))
        .map_err(|error| Error::TransparentOutput(error.to_string()))?;

    Ok(Utxo::new(output, height, from_coinbase))
}
