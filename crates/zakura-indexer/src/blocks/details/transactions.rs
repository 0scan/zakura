//! Explorer transaction summaries derived from canonical block data.

use std::collections::HashMap;

use zakura_chain::{
    block::{Block, Hash, Height},
    parameters::Network,
    serialization::ZcashSerialize,
    transparent::{Input, OutPoint, Utxo},
};

use crate::{
    types::{BlockTransaction, BlockTransactionInput, BlockTransactionOutput},
    Error, Indexer,
};

pub(super) fn build(
    indexer: &Indexer,
    network: &Network,
    block: &Block,
    height: Height,
    hash: Hash,
    spent_outputs: &HashMap<OutPoint, bool>,
) -> Result<Vec<BlockTransaction>, Error> {
    block
        .transactions
        .iter()
        .enumerate()
        .map(|(transaction_index, transaction)| {
            let is_coinbase = transaction_index == 0;
            let txid = transaction.hash();
            let spent_utxos = transaction
                .inputs()
                .iter()
                .filter_map(Input::outpoint)
                .map(|outpoint| {
                    let utxo = indexer.transparent_output(outpoint)?.ok_or_else(|| {
                        Error::CorruptData(format!(
                            "missing indexed transparent output for {outpoint:?}"
                        ))
                    })?;
                    Ok((outpoint, utxo))
                })
                .collect::<Result<HashMap<OutPoint, Utxo>, Error>>()?;

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
                    let utxo = spent_utxos.get(outpoint).ok_or_else(|| {
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
                    let output_index = u32::try_from(output_index).map_err(|_| {
                        Error::Calculation("transaction output index exceeds u32".to_string())
                    })?;
                    let outpoint = OutPoint {
                        hash: txid,
                        index: output_index,
                    };
                    Ok(BlockTransactionOutput {
                        transaction_id: txid.to_string(),
                        address: output.address(network).map(|address| address.to_string()),
                        value: output.value().zatoshis().to_string(),
                        output_index,
                        script_pub_key: hex::encode(output.lock_script.as_raw_bytes()),
                        spent: spent_outputs.get(&outpoint).copied().unwrap_or(false),
                    })
                })
                .collect::<Result<Vec<_>, Error>>()?;

            let total_input = spent_utxos.values().try_fold(0_i64, |total, utxo| {
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
            let fee = if is_coinbase {
                0
            } else {
                transaction
                    .value_balance(&spent_utxos)
                    .map_err(|error| Error::Calculation(error.to_string()))?
                    .remaining_transaction_value()
                    .map_err(|error| Error::Calculation(error.to_string()))?
                    .zatoshis()
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
                total.checked_add(value).ok_or_else(|| {
                    Error::Calculation("transaction value balance exceeds i64".to_string())
                })
            })?;
            let serialized = transaction
                .zcash_serialize_to_vec()
                .map_err(|error| Error::Calculation(error.to_string()))?;

            Ok(BlockTransaction {
                txid: txid.to_string(),
                hex: hex::encode(&serialized),
                block_height: height.0.to_string(),
                block_hash: hash.to_string(),
                block_time: block.header.time.timestamp().to_string(),
                size: u32::try_from(serialized.len()).map_err(|_| {
                    Error::Calculation("serialized transaction size exceeds u32".to_string())
                })?,
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
                vin_count: count_u32(transaction.inputs().len(), "transparent input count")?,
                vout_count: count_u32(transaction.outputs().len(), "transparent output count")?,
                value_balance: value_balance.to_string(),
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
                transaction_index: count_u32(transaction_index, "transaction index")?,
                inputs,
                outputs,
            })
        })
        .collect()
}

fn count_u32(value: usize, name: &str) -> Result<u32, Error> {
    u32::try_from(value).map_err(|_| Error::Calculation(format!("{name} exceeds u32")))
}
