//! Pure transaction-kind, pool, flow, and public-amount derivation.

use zakura_chain::{block::Height, transaction::Transaction, transparent::Output};

use crate::{
    models::{TransactionPosition, TransactionRecord},
    types::{ShieldedFlow, ShieldedPool, TransactionClassification, TransactionKind},
    Error,
};

/// Classifies a verified non-coinbase transaction using its resolved transparent inputs.
pub fn classify_unmined_transaction(
    transaction: &Transaction,
    fee_zat: u64,
    spent_outputs: &[Output],
) -> Result<TransactionClassification, Error> {
    if transaction.is_coinbase() {
        return Err(Error::Calculation(
            "coinbase transactions cannot enter the mempool".to_string(),
        ));
    }

    let transparent_input_count = transaction
        .inputs()
        .iter()
        .filter_map(|input| input.outpoint())
        .count();
    if transparent_input_count != spent_outputs.len() {
        return Err(Error::Calculation(format!(
            "mempool transaction has {transparent_input_count} transparent inputs but {} resolved outputs",
            spent_outputs.len()
        )));
    }

    let total_input = spent_outputs.iter().try_fold(0_i64, |total, output| {
        total
            .checked_add(output.value().zatoshis())
            .ok_or_else(|| Error::Calculation("transaction input total exceeds i64".to_string()))
    })?;
    let total_output = transaction
        .outputs()
        .iter()
        .try_fold(0_i64, |total, output| {
            total.checked_add(output.value().zatoshis()).ok_or_else(|| {
                Error::Calculation("transaction output total exceeds i64".to_string())
            })
        })?;
    let transparent_value_balance_zat = total_input.checked_sub(total_output).ok_or_else(|| {
        Error::Calculation("transparent transaction value balance exceeds i64".to_string())
    })?;
    let record = TransactionRecord {
        position: TransactionPosition {
            height: Height::MIN,
            transaction_index: 1,
        },
        serialized_size: 0,
        fee_zat,
        transparent_value_balance_zat,
        sapling_value_balance_zat: transaction
            .sapling_value_balance()
            .sapling_amount()
            .zatoshis(),
        orchard_value_balance_zat: transaction
            .orchard_value_balance()
            .orchard_amount()
            .zatoshis(),
        ironwood_value_balance_zat: transaction
            .ironwood_value_balance()
            .ironwood_amount()
            .zatoshis(),
        transparent_input_count: count_u32(transparent_input_count, "transparent input count")?,
        transparent_output_count: count_u32(
            transaction.outputs().len(),
            "transparent output count",
        )?,
        joinsplit_count: count_u32(transaction.joinsplit_count(), "Sprout JoinSplit count")?,
        sapling_spend_count: count_u32(
            transaction.sapling_spends_per_anchor().count(),
            "Sapling spend count",
        )?,
        sapling_output_count: count_u32(
            transaction.sapling_outputs().count(),
            "Sapling output count",
        )?,
        orchard_action_count: count_u32(
            transaction.orchard_actions().count(),
            "Orchard action count",
        )?,
        ironwood_action_count: count_u32(
            transaction.ironwood_actions().count(),
            "Ironwood action count",
        )?,
    };

    Ok(TransactionClassification {
        kind: transaction_kind(&record),
        pool: shielded_pool(&record),
        flow: shielded_flow(&record)?,
        amount_zat: public_flow_amount(&record)?,
        shielded_value_balance_zat: shielded_value_balance(&record)?,
    })
}

pub(crate) fn transaction_kind(record: &TransactionRecord) -> TransactionKind {
    if record.position.transaction_index == 0 {
        TransactionKind::Coinbase
    } else if shielded_pool(record).is_some() {
        TransactionKind::Shielded
    } else {
        TransactionKind::Transparent
    }
}

pub(crate) fn shielded_pool(record: &TransactionRecord) -> Option<ShieldedPool> {
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

pub(crate) fn shielded_flow(record: &TransactionRecord) -> Result<Option<ShieldedFlow>, Error> {
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

fn count_u32(value: usize, name: &str) -> Result<u32, Error> {
    u32::try_from(value).map_err(|_| Error::Calculation(format!("{name} exceeds u32")))
}

#[cfg(test)]
mod tests {
    use zakura_chain::block::Height;

    use super::*;
    use crate::models::TransactionPosition;

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
