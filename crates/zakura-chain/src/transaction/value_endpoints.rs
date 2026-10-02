//! Representative transaction value sources and destinations.

use std::collections::HashMap;

use crate::{
    amount::{Amount, NegativeAllowed, NonNegative},
    orchard,
    parameters::Network,
    transaction::Transaction,
    transparent::{Address, Output},
    value_balance::{ValueBalance, ValueBalanceError},
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

    /// A shielded or aggregate transaction value balance could not be calculated.
    #[error("could not derive the transaction value balance")]
    TransactionValueBalance(#[from] ValueBalanceError),

    /// The transaction's transparent value balance could not be calculated.
    #[error("could not derive the transparent value balance")]
    TransparentValueBalance(#[source] crate::amount::Error),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct EndpointCandidate {
    endpoint: TransactionValueEndpoint,
    value_zat: u64,
}

/// Reusable results from analyzing a transaction's observable value endpoints.
#[derive(Debug)]
pub struct TransactionValueEndpoints {
    primary_from: Option<TransactionValueEndpoint>,
    primary_to: Option<TransactionValueEndpoint>,
    transparent_addresses: HashMap<Address, TransparentAddressValues>,
    value_balance: ValueBalance<NegativeAllowed>,
    transparent_output_total_zat: i64,
}

impl TransactionValueEndpoints {
    /// Returns the representative source and destination endpoints.
    pub fn primary_endpoints(
        &self,
    ) -> (
        Option<TransactionValueEndpoint>,
        Option<TransactionValueEndpoint>,
    ) {
        (self.primary_from, self.primary_to)
    }

    /// Returns the transaction's value balance calculated from the supplied inputs.
    pub fn value_balance(&self) -> ValueBalance<NegativeAllowed> {
        self.value_balance
    }

    /// Returns the total value of every transparent output in zatoshis.
    pub fn transparent_output_total_zat(&self) -> i64 {
        self.transparent_output_total_zat
    }

    /// Returns every transparent address involved in the transaction, paired
    /// with the total value received by that address.
    pub fn into_transparent_address_received_values(self) -> impl Iterator<Item = (Address, u64)> {
        self.transparent_addresses
            .into_iter()
            .map(|(address, values)| (address, values.output_value_zat))
    }
}

#[derive(Default, Debug)]
struct TransparentAddressValues {
    input_value_zat: u64,
    output_value_zat: u64,
    has_inputs: bool,
    has_outputs: bool,
}

#[derive(Clone, Copy)]
enum TransparentValueSide {
    Input,
    Output,
}

#[derive(Default)]
struct PrimaryEndpointSelector {
    primary: Option<EndpointCandidate>,
    primary_address_text: Option<String>,
}

impl PrimaryEndpointSelector {
    fn consider(&mut self, candidate: EndpointCandidate) {
        let Some(current) = self.primary else {
            self.replace(candidate);
            return;
        };

        match candidate.value_zat.cmp(&current.value_zat) {
            std::cmp::Ordering::Greater => self.replace(candidate),
            std::cmp::Ordering::Less => {}
            std::cmp::Ordering::Equal => {
                let candidate_rank = endpoint_preference_rank(candidate.endpoint);
                let current_rank = endpoint_preference_rank(current.endpoint);
                if candidate_rank < current_rank {
                    self.replace(candidate);
                } else if candidate_rank == current_rank && candidate_rank == 0 {
                    self.consider_equal_value_transparent(candidate, current);
                }
            }
        }
    }

    fn consider_equal_value_transparent(
        &mut self,
        candidate: EndpointCandidate,
        current: EndpointCandidate,
    ) {
        let TransactionValueEndpoint::Transparent(candidate_address) = candidate.endpoint else {
            return;
        };
        let TransactionValueEndpoint::Transparent(current_address) = current.endpoint else {
            return;
        };

        let current_address_text = self
            .primary_address_text
            .get_or_insert_with(|| current_address.to_string());
        let candidate_address_text = candidate_address.to_string();
        if candidate_address_text < *current_address_text {
            self.primary = Some(candidate);
            self.primary_address_text = Some(candidate_address_text);
        }
    }

    fn replace(&mut self, candidate: EndpointCandidate) {
        self.primary = Some(candidate);
        self.primary_address_text = None;
    }

    fn into_endpoint(self) -> Option<TransactionValueEndpoint> {
        self.primary.map(|candidate| candidate.endpoint)
    }
}

/// Returns representative source and destination endpoints for `transaction`.
///
/// `spent_outputs` must contain one output for each non-coinbase transparent
/// input, in input order. Values are aggregated by transparent address before
/// comparison. Shielded pools use public value balances, while a zero-balance
/// shielded bundle still identifies its pool because dummy notes are private.
/// The largest observable value wins; equal values prefer a transparent address,
/// then Sprout, Sapling, Orchard, and Ironwood in that order.
pub fn primary_value_endpoints<'a>(
    transaction: &Transaction,
    network: &Network,
    spent_outputs: impl IntoIterator<Item = &'a Output>,
) -> Result<
    (
        Option<TransactionValueEndpoint>,
        Option<TransactionValueEndpoint>,
    ),
    PrimaryValueEndpointsError,
> {
    Ok(transaction_value_endpoints(transaction, network, spent_outputs)?.primary_endpoints())
}

/// Returns reusable endpoint and transparent-address analysis for `transaction`.
///
/// This function has the same input requirements and endpoint selection rules as
/// [`primary_value_endpoints`]. The transparent-address results can be reused by
/// callers that also maintain address indexes, avoiding a second output scan.
pub fn transaction_value_endpoints<'a>(
    transaction: &Transaction,
    network: &Network,
    spent_outputs: impl IntoIterator<Item = &'a Output>,
) -> Result<TransactionValueEndpoints, PrimaryValueEndpointsError> {
    let is_coinbase = transaction.is_coinbase();
    let transparent_input_count = transaction
        .inputs()
        .iter()
        .filter_map(|input| input.outpoint())
        .count();
    let mut transparent_addresses = HashMap::new();
    let mut transparent_input_total_zat = 0_u64;
    let resolved_output_count = if is_coinbase {
        spent_outputs.into_iter().count()
    } else {
        append_transparent_values(
            &mut transparent_addresses,
            network,
            spent_outputs.into_iter(),
            TransparentValueSide::Input,
            &mut transparent_input_total_zat,
        )
    };
    if transparent_input_count != resolved_output_count {
        return Err(PrimaryValueEndpointsError::MismatchedTransparentInputs {
            transparent_input_count,
            resolved_output_count,
        });
    }

    let mut transparent_output_total_zat = 0_u64;
    append_transparent_values(
        &mut transparent_addresses,
        network,
        transaction.outputs().iter(),
        TransparentValueSide::Output,
        &mut transparent_output_total_zat,
    );

    let mut sources = PrimaryEndpointSelector::default();
    let mut destinations = PrimaryEndpointSelector::default();
    append_transparent_candidates(&mut sources, &mut destinations, &transparent_addresses);

    let has_joinsplits = transaction.joinsplit_count() > 0;
    let sprout_value_balance = if has_joinsplits {
        let value_balance = transaction.sprout_value_balance()?;
        append_pool_candidates(
            &mut sources,
            &mut destinations,
            TransactionValueEndpoint::Sprout,
            value_balance.sprout_amount().zatoshis(),
            true,
            true,
        );
        value_balance
    } else {
        ValueBalance::zero()
    };

    let has_sapling_spends = transaction.sapling_spends_per_anchor().next().is_some();
    let has_sapling_outputs = transaction.sapling_outputs().next().is_some();
    let sapling_value_balance = transaction.sapling_value_balance();
    if has_sapling_spends || has_sapling_outputs {
        append_pool_candidates(
            &mut sources,
            &mut destinations,
            TransactionValueEndpoint::Sapling,
            sapling_value_balance.sapling_amount().zatoshis(),
            has_sapling_spends,
            has_sapling_outputs,
        );
    }

    let has_orchard_actions = transaction.orchard_actions().next().is_some();
    let orchard_flags = transaction
        .orchard_flags()
        .unwrap_or_else(orchard::Flags::empty);
    let orchard_value_balance = transaction.orchard_value_balance();
    if has_orchard_actions {
        append_pool_candidates(
            &mut sources,
            &mut destinations,
            TransactionValueEndpoint::Orchard,
            orchard_value_balance.orchard_amount().zatoshis(),
            orchard_flags.contains(orchard::Flags::ENABLE_SPENDS),
            orchard_flags.contains(orchard::Flags::ENABLE_OUTPUTS),
        );
    }

    let has_ironwood_actions = transaction.ironwood_actions().next().is_some();
    let ironwood_flags = transaction
        .ironwood_flags()
        .unwrap_or_else(orchard::Flags::empty);
    let ironwood_value_balance = transaction.ironwood_value_balance();
    if has_ironwood_actions {
        append_pool_candidates(
            &mut sources,
            &mut destinations,
            TransactionValueEndpoint::Ironwood,
            ironwood_value_balance.ironwood_amount().zatoshis(),
            ironwood_flags.contains(orchard::Flags::ENABLE_SPENDS),
            ironwood_flags.contains(orchard::Flags::ENABLE_OUTPUTS),
        );
    }

    let primary_from = if is_coinbase {
        Some(TransactionValueEndpoint::Coinbase)
    } else {
        sources.into_endpoint()
    };
    let transparent_input_total: Amount<NegativeAllowed> =
        Amount::<NonNegative>::try_from(transparent_input_total_zat)
            .map_err(PrimaryValueEndpointsError::TransparentValueBalance)?
            .constrain()
            .map_err(PrimaryValueEndpointsError::TransparentValueBalance)?;
    let transparent_output_total: Amount<NegativeAllowed> =
        Amount::<NonNegative>::try_from(transparent_output_total_zat)
            .map_err(PrimaryValueEndpointsError::TransparentValueBalance)?
            .constrain()
            .map_err(PrimaryValueEndpointsError::TransparentValueBalance)?;
    let transparent_value_balance = (transparent_input_total - transparent_output_total)
        .map(ValueBalance::from_transparent_amount)
        .map_err(PrimaryValueEndpointsError::TransparentValueBalance)?;
    let value_balance = (transparent_value_balance
        + sprout_value_balance
        + sapling_value_balance
        + orchard_value_balance
        + ironwood_value_balance)?;
    Ok(TransactionValueEndpoints {
        primary_from,
        primary_to: destinations.into_endpoint(),
        transparent_addresses,
        value_balance,
        transparent_output_total_zat: transparent_output_total.zatoshis(),
    })
}

