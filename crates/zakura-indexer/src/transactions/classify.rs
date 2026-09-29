//! Pure transaction-kind, pool, flow, and public-amount derivation.

use crate::{
    types::{ShieldedFlow, ShieldedPool, TransactionKind},
    Error,
};

use super::record::TransactionRecord;

pub(super) fn transaction_kind(record: &TransactionRecord) -> TransactionKind {
    if record.position.transaction_index == 0 {
        TransactionKind::Coinbase
    } else if shielded_pool(record).is_some() {
        TransactionKind::Shielded
    } else {
        TransactionKind::Transparent
    }
}

pub(super) fn shielded_pool(record: &TransactionRecord) -> Option<ShieldedPool> {
    let pools = [
        (record.joinsplit_count > 0, ShieldedPool::Sprout),
        (
            record.sapling_spend_count > 0 || record.sapling_output_count > 0,
            ShieldedPool::Sapling,
        ),
        (record.orchard_action_count > 0, ShieldedPool::Orchard),
        (record.ironwood_action_count > 0, ShieldedPool::Ironwood),
    ];
    let mut active = pools
        .into_iter()
        .filter_map(|(active, pool)| active.then_some(pool));
    let first = active.next()?;
    if active.next().is_some() {
        Some(ShieldedPool::Mixed)
    } else {
        Some(first)
    }
}

pub(super) fn shielded_flow(record: &TransactionRecord) -> Result<Option<ShieldedFlow>, Error> {
    if transaction_kind(record) != TransactionKind::Shielded {
        return Ok(None);
    }
    if record.transparent_input_count == 0 && record.transparent_output_count == 0 {
        return Ok(Some(ShieldedFlow::FullyShielded));
    }

    Ok(Some(match shielded_value_balance(record)? {
        value if value < 0 => ShieldedFlow::Shield,
        value if value > 0 => ShieldedFlow::Deshield,
        _ => ShieldedFlow::Complex,
    }))
}

pub(super) fn public_flow_amount(record: &TransactionRecord) -> Result<Option<u64>, Error> {
    match shielded_flow(record)? {
        Some(ShieldedFlow::Shield | ShieldedFlow::Deshield) => {
            Ok(Some(shielded_value_balance(record)?.unsigned_abs()))
        }
        Some(ShieldedFlow::FullyShielded | ShieldedFlow::Complex) | None => Ok(None),
    }
}

pub(super) fn shielded_value_balance(record: &TransactionRecord) -> Result<i64, Error> {
    let balance = if transaction_kind(record) == TransactionKind::Coinbase {
        i128::from(record.sapling_value_balance_zat)
            + i128::from(record.orchard_value_balance_zat)
            + i128::from(record.ironwood_value_balance_zat)
    } else {
        i128::from(record.fee_zat) - i128::from(record.transparent_value_balance_zat)
    };
    i64::try_from(balance).map_err(|_| {
        Error::Calculation("shielded transaction value balance exceeds i64".to_string())
    })
}

pub(super) fn format_zec(zatoshis: u64) -> String {
    const ZATOSHIS_PER_ZEC: u64 = 100_000_000;

    let whole = zatoshis / ZATOSHIS_PER_ZEC;
    let remainder = zatoshis % ZATOSHIS_PER_ZEC;
    if remainder == 0 {
        return whole.to_string();
    }

    let fraction = format!("{remainder:08}");
    format!("{whole}.{}", fraction.trim_end_matches('0'))
}

#[cfg(test)]
mod tests {
    use zakura_chain::block::Height;

    use super::*;
    use crate::transactions::record::TransactionPosition;

    #[test]
    fn classifies_modern_shielded_flows_without_treating_fees_as_deshielding() {
        let mut record = test_record();
        record.ironwood_action_count = 1;
        record.transparent_input_count = 1;
        record.transparent_value_balance_zat = 3_546_730_000;
        record.fee_zat = 10_000;

        assert_eq!(transaction_kind(&record), TransactionKind::Shielded);
        assert_eq!(shielded_pool(&record), Some(ShieldedPool::Ironwood));
        assert_eq!(shielded_flow(&record).unwrap(), Some(ShieldedFlow::Shield));
        assert_eq!(public_flow_amount(&record).unwrap(), Some(3_546_720_000));

        record.transparent_input_count = 0;
        record.transparent_value_balance_zat = 0;
        assert_eq!(
            shielded_flow(&record).unwrap(),
            Some(ShieldedFlow::FullyShielded)
        );
        assert_eq!(public_flow_amount(&record).unwrap(), None);
    }

    #[test]
    fn identifies_mixed_pool_activity_from_component_counts() {
        let mut record = test_record();
        record.sapling_output_count = 1;
        record.orchard_action_count = 1;

        assert_eq!(shielded_pool(&record), Some(ShieldedPool::Mixed));
    }

    #[test]
    fn formats_zatoshis_without_floating_point_rounding() {
        assert_eq!(format_zec(3_546_720_000), "35.4672");
        assert_eq!(format_zec(100_000_000), "1");
        assert_eq!(format_zec(1), "0.00000001");
    }

    fn test_record() -> TransactionRecord {
        TransactionRecord {
            position: TransactionPosition {
                height: Height(1),
                transaction_index: 1,
            },
            serialized_size: 100,
            fee_zat: 0,
            transparent_value_balance_zat: 0,
            sapling_value_balance_zat: 0,
            orchard_value_balance_zat: 0,
            ironwood_value_balance_zat: 0,
            transparent_input_count: 0,
            transparent_output_count: 0,
            joinsplit_count: 0,
            sapling_spend_count: 0,
            sapling_output_count: 0,
            orchard_action_count: 0,
            ironwood_action_count: 0,
        }
    }
}
