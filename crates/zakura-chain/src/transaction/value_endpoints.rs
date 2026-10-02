//! Representative transaction value sources and destinations.

use std::{cmp::Ordering, collections::HashMap};

use crate::{
    orchard,
    parameters::Network,
    transaction::Transaction,
    transparent::{Address, Output},
    value_balance::ValueBalanceError,
};

/// One representative source or destination for a transaction's observable value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransactionValueEndpoint {
    /// Newly issued value from a block's coinbase transaction.
    Coinbase,
    /// A public transparent address.
    Transparent(Address),
    /// The Sprout shielded pool.
    Sprout,
    /// The Sapling shielded pool.
    Sapling,
    /// The Orchard shielded pool.
    Orchard,
    /// The Ironwood shielded pool.
    Ironwood,
}

/// Errors returned while deriving representative transaction endpoints.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum PrimaryValueEndpointsError {
    /// The resolved transparent outputs do not parallel the transaction inputs.
    #[error(
        "transaction has {transparent_input_count} transparent inputs but {resolved_output_count} resolved outputs"
    )]
    MismatchedTransparentInputs {
        /// Number of non-coinbase transparent inputs in the transaction.
        transparent_input_count: usize,
        /// Number of resolved outputs supplied by the caller.
        resolved_output_count: usize,
    },

    /// The transaction's Sprout value balance could not be calculated.
    #[error("could not derive the Sprout value balance")]
    SproutValueBalance(#[from] ValueBalanceError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct EndpointCandidate {
    endpoint: TransactionValueEndpoint,
    value_zat: u64,
}

/// Returns representative source and destination endpoints for `transaction`.
///
/// `spent_outputs` must contain one output for each non-coinbase transparent
/// input, in input order. Values are aggregated by transparent address before
/// comparison. Shielded pools use public value balances, while a zero-balance
/// shielded bundle still identifies its pool because dummy notes are private.
/// The largest observable value wins; equal values prefer a transparent address,
/// then Sprout, Sapling, Orchard, and Ironwood in that order.
pub fn primary_value_endpoints(
    transaction: &Transaction,
    network: &Network,
    spent_outputs: &[&Output],
) -> Result<
    (
        Option<TransactionValueEndpoint>,
        Option<TransactionValueEndpoint>,
    ),
    PrimaryValueEndpointsError,
> {
    let transparent_input_count = transaction
        .inputs()
        .iter()
        .filter_map(|input| input.outpoint())
        .count();
    if transparent_input_count != spent_outputs.len() {
        return Err(PrimaryValueEndpointsError::MismatchedTransparentInputs {
            transparent_input_count,
            resolved_output_count: spent_outputs.len(),
        });
    }

    let mut sources = Vec::new();
    let mut destinations = Vec::new();
    let is_coinbase = transaction.is_coinbase();

    if !is_coinbase {
        append_transparent_candidates(&mut sources, network, spent_outputs.iter().copied());
    }
    append_transparent_candidates(&mut destinations, network, transaction.outputs().iter());

    append_pool_candidates(
        &mut sources,
        &mut destinations,
        TransactionValueEndpoint::Sprout,
        transaction
            .sprout_value_balance()?
            .sprout_amount()
            .zatoshis(),
        transaction.joinsplit_count() > 0,
        transaction.joinsplit_count() > 0,
    );
    append_pool_candidates(
        &mut sources,
        &mut destinations,
        TransactionValueEndpoint::Sapling,
        transaction
            .sapling_value_balance()
            .sapling_amount()
            .zatoshis(),
        transaction.sapling_spends_per_anchor().next().is_some(),
        transaction.sapling_outputs().next().is_some(),
    );

    let orchard_action_count = transaction.orchard_actions().count();
    let orchard_flags = transaction
        .orchard_flags()
        .unwrap_or_else(orchard::Flags::empty);
    append_pool_candidates(
        &mut sources,
        &mut destinations,
        TransactionValueEndpoint::Orchard,
        transaction
            .orchard_value_balance()
            .orchard_amount()
            .zatoshis(),
        orchard_action_count > 0 && orchard_flags.contains(orchard::Flags::ENABLE_SPENDS),
        orchard_action_count > 0 && orchard_flags.contains(orchard::Flags::ENABLE_OUTPUTS),
    );

    let ironwood_action_count = transaction.ironwood_actions().count();
    let ironwood_flags = transaction
        .ironwood_flags()
        .unwrap_or_else(orchard::Flags::empty);
    append_pool_candidates(
        &mut sources,
        &mut destinations,
        TransactionValueEndpoint::Ironwood,
        transaction
            .ironwood_value_balance()
            .ironwood_amount()
            .zatoshis(),
        ironwood_action_count > 0 && ironwood_flags.contains(orchard::Flags::ENABLE_SPENDS),
        ironwood_action_count > 0 && ironwood_flags.contains(orchard::Flags::ENABLE_OUTPUTS),
    );

    let primary_from = if is_coinbase {
        Some(TransactionValueEndpoint::Coinbase)
    } else {
        select_primary(sources)
    };
    Ok((primary_from, select_primary(destinations)))
}