fn append_transparent_values<'a>(
    values_by_address: &mut HashMap<Address, TransparentAddressValues>,
    network: &Network,
    outputs: impl Iterator<Item = &'a Output>,
    side: TransparentValueSide,
    total_value_zat: &mut u64,
) -> usize {
    let mut output_count = 0;
    for output in outputs {
        output_count += 1;
        let value_zat = u64::try_from(output.value().zatoshis())
            .expect("transparent output values are nonnegative");
        *total_value_zat = total_value_zat
            .checked_add(value_zat)
            .expect("transparent output totals fit in u64");
        if let Some(address) = output.address(network) {
            let values = values_by_address.entry(address).or_default();
            let (total, is_present) = match side {
                TransparentValueSide::Input => {
                    (&mut values.input_value_zat, &mut values.has_inputs)
                }
                TransparentValueSide::Output => {
                    (&mut values.output_value_zat, &mut values.has_outputs)
                }
            };
            *total = total
                .checked_add(value_zat)
                .expect("transparent address totals fit in the money range");
            *is_present = true;
        }
    }
    output_count
}

fn append_transparent_candidates(
    sources: &mut PrimaryEndpointSelector,
    destinations: &mut PrimaryEndpointSelector,
    values_by_address: &HashMap<Address, TransparentAddressValues>,
) {
    for (&address, values) in values_by_address {
        if values.has_inputs {
            sources.consider(EndpointCandidate {
                endpoint: TransactionValueEndpoint::Transparent(address),
                value_zat: values.input_value_zat,
            });
        }
        if values.has_outputs {
            destinations.consider(EndpointCandidate {
                endpoint: TransactionValueEndpoint::Transparent(address),
                value_zat: values.output_value_zat,
            });
        }
    }
}

