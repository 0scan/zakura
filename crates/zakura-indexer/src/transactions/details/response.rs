//! Transaction response construction shared by transaction and block details.

use std::collections::HashMap;

use zakura_chain::{
    block::{Block, Hash, Height},
    parameters::Network,
    serialization::ZcashSerialize,
    transaction::Transaction,
    transparent::{Input, OutPoint, Utxo},
};

use crate::{
    models::{TransactionPosition, TransactionRecord},
    types::{
        BlockTransaction, BlockTransactionInput, BlockTransactionOutput, TransactionData,
        TransactionDetails, TransactionStatus,
    },
    Error,
};

use super::super::{
    classify::{public_flow_amount, shielded_flow, shielded_pool, transaction_kind},
    primary_transaction_endpoints, response_endpoint,
};

/// Canonical block context for one transaction response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TransactionBlockContext {
    pub(crate) position: TransactionPosition,
    pub(crate) block_hash: Hash,
    pub(crate) block_time: i64,
}

/// Current best-chain status loaded after resolving indexed block context.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct TransactionChainStatus {
    pub(super) confirmations: u32,
    pub(super) finalized: bool,
}

impl TransactionChainStatus {
    fn response_status(self) -> TransactionStatus {
        if self.finalized {
            TransactionStatus::Finalized
        } else {
            TransactionStatus::Confirmed
        }
    }
}

pub(crate) fn build_block_transactions(
    network: &Network,
    block: &Block,
    height: Height,
    hash: Hash,
    transparent_inputs: &HashMap<OutPoint, Utxo>,
    spent_outputs: &HashMap<OutPoint, bool>,
) -> Result<Vec<BlockTransaction>, Error> {
    block
        .transactions
        .iter()
        .enumerate()
        .map(|(transaction_index, transaction)| {
            let transaction_index = count_u32(transaction_index, "transaction index")?;
            build_transaction(
                network,
                transaction,
                TransactionBlockContext {
                    position: TransactionPosition {
                        height,
                        transaction_index,
                    },
                    block_hash: hash,
                    block_time: block.header.time.timestamp(),
                },
                transparent_inputs,
                spent_outputs,
            )
        })
        .collect()
}

pub(super) fn build_transaction_details(
    network: &Network,
    transaction: &Transaction,
    context: TransactionBlockContext,
    record: TransactionRecord,
    status: TransactionChainStatus,
    transparent_inputs: &HashMap<OutPoint, Utxo>,
    spent_outputs: &HashMap<OutPoint, bool>,
) -> Result<TransactionDetails, Error> {
    let response = build_transaction(
        network,
        transaction,
        context,
        transparent_inputs,
        spent_outputs,
    )?;
    validate_indexed_record(transaction, record, &response)?;

    let flow_amount_zat = public_flow_amount(&record)?;
    let coinbase_script = transaction
        .inputs()
        .first()
        .and_then(Input::coinbase_script);

    Ok(TransactionDetails {
        transaction: response,
        status: status.response_status(),
        confirmations: status.confirmations,
        kind: transaction_kind(&record),
        pool: shielded_pool(&record),
        flow: shielded_flow(&record)?,
        flow_amount_zat: flow_amount_zat.map(|amount| amount.to_string()),
        joinsplit_count: record.joinsplit_count,
        coinbase_hex: coinbase_script.as_deref().map(hex::encode),
    })
}

