//! Validated inclusive block-height ranges for transaction queries.

use std::ops::RangeInclusive;

use zakura_chain::block::Height;

use crate::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TransactionHeightRange {
    pub(crate) from: Height,
    pub(crate) to: Height,
}

impl TransactionHeightRange {
    pub(crate) fn new(range: RangeInclusive<u32>) -> Result<Self, Error> {
        if *range.end() > Height::MAX.0 {
            return Err(Error::InvalidQuery(format!(
                "block height must not exceed {}",
                Height::MAX.0
            )));
        }
        let range = Self {
            from: Height(*range.start()),
            to: Height(*range.end()),
        };
        if range.from > range.to {
            return Err(Error::InvalidQuery(
                "from_height must be less than or equal to to_height".to_string(),
            ));
        }
        Ok(range)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_the_complete_height_range() {
        assert_eq!(
            TransactionHeightRange::new(Height::MIN.0..=Height::MAX.0).unwrap(),
            TransactionHeightRange {
                from: Height::MIN,
                to: Height::MAX,
            }
        );
    }

    #[test]
    fn rejects_an_inverted_height_range() {
        assert!(TransactionHeightRange::new(2..=1).is_err());
    }

    #[test]
    fn rejects_a_height_above_the_consensus_maximum() {
        assert!(TransactionHeightRange::new(0..=Height::MAX.0 + 1).is_err());
    }
}
