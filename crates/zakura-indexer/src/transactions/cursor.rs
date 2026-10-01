//! Opaque transaction pagination positions bound to their filter set.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use zakura_chain::block::{Hash, Height};

use crate::height_range::TransactionHeightRange;
use crate::{models::TransactionPosition, Error};

use super::filter::TransactionQuery;

const LEGACY_CURSOR_BYTE_LENGTH: usize = 44;
const CURSOR_BYTE_LENGTH: usize = 52;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct TransactionCursor {
    pub(super) position: TransactionPosition,
    pub(super) block_hash: Hash,
    filter_tags: [u8; 4],
    height_range: TransactionHeightRange,
}

impl TransactionCursor {
    pub(super) fn new(
        position: TransactionPosition,
        block_hash: Hash,
        query: TransactionQuery,
        height_range: TransactionHeightRange,
    ) -> Self {
        Self {
            position,
            block_hash,
            filter_tags: query.cursor_tags(),
            height_range,
        }
    }

    pub(super) fn matches(
        self,
        query: TransactionQuery,
        height_range: TransactionHeightRange,
    ) -> bool {
        self.filter_tags == query.cursor_tags() && self.height_range == height_range
    }

    pub(super) fn encode(self) -> String {
        let mut bytes = [0; CURSOR_BYTE_LENGTH];
        bytes[..4].copy_from_slice(&self.position.height.0.to_be_bytes());
        bytes[4..8].copy_from_slice(&self.position.transaction_index.to_be_bytes());
        bytes[8..40].copy_from_slice(&self.block_hash.0);
        bytes[40..44].copy_from_slice(&self.filter_tags);
        bytes[44..48].copy_from_slice(&self.height_range.from.0.to_be_bytes());
        bytes[48..52].copy_from_slice(&self.height_range.to.0.to_be_bytes());
        URL_SAFE_NO_PAD.encode(bytes)
    }

    pub(super) fn decode(encoded: &str) -> Result<Self, Error> {
        let bytes = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| Error::InvalidCursor("cursor is not valid URL-safe base64".to_string()))?;
        if bytes.len() != LEGACY_CURSOR_BYTE_LENGTH && bytes.len() != CURSOR_BYTE_LENGTH {
            return Err(Error::InvalidCursor(format!(
                "decoded transaction cursor must be {LEGACY_CURSOR_BYTE_LENGTH} or {CURSOR_BYTE_LENGTH} bytes"
            )));
        }

        let height_range = if bytes.len() == LEGACY_CURSOR_BYTE_LENGTH {
            TransactionHeightRange {
                from: Height::MIN,
                to: Height::MAX,
            }
        } else {
            TransactionHeightRange {
                from: Height(u32::from_be_bytes(bytes[44..48].try_into().map_err(
                    |_| Error::InvalidCursor("cursor from height must be 4 bytes".to_string()),
                )?)),
                to: Height(u32::from_be_bytes(bytes[48..52].try_into().map_err(
                    |_| Error::InvalidCursor("cursor to height must be 4 bytes".to_string()),
                )?)),
            }
        };

        Ok(Self {
            position: TransactionPosition {
                height: Height(u32::from_be_bytes(bytes[..4].try_into().map_err(|_| {
                    Error::InvalidCursor("cursor height must be 4 bytes".to_string())
                })?)),
                transaction_index: u32::from_be_bytes(bytes[4..8].try_into().map_err(|_| {
                    Error::InvalidCursor("cursor transaction index must be 4 bytes".to_string())
                })?),
            },
            block_hash: Hash(bytes[8..40].try_into().map_err(|_| {
                Error::InvalidCursor("cursor block hash must be 32 bytes".to_string())
            })?),
            filter_tags: bytes[40..44].try_into().map_err(|_| {
                Error::InvalidCursor("cursor filter selector must be 4 bytes".to_string())
            })?,
            height_range,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_round_trips_position_hash_and_filters() {
        let query = TransactionQuery::default();
        let height_range = TransactionHeightRange::new(10..=100).unwrap();
        let cursor = TransactionCursor::new(
            TransactionPosition {
                height: Height(42),
                transaction_index: 7,
            },
            Hash([9; 32]),
            query,
            height_range,
        );

        let decoded = TransactionCursor::decode(&cursor.encode()).unwrap();
        assert_eq!(decoded, cursor);
        assert!(decoded.matches(query, height_range));
        assert!(!decoded.matches(query, TransactionHeightRange::new(11..=100).unwrap()));
    }

    #[test]
    fn legacy_cursor_uses_the_complete_height_range() {
        let query = TransactionQuery::default();
        let cursor = TransactionCursor::new(
            TransactionPosition {
                height: Height(42),
                transaction_index: 7,
            },
            Hash([9; 32]),
            query,
            TransactionHeightRange::new(Height::MIN.0..=Height::MAX.0).unwrap(),
        );
        let mut bytes = URL_SAFE_NO_PAD.decode(cursor.encode()).unwrap();
        bytes.truncate(LEGACY_CURSOR_BYTE_LENGTH);

        let decoded = TransactionCursor::decode(&URL_SAFE_NO_PAD.encode(bytes)).unwrap();
        assert_eq!(decoded, cursor);
    }
}