fn build_transaction(
    network: &Network,
    transaction: &Transaction,
    context: TransactionBlockContext,
    transparent_inputs: &HashMap<OutPoint, Utxo>,
    spent_outputs: &HashMap<OutPoint, bool>,
) -> Result<BlockTransaction, Error> {
    let is_coinbase = transaction.is_coinbase();
    if is_coinbase != (context.position.transaction_index == 0) {
        return Err(Error::CorruptData(format!(
            "transaction {} coinbase status does not match index {}",
            transaction.hash(),
            context.position.transaction_index
        )));
    }

    let txid = transaction.hash();
    let inputs = transaction
        .inputs()
        .iter()
        .filter_map(|input| match input {
            Input::Coinbase { .. } => None,
            Input::PrevOut {
                outpoint,
                unlock_script,
                sequence,
            } => Some((outpoint, unlock_script, sequence)),
        })
        .map(|(outpoint, unlock_script, sequence)| {
            let utxo = transparent_inputs.get(outpoint).ok_or_else(|| {
                Error::CorruptData(format!(
                    "missing loaded transparent output for {outpoint:?}"
                ))
            })?;
            Ok(BlockTransactionInput {
                previous_transaction_id: outpoint.hash.to_string(),
                previous_output_index: outpoint.index,
                address: utxo
                    .output
                    .address(network)
                    .map(|address| address.to_string()),
                value: utxo.output.value().zatoshis().to_string(),
                script_sig: hex::encode(unlock_script.as_raw_bytes()),
                sequence: *sequence,
            })
        })
        .collect::<Result<Vec<_>, Error>>()?;

    let outputs = transaction
        .outputs()
        .iter()
        .enumerate()
        .map(|(output_index, output)| {
            let output_index = count_u32(output_index, "transaction output index")?;
            let outpoint = OutPoint {
                hash: txid,
                index: output_index,
            };
            let spent = spent_outputs.get(&outpoint).copied().ok_or_else(|| {
                Error::StateResponse(format!(
                    "missing spent status for transaction output {outpoint:?}"
                ))
            })?;
            Ok(BlockTransactionOutput {
                transaction_id: txid.to_string(),
                address: output.address(network).map(|address| address.to_string()),
                value: output.value().zatoshis().to_string(),
                output_index,
                script_pub_key: hex::encode(output.lock_script.as_raw_bytes()),
                spent,
            })
        })
        .collect::<Result<Vec<_>, Error>>()?;

    let total_input = transaction
        .inputs()
        .iter()
        .filter_map(Input::outpoint)
        .try_fold(0_i64, |total, outpoint| {
            let utxo = transparent_inputs.get(&outpoint).ok_or_else(|| {
                Error::StateResponse(format!(
                    "missing loaded transparent output for {outpoint:?}"
                ))
            })?;
            total
                .checked_add(utxo.output.value().zatoshis())
                .ok_or_else(|| {
                    Error::Calculation("transaction input total exceeds i64".to_string())
                })
        })?;
    let total_output = transaction
        .outputs()
        .iter()
        .try_fold(0_i64, |total, output| {
            total.checked_add(output.value().zatoshis()).ok_or_else(|| {
                Error::Calculation("transaction output total exceeds i64".to_string())
            })
        })?;
    let (fee, value_balance_transparent) = if is_coinbase {
        (0, 0)
    } else {
        let transaction_value_balance = transaction
            .value_balance(transparent_inputs)
            .map_err(|error| Error::Calculation(error.to_string()))?;
        let fee = transaction_value_balance
            .remaining_transaction_value()
            .map_err(|error| Error::Calculation(error.to_string()))?
            .zatoshis();
        (
            fee,
            transaction_value_balance.transparent_amount().zatoshis(),
        )
    };

    let sapling_spend_count = count_u32(
        transaction.sapling_spends_per_anchor().count(),
        "Sapling spend count",
    )?;
    let sapling_output_count = count_u32(
        transaction.sapling_outputs().count(),
        "Sapling output count",
    )?;
    let orchard_actions = count_u32(
        transaction.orchard_actions().count(),
        "Orchard action count",
    )?;
    let ironwood_actions = count_u32(
        transaction.ironwood_actions().count(),
        "Ironwood action count",
    )?;
    let has_sprout = transaction.joinsplit_count() > 0;
    let value_balance_sapling = transaction.sapling_value_balance().sapling_amount();
    let value_balance_orchard = transaction.orchard_value_balance().orchard_amount();
    let value_balance_ironwood = transaction.ironwood_value_balance().ironwood_amount();
    let value_balance = [
        value_balance_sapling.zatoshis(),
        value_balance_orchard.zatoshis(),
        value_balance_ironwood.zatoshis(),
    ]
    .into_iter()
    .try_fold(0_i64, |total, value| {
        total
            .checked_add(value)
            .ok_or_else(|| Error::Calculation("transaction value balance exceeds i64".to_string()))
    })?;
    let serialized = transaction
        .zcash_serialize_to_vec()
        .map_err(|error| Error::Calculation(error.to_string()))?;
    let spent_outputs = transaction
        .inputs()
        .iter()
        .filter_map(Input::outpoint)
        .map(|outpoint| {
            transparent_inputs
                .get(&outpoint)
                .map(|utxo| utxo.output.clone())
                .ok_or_else(|| {
                    Error::StateResponse(format!(
                        "missing loaded transparent output for {outpoint:?}"
                    ))
                })
        })
        .collect::<Result<Vec<_>, Error>>()?;
    let (primary_from, primary_to) =
        primary_transaction_endpoints(transaction, network, &spent_outputs)?;

    let transaction = TransactionData {
        txid: txid.to_string(),
        hex: hex::encode(&serialized),
        size: count_u32(serialized.len(), "serialized transaction size")?,
        version: transaction.version(),
        version_group_id: transaction
            .version_group_id()
            .map(|id| hex::encode(id.to_le_bytes())),
        lock_time: transaction.raw_lock_time().to_string(),
        expiry_height: transaction
            .expiry_height()
            .map(|height| height.0.to_string()),
        auth_digest: transaction.auth_digest().map(|digest| digest.to_string()),
        overwintered: transaction.is_overwintered(),
        primary_from,
        primary_to,
        vin_count: count_u32(transaction.inputs().len(), "transparent input count")?,
        vout_count: count_u32(transaction.outputs().len(), "transparent output count")?,
        value_balance: value_balance.to_string(),
        value_balance_transparent: value_balance_transparent.to_string(),
        value_balance_sapling: value_balance_sapling.zatoshis().to_string(),
        value_balance_orchard: value_balance_orchard.zatoshis().to_string(),
        value_balance_ironwood: value_balance_ironwood.zatoshis().to_string(),
        has_sapling: sapling_spend_count > 0 || sapling_output_count > 0,
        has_orchard: orchard_actions > 0,
        has_ironwood: ironwood_actions > 0,
        has_sprout,
        sapling_spend_count,
        sapling_output_count,
        orchard_actions,
        ironwood_actions,
        fee: fee.to_string(),
        total_input: total_input.to_string(),
        total_output: total_output.to_string(),
        is_coinbase,
        inputs,
        outputs,
    };

    Ok(BlockTransaction {
        transaction,
        block_height: context.position.height.0.to_string(),
        block_hash: context.block_hash.to_string(),
        block_time: context.block_time.to_string(),
        transaction_index: context.position.transaction_index,
    })
}