fn append_pool_candidates(
    sources: &mut PrimaryEndpointSelector,
    destinations: &mut PrimaryEndpointSelector,
    endpoint: TransactionValueEndpoint,
    value_balance_zat: i64,
    has_spends: bool,
    has_outputs: bool,
) {
    if has_spends {
        sources.consider(EndpointCandidate {
            endpoint,
            value_zat: u64::try_from(value_balance_zat).unwrap_or(0),
        });
    }
    if has_outputs {
        destinations.consider(EndpointCandidate {
            endpoint,
            value_zat: if value_balance_zat < 0 {
                value_balance_zat.unsigned_abs()
            } else {
                0
            },
        });
    }
}

fn endpoint_preference_rank(endpoint: TransactionValueEndpoint) -> u8 {
    match endpoint {
        TransactionValueEndpoint::Transparent(_) => 0,
        TransactionValueEndpoint::Sprout => 1,
        TransactionValueEndpoint::Sapling => 2,
        TransactionValueEndpoint::Orchard => 3,
        TransactionValueEndpoint::Ironwood => 4,
        TransactionValueEndpoint::Coinbase => 5,
    }
}

#[cfg(test)]
mod tests {
    use crate::{amount::Amount, parameters::NetworkKind};

    use super::*;

    #[test]
    fn primary_endpoint_uses_value_then_stable_preference() {
        let address = Address::from_pub_key_hash(NetworkKind::Mainnet, [7; 20]);
        let mut selector = PrimaryEndpointSelector::default();
        selector.consider(EndpointCandidate {
            endpoint: TransactionValueEndpoint::Orchard,
            value_zat: 10,
        });
        selector.consider(EndpointCandidate {
            endpoint: TransactionValueEndpoint::Ironwood,
            value_zat: 11,
        });
        assert_eq!(
            selector.into_endpoint(),
            Some(TransactionValueEndpoint::Ironwood)
        );

        let mut selector = PrimaryEndpointSelector::default();
        selector.consider(EndpointCandidate {
            endpoint: TransactionValueEndpoint::Sapling,
            value_zat: 10,
        });
        selector.consider(EndpointCandidate {
            endpoint: TransactionValueEndpoint::Transparent(address),
            value_zat: 10,
        });
        assert_eq!(
            selector.into_endpoint(),
            Some(TransactionValueEndpoint::Transparent(address))
        );
    }

