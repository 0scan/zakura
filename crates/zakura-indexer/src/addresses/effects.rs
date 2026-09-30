//! Transparent address effects derived for explorer responses.

use std::collections::HashMap;

use zakura_chain::{
    parameters::Network,
    transaction::Transaction,
    transparent::{Address, OutPoint, Utxo},
};

use crate::Error;

/// One address's public value movement within a canonical transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct AddressEffect {
    pub(super) address: Address,
    pub(super) received_zat: u64,
    pub(super) sent_zat: u64,
}

/// All recognizable transparent address effects produced by one transaction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct TransactionAddressEffects {
    pub(super) effects: Vec<AddressEffect>,
}

pub(super) fn derive_address_effects(
    network: &Network,
    transaction: &Transaction,
    spent_utxos: &HashMap<OutPoint, Utxo>,
) -> Result<TransactionAddressEffects, Error> {
    let mut changes = HashMap::<Address, (u64, u64)>::new();

    for outpoint in transaction
        .inputs()
        .iter()
        .filter_map(|input| input.outpoint())
    {
        let utxo = spent_utxos.get(&outpoint).ok_or_else(|| {
            Error::Calculation(format!(
                "missing transparent output while deriving address effect {outpoint:?}"
            ))
        })?;
        let Some(address) = utxo.output.address(network) else {
            continue;
        };
        let value = non_negative_zatoshis(utxo.output.value().zatoshis())?;
        let sent = &mut changes.entry(address).or_default().1;
        *sent = sent.checked_add(value).ok_or_else(|| {
            Error::Calculation("address sent value exceeds u64 in one transaction".to_string())
        })?;
    }

    for output in transaction.outputs() {
        let Some(address) = output.address(network) else {
            continue;
        };
        let value = non_negative_zatoshis(output.value().zatoshis())?;
        let received = &mut changes.entry(address).or_default().0;
        *received = received.checked_add(value).ok_or_else(|| {
            Error::Calculation("address received value exceeds u64 in one transaction".to_string())
        })?;
    }

    let mut effects = changes
        .into_iter()
        .map(|(address, (received_zat, sent_zat))| AddressEffect {
            address,
            received_zat,
            sent_zat,
        })
        .collect::<Vec<_>>();
    effects.sort_by_key(|effect| effect.address.to_string());

    Ok(TransactionAddressEffects { effects })
}

fn non_negative_zatoshis(value: i64) -> Result<u64, Error> {
    u64::try_from(value).map_err(|_| {
        Error::Calculation("transparent output value must be non-negative".to_string())
    })
}