fn validate_indexed_record(
    transaction: &Transaction,
    record: TransactionRecord,
    response: &BlockTransaction,
) -> Result<(), Error> {
    let joinsplit_count = count_u32(transaction.joinsplit_count(), "Sprout JoinSplit count")?;
    let indexed_input_count = if response.transaction.is_coinbase {
        0
    } else {
        response.transaction.vin_count
    };
    let matches = response.transaction.size == record.serialized_size
        && response.transaction.fee == record.fee_zat.to_string()
        && response.transaction.primary_from == response_endpoint(record.primary_from)
        && response.transaction.primary_to == response_endpoint(record.primary_to)
        && indexed_input_count == record.transparent_input_count
        && response.transaction.vout_count == record.transparent_output_count
        && response.transaction.total_output == record.transparent_output_total_zat.to_string()
        && response.transaction.value_balance_transparent
            == record.transparent_value_balance_zat.to_string()
        && response.transaction.value_balance_sapling
            == record.sapling_value_balance_zat.to_string()
        && response.transaction.value_balance_orchard
            == record.orchard_value_balance_zat.to_string()
        && response.transaction.value_balance_ironwood
            == record.ironwood_value_balance_zat.to_string()
        && joinsplit_count == record.joinsplit_count
        && response.transaction.sapling_spend_count == record.sapling_spend_count
        && response.transaction.sapling_output_count == record.sapling_output_count
        && response.transaction.orchard_actions == record.orchard_action_count
        && response.transaction.ironwood_actions == record.ironwood_action_count;
    if !matches {
        return Err(Error::CorruptData(format!(
            "indexed transaction record does not match canonical transaction {}",
            transaction.hash()
        )));
    }

    Ok(())
}

fn count_u32(value: usize, name: &str) -> Result<u32, Error> {
    u32::try_from(value).map_err(|_| Error::Calculation(format!("{name} exceeds u32")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transaction_status_includes_finality() {
        let confirmed = TransactionChainStatus {
            confirmations: 1,
            finalized: false,
        };
        let finalized = TransactionChainStatus {
            confirmations: 100,
            finalized: true,
        };

        assert_eq!(confirmed.response_status(), TransactionStatus::Confirmed);
        assert_eq!(finalized.response_status(), TransactionStatus::Finalized);
    }
}
