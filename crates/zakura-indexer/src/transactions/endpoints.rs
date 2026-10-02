//! Representative transaction source and destination responses.

use zakura_chain::{
    parameters::Network,
    transaction::{primary_value_endpoints, Transaction, TransactionValueEndpoint},
    transparent::Output,
};

use crate::{
    types::{TransactionEndpoint, TransactionEndpointType},
    Error,
};

/// Returns representative source and destination endpoints for a transaction.
///
/// `spent_outputs` must contain one output for each non-coinbase transparent
/// input, in input order. Transparent addresses are aggregated before comparison;
/// shielded pools use their public value balances.
pub fn primary_transaction_endpoints(
    transaction: &Transaction,
    network: &Network,
    spent_outputs: &[Output],
) -> Result<(Option<TransactionEndpoint>, Option<TransactionEndpoint>), Error> {
    let spent_outputs = spent_outputs.iter().collect::<Vec<_>>();
    let (primary_from, primary_to) = primary_value_endpoints(transaction, network, &spent_outputs)
        .map_err(|error| Error::Calculation(error.to_string()))?;
    Ok((
        response_endpoint(primary_from),
        response_endpoint(primary_to),
    ))
}

pub(crate) fn response_endpoint(
    endpoint: Option<TransactionValueEndpoint>,
) -> Option<TransactionEndpoint> {
    endpoint.map(|endpoint| match endpoint {
        TransactionValueEndpoint::Coinbase => TransactionEndpoint {
            endpoint_type: TransactionEndpointType::Coinbase,
            address: None,
        },
        TransactionValueEndpoint::Transparent(address) => TransactionEndpoint {
            endpoint_type: TransactionEndpointType::Transparent,
            address: Some(address.to_string()),
        },
        TransactionValueEndpoint::Sprout => TransactionEndpoint {
            endpoint_type: TransactionEndpointType::Sprout,
            address: None,
        },
        TransactionValueEndpoint::Sapling => TransactionEndpoint {
            endpoint_type: TransactionEndpointType::Sapling,
            address: None,
        },
        TransactionValueEndpoint::Orchard => TransactionEndpoint {
            endpoint_type: TransactionEndpointType::Orchard,
            address: None,
        },
        TransactionValueEndpoint::Ironwood => TransactionEndpoint {
            endpoint_type: TransactionEndpointType::Ironwood,
            address: None,
        },
    })
}

#[cfg(test)]
mod tests {
    use zakura_chain::{parameters::NetworkKind, transparent::Address};

    use super::*;

    #[test]
    fn response_serializes_type_and_optional_address() {
        let address = Address::from_pub_key_hash(NetworkKind::Mainnet, [7; 20]);
        let response = response_endpoint(Some(TransactionValueEndpoint::Transparent(address)))
            .expect("a transparent endpoint produces a response");

        assert_eq!(response.endpoint_type, TransactionEndpointType::Transparent);
        assert_eq!(response.address, Some(address.to_string()));
        let json = serde_json::to_value(response).expect("transaction endpoint serializes");
        assert_eq!(json["type"], "transparent");
        assert_eq!(json["address"], address.to_string());

        let pool = response_endpoint(Some(TransactionValueEndpoint::Ironwood))
            .expect("an Ironwood endpoint produces a response");
        let json = serde_json::to_value(pool).expect("transaction endpoint serializes");
        assert_eq!(json["type"], "ironwood");
        assert!(json.get("address").is_none());
    }
}
