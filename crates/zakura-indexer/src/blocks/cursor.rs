//! Opaque canonical-block pagination positions.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use zakura_chain::block::{Hash, Height};

use crate::Error;

const CURSOR_BYTE_LENGTH: usize = 36;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct BlockCursor {
    pub(super) height: Height,
    pub(super) hash: Hash,
}

impl BlockCursor {
    pub(super) fn new(height: Height, hash: Hash) -> Self {
        Self { height, hash }
    }

    pub(super) fn encode(self) -> String {
        let mut bytes = [0; CURSOR_BYTE_LENGTH];
        bytes[..4].copy_from_slice(&self.height.0.to_be_bytes());
        bytes[4..].copy_from_slice(&self.hash.0);
        URL_SAFE_NO_PAD.encode(bytes)
    }

    pub(super) fn decode(encoded: &str) -> Result<Self, Error> {
        let bytes = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| Error::InvalidCursor("cursor is not valid URL-safe base64".to_string()))?;
        if bytes.len() != CURSOR_BYTE_LENGTH {
            return Err(Error::InvalidCursor(format!(
                "decoded cursor must be {CURSOR_BYTE_LENGTH} bytes"
            )));
        }

        let height_bytes = bytes[..4]
            .try_into()
            .map_err(|_| Error::InvalidCursor("cursor height must be 4 bytes".to_string()))?;
        let hash_bytes = bytes[4..]
            .try_into()
            .map_err(|_| Error::InvalidCursor("cursor hash must be 32 bytes".to_string()))?;

        Ok(Self {
            height: Height(u32::from_be_bytes(height_bytes)),
            hash: Hash(hash_bytes),
        })
    }
}

#[cfg(test)]
mod tests {
    use zakura_chain::block::{Hash, Height};

    use super::BlockCursor;

    #[test]
    fn block_cursor_round_trips_its_canonical_position() {
        let cursor = BlockCursor::new(Height(42), Hash([7; 32]));

        assert_eq!(BlockCursor::decode(&cursor.encode()).unwrap(), cursor);
    }

    #[test]
    fn block_cursor_rejects_malformed_input() {
        assert!(BlockCursor::decode("not-a-valid-cursor").is_err());
    }
}
