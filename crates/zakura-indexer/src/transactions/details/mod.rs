//! On-demand canonical transaction details assembled directly from state.

mod response;
mod state;

use zakura_chain::{parameters::Network, transaction::Hash as TransactionHash};
use zakura_state::ReadState;

use crate::{models::TransactionRecord, types::TransactionDetails, Error};

pub(crate) use response::build_block_transactions;
use response::{build_transaction_details, TransactionBlockContext};

/// Returns complete explorer transaction details directly from canonical state.
pub async fn transaction_details_from_state<State>(
    read_state: State,
    network: &Network,
    txid: TransactionHash,
) -> Result<Option<TransactionDetails>, Error>
where
    State: ReadState,
{
    let Some(mined) = state::transaction(read_state.clone(), txid).await? else {
        return Ok(None);
    };
    if mined.tx.hash() != txid {
        return Err(Error::StateResponse(
            "state returned a transaction with a different hash".to_string(),
        ));
    }

    let summary = state::transaction_summary(read_state.clone(), txid)
        .await?
        .ok_or_else(|| Error::ExplorerDataUnavailable(format!("transaction summary for {txid}")))?;
    if summary.location.height != mined.height {
        return Err(Error::StateResponse(format!(
            "transaction {txid} summary height does not match its mined height"
        )));
    }
    let context = TransactionBlockContext {
        position: TransactionRecord::from_state(summary.location, summary.record).position,
        block_hash: summary.block_hash,
        block_time: summary.block_time,
    };
    let record = TransactionRecord::from_state(summary.location, summary.record);
    let (status, transparent_inputs, spent_outputs) = tokio::try_join!(
        state::chain_status(read_state.clone(), summary.block_hash, mined.height,),
        state::transparent_inputs(read_state.clone(), &mined.tx),
        state::spent_output_statuses(read_state, &mined.tx),
    )?;

    let network = network.clone();
    tokio::task::spawn_blocking(move || {
        build_transaction_details(
            &network,
            &mined.tx,
            context,
            record,
            status,
            &transparent_inputs,
            &spent_outputs,
        )
    })
    .await
    .map_err(|error| Error::Task(error.to_string()))?
    .map(Some)
}

#[cfg(test)]
mod tests {
    use super::transaction_details_from_state;
    use tower::{service_fn, BoxError};
    use zakura_chain::{
        block::{genesis::regtest_genesis_block, Height},
        parameters::{testnet::RegtestParameters, Network},
        serialization::ZcashSerialize,
    };
    use zakura_state::{
        ExplorerReadRequest, ExplorerReadResponse, ExplorerTransactionRecord,
        ExplorerTransactionSummary, MinedTx, ReadRequest, ReadResponse, TransactionLocation,
    };

    use crate::types::{TransactionKind, TransactionStatus};

    #[tokio::test]
    async fn assembles_canonical_transaction_details_without_an_rpc_call() {
        let network = Network::new_regtest(RegtestParameters::default());
        let block = regtest_genesis_block();
        let transaction = block
            .transactions
            .first()
            .expect("genesis block contains a coinbase transaction")
            .clone();
        let txid = transaction.hash();
        let block_hash = block.hash();
        let block_time = block.header.time;
        let transaction_size = u32::try_from(transaction.zcash_serialized_size())
            .expect("test transaction size fits in u32");

        let read_state = service_fn(move |request: ReadRequest| {
            let transaction = transaction.clone();
            let block_time = block_time;
            async move {
                Ok::<_, BoxError>(match request {
                    ReadRequest::Transaction(requested_txid) => {
                        assert_eq!(requested_txid, txid);
                        ReadResponse::Transaction(Some(MinedTx::new(
                            transaction,
                            Height(0),
                            1,
                            block_time,
                        )))
                    }
                    ReadRequest::Explorer(ExplorerReadRequest::TransactionSummary(
                        requested_txid,
                    )) => {
                        assert_eq!(requested_txid, txid);
                        ReadResponse::Explorer(ExplorerReadResponse::TransactionSummary(Some(
                            ExplorerTransactionSummary {
                                location: TransactionLocation::from_usize(Height(0), 0),
                                txid,
                                block_hash,
                                block_time: block_time.timestamp(),
                                record: ExplorerTransactionRecord {
                                    serialized_size: transaction_size,
                                    fee_zat: 0,
                                    transparent_value_balance_zat: 0,
                                    sapling_value_balance_zat: 0,
                                    orchard_value_balance_zat: 0,
                                    ironwood_value_balance_zat: 0,
                                    transparent_input_count: 0,
                                    transparent_output_count: 1,
                                    joinsplit_count: 0,
                                    sapling_spend_count: 0,
                                    sapling_output_count: 0,
                                    orchard_action_count: 0,
                                    ironwood_action_count: 0,
                                },
                            },
                        )))
                    }
                    ReadRequest::Explorer(ExplorerReadRequest::TransparentOutputs(outpoints)) => {
                        assert!(outpoints.is_empty());
                        ReadResponse::Explorer(ExplorerReadResponse::TransparentOutputs(Vec::new()))
                    }
                    ReadRequest::Depth(requested_hash) => {
                        assert_eq!(requested_hash, block_hash);
                        ReadResponse::Depth(Some(0))
                    }
                    ReadRequest::FinalizedTip => {
                        ReadResponse::FinalizedTip(Some((Height(0), block_hash)))
                    }
                    ReadRequest::Explorer(ExplorerReadRequest::TransparentOutputSpends(
                        outpoints,
                    )) => ReadResponse::Explorer(ExplorerReadResponse::TransparentOutputSpends(
                        vec![false; outpoints.len()],
                    )),
                    request => panic!("unexpected test state request: {request:?}"),
                })
            }
        });

        let details = transaction_details_from_state(read_state, &network, txid)
            .await
            .expect("transaction details should be assembled")
            .expect("genesis transaction should exist");

        assert_eq!(details.transaction.transaction.txid, txid.to_string());
        assert_eq!(details.transaction.block_hash, block_hash.to_string());
        assert_eq!(details.transaction.transaction_index, 0);
        assert_eq!(details.status, TransactionStatus::Finalized);
        assert_eq!(details.kind, TransactionKind::Coinbase);
        assert_eq!(details.confirmations, 1);
        assert!(details.coinbase_hex.is_some());

        let json = serde_json::to_value(details).expect("transaction details serialize as JSON");
        assert_eq!(json["txid"], txid.to_string());
        assert_eq!(json["status"], "finalized");
        assert_eq!(json["tx_index"], 0);
        assert!(json.get("transaction_index").is_none());
        assert!(json.get("flow_amount_zat").is_some());
        assert!(json.get("amount_zat").is_none());
        assert!(json.get("canonical").is_none());
        assert!(json.get("finalized").is_none());
        assert!(json.get("finality_status").is_none());
    }
}
