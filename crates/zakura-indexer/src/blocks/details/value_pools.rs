//! Conversion of state value-pool accounting into REST response values.

use zakura_chain::{
    amount::{Amount, NegativeAllowed, NonNegative, COIN},
    value_balance::ValueBalance,
};

use crate::{types::ValuePoolBalance, Error};

pub(super) fn responses(
    current: ValueBalance<NonNegative>,
    previous: Option<ValueBalance<NonNegative>>,
) -> Result<(ValuePoolBalance, Vec<ValuePoolBalance>), Error> {
    let current_signed = current
        .constrain::<NegativeAllowed>()
        .map_err(|error| Error::Calculation(error.to_string()))?;
    let previous_signed = previous
        .unwrap_or_else(ValueBalance::zero)
        .constrain::<NegativeAllowed>()
        .map_err(|error| Error::Calculation(error.to_string()))?;
    let delta = (current_signed - previous_signed)
        .map_err(|error| Error::Calculation(error.to_string()))?;

    let pools = [
        pool(
            "transparent",
            current.transparent_amount(),
            delta.transparent_amount(),
        ),
        pool("sprout", current.sprout_amount(), delta.sprout_amount()),
        pool("sapling", current.sapling_amount(), delta.sapling_amount()),
        pool("orchard", current.orchard_amount(), delta.orchard_amount()),
        pool(
            "lockbox",
            current.deferred_amount(),
            delta.deferred_amount(),
        ),
        pool(
            "ironwood",
            current.ironwood_amount(),
            delta.ironwood_amount(),
        ),
    ];

    let total_zatoshis = [
        current.transparent_amount().zatoshis(),
        current.sprout_amount().zatoshis(),
        current.sapling_amount().zatoshis(),
        current.orchard_amount().zatoshis(),
        current.deferred_amount().zatoshis(),
        current.ironwood_amount().zatoshis(),
    ]
    .into_iter()
    .try_fold(0_i64, |total, pool_zatoshis| {
        total
            .checked_add(pool_zatoshis)
            .ok_or_else(|| Error::Calculation("chain supply exceeds i64".to_string()))
    })?;
    let chain_supply = ValuePoolBalance {
        id: None,
        chain_value: zec(total_zatoshis),
        chain_value_zat: total_zatoshis.to_string(),
        monitored: total_zatoshis != 0,
        value_delta: None,
        value_delta_zat: None,
    };

    Ok((chain_supply, pools.into_iter().collect()))
}

fn pool(
    id: &str,
    current: Amount<NonNegative>,
    delta: Amount<NegativeAllowed>,
) -> ValuePoolBalance {
    let current = current.zatoshis();
    let delta = delta.zatoshis();
    ValuePoolBalance {
        id: Some(id.to_string()),
        chain_value: zec(current),
        chain_value_zat: current.to_string(),
        monitored: current != 0,
        value_delta: Some(zec(delta)),
        value_delta_zat: Some(delta.to_string()),
    }
}

fn zec(zatoshis: i64) -> f64 {
    // Both values are below 2^53, so these integer-to-f64 conversions are exact.
    zatoshis as f64 / COIN as f64
}