fn append_transparent_candidates<'a>(
    candidates: &mut Vec<EndpointCandidate>,
    network: &Network,
    outputs: impl Iterator<Item = &'a Output>,
) {
    let mut totals = HashMap::new();
    for output in outputs {
        if let Some(address) = output.address(network) {
            let value_zat = u64::try_from(output.value().zatoshis())
                .expect("transparent output values are nonnegative");
            let total = totals.entry(address).or_insert(0_u64);
            *total = total
                .checked_add(value_zat)
                .expect("transparent address totals fit in the money range");
        }
    }
    candidates.extend(
        totals
            .into_iter()
            .map(|(address, value_zat)| EndpointCandidate {
                endpoint: TransactionValueEndpoint::Transparent(address),
                value_zat,
            }),
    );
}

fn append_pool_candidates(
    sources: &mut Vec<EndpointCandidate>,
    destinations: &mut Vec<EndpointCandidate>,
    endpoint: TransactionValueEndpoint,
    value_balance_zat: i64,
    has_spends: bool,
    has_outputs: bool,
) {
    if has_spends {
        sources.push(EndpointCandidate {
            endpoint,
            value_zat: u64::try_from(value_balance_zat).unwrap_or(0),
        });
    }
    if has_outputs {
        destinations.push(EndpointCandidate {
            endpoint,
            value_zat: if value_balance_zat < 0 {
                value_balance_zat.unsigned_abs()
            } else {
                0
            },
        });
    }
}

fn select_primary(candidates: Vec<EndpointCandidate>) -> Option<TransactionValueEndpoint> {
    candidates
        .into_iter()
        .reduce(|current, candidate| {
            if compare_candidates(candidate, current).is_gt() {
                candidate
            } else {
                current
            }
        })
        .map(|candidate| candidate.endpoint)
}

fn compare_candidates(left: EndpointCandidate, right: EndpointCandidate) -> Ordering {
    left.value_zat
        .cmp(&right.value_zat)
        .then_with(|| endpoint_preference(right.endpoint).cmp(&endpoint_preference(left.endpoint)))
}

fn endpoint_preference(endpoint: TransactionValueEndpoint) -> (u8, String) {
    match endpoint {
        TransactionValueEndpoint::Transparent(address) => (0, address.to_string()),
        TransactionValueEndpoint::Sprout => (1, String::new()),
        TransactionValueEndpoint::Sapling => (2, String::new()),
        TransactionValueEndpoint::Orchard => (3, String::new()),
        TransactionValueEndpoint::Ironwood => (4, String::new()),
        TransactionValueEndpoint::Coinbase => (5, String::new()),
    }
}

#[cfg(test)]
mod tests {
    use crate::parameters::NetworkKind;

    use super::*;

    #[test]
    fn primary_endpoint_uses_value_then_stable_preference() {
        let address = Address::from_pub_key_hash(NetworkKind::Mainnet, [7; 20]);
        assert_eq!(
            select_primary(vec![
                EndpointCandidate {
                    endpoint: TransactionValueEndpoint::Orchard,
                    value_zat: 10,
                },
                EndpointCandidate {
                    endpoint: TransactionValueEndpoint::Ironwood,
                    value_zat: 11,
                },
            ]),
            Some(TransactionValueEndpoint::Ironwood)
        );
        assert_eq!(
            select_primary(vec![
                EndpointCandidate {
                    endpoint: TransactionValueEndpoint::Sapling,
                    value_zat: 10,
                },
                EndpointCandidate {
                    endpoint: TransactionValueEndpoint::Transparent(address),
                    value_zat: 10,
                },
            ]),
            Some(TransactionValueEndpoint::Transparent(address))
        );
    }

    #[test]
    fn zero_balance_pool_still_represents_fully_shielded_activity() {
        let mut sources = Vec::new();
        let mut destinations = Vec::new();
        append_pool_candidates(
            &mut sources,
            &mut destinations,
            TransactionValueEndpoint::Ironwood,
            0,
            true,
            true,
        );

        assert_eq!(
            select_primary(sources),
            Some(TransactionValueEndpoint::Ironwood)
        );
        assert_eq!(
            select_primary(destinations),
            Some(TransactionValueEndpoint::Ironwood)
        );
    }
}