    #[test]
    fn equal_transparent_values_use_address_text_as_a_stable_tie_breaker() {
        let first = Address::from_pub_key_hash(NetworkKind::Mainnet, [1; 20]);
        let second = Address::from_pub_key_hash(NetworkKind::Mainnet, [2; 20]);
        let expected = if first.to_string() < second.to_string() {
            first
        } else {
            second
        };

        for addresses in [[first, second], [second, first]] {
            let mut selector = PrimaryEndpointSelector::default();
            for address in addresses {
                selector.consider(EndpointCandidate {
                    endpoint: TransactionValueEndpoint::Transparent(address),
                    value_zat: 10,
                });
            }
            assert_eq!(
                selector.into_endpoint(),
                Some(TransactionValueEndpoint::Transparent(expected))
            );
        }
    }

    #[test]
    fn transparent_address_analysis_is_reusable_for_address_indexes() {
        let address = Address::from_pub_key_hash(NetworkKind::Mainnet, [1; 20]);
        let zero_value_address = Address::from_pub_key_hash(NetworkKind::Mainnet, [2; 20]);
        let spent_outputs = [Output::new(
            Amount::try_from(3).expect("three zatoshis is a valid amount"),
            address.script(),
        )];
        let transaction_outputs = [
            Output::new(
                Amount::try_from(4).expect("four zatoshis is a valid amount"),
                address.script(),
            ),
            Output::new(Amount::zero(), zero_value_address.script()),
        ];
        let mut transparent_addresses = HashMap::new();
        let mut input_total_zat = 0;
        let mut output_total_zat = 0;

        append_transparent_values(
            &mut transparent_addresses,
            &Network::Mainnet,
            spent_outputs.iter(),
            TransparentValueSide::Input,
            &mut input_total_zat,
        );
        append_transparent_values(
            &mut transparent_addresses,
            &Network::Mainnet,
            transaction_outputs.iter(),
            TransparentValueSide::Output,
            &mut output_total_zat,
        );

        let value_endpoints = TransactionValueEndpoints {
            primary_from: None,
            primary_to: None,
            transparent_addresses,
            value_balance: ValueBalance::from_transparent_amount(
                Amount::try_from(-1).expect("minus one zatoshi is a valid signed amount"),
            ),
            transparent_output_total_zat: i64::try_from(output_total_zat)
                .expect("test output total fits in i64"),
        };
        assert_eq!(input_total_zat, 3);
        assert_eq!(value_endpoints.transparent_output_total_zat(), 4);
        assert_eq!(
            value_endpoints
                .value_balance()
                .transparent_amount()
                .zatoshis(),
            -1
        );

        let received_values = value_endpoints
            .into_transparent_address_received_values()
            .collect::<HashMap<_, _>>();
        assert_eq!(received_values.get(&address), Some(&4));
        assert_eq!(received_values.get(&zero_value_address), Some(&0));
    }

    #[test]
    fn zero_balance_pool_still_represents_fully_shielded_activity() {
        let mut sources = PrimaryEndpointSelector::default();
        let mut destinations = PrimaryEndpointSelector::default();
        append_pool_candidates(
            &mut sources,
            &mut destinations,
            TransactionValueEndpoint::Ironwood,
            0,
            true,
            true,
        );

        assert_eq!(
            sources.into_endpoint(),
            Some(TransactionValueEndpoint::Ironwood)
        );
        assert_eq!(
            destinations.into_endpoint(),
            Some(TransactionValueEndpoint::Ironwood)
        );
    }
}
