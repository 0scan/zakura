//! On-demand canonical transaction details assembled from state and indexed data.

mod response;
mod state;

use zakura_chain::{block::Height, transaction::Hash as TransactionHash};
use zakura_state::ReadState;

use crate::{models::TransactionRecord, types::TransactionDetails, Error, Indexer};

pub(crate) use response::build_block_transactions;
use response::{build_transaction_details, TransactionBlockContext};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct IndexedTransactionContext {
    record: TransactionRecord,
    block: TransactionBlockContext,
}

impl Indexer {
    /// Returns complete explorer details for one canonical transaction.
    ///
    /// Consensus transaction bytes and current chain status are read directly
    /// from the in-process state service. Filter classifications, fees, and
    /// historical transparent inputs come from the rebuildable explorer index.
    pub async fn transaction_details<State>(
        &self,
        read_state: State,
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

        let initial_context = self.load_indexed_transaction_context(
            txid,
            mined.height,
            mined.block_time.timestamp(),
        )?;
        let (status, spent_outputs) = tokio::try_join!(
            state::chain_status(
                read_state.clone(),
                initial_context.block.block_hash,
                mined.height,
            ),
            state::spent_output_statuses(read_state, &mined.tx),
        )?;

        let final_context = self.load_indexed_transaction_context(
            txid,
            mined.height,
            mined.block_time.timestamp(),
        )?;
        if final_context != initial_context {
            return Err(Error::TransactionNotIndexed(format!(
                "canonical position for {txid} changed while details were being assembled"
            )));
        }

        let indexer = self.clone();
        let network = self.network.clone();
        tokio::task::spawn_blocking(move || {
            build_transaction_details(
                &indexer,
                &network,
                &mined.tx,
                final_context.block,
                final_context.record,
                status,
                &spent_outputs,
            )
        })
        .await
        .map_err(|error| Error::Task(error.to_string()))?
        .map(Some)
    }

    fn load_indexed_transaction_context(
        &self,
        txid: TransactionHash,
        state_height: Height,
        state_block_time: i64,
    ) -> Result<IndexedTransactionContext, Error> {
        let record = self.transaction_record(txid)?.ok_or_else(|| {
            Error::TransactionNotIndexed(format!("transaction record for {txid}"))
        })?;
        if record.position.height != state_height {
            return Err(Error::TransactionNotIndexed(format!(
                "transaction {txid} moved from indexed height {} to state height {}",
                record.position.height.0, state_height.0
            )));
        }
        if self.canonical_transaction_hash(record.position)? != Some(txid) {
            return Err(Error::TransactionNotIndexed(format!(
                "canonical position for transaction {txid}"
            )));
        }

        let block_hash = self.canonical_block_hash(state_height)?.ok_or_else(|| {
            Error::TransactionNotIndexed(format!(
                "canonical block at transaction height {}",
                state_height.0
            ))
        })?;
        let block = self.block_record(block_hash)?.ok_or_else(|| {
            Error::TransactionNotIndexed(format!(
                "block record for transaction {txid} in {block_hash}"
            ))
        })?;
        if block.timestamp != state_block_time.to_string() {
            return Err(Error::TransactionNotIndexed(format!(
                "block timestamp for transaction {txid} does not match canonical state"
            )));
        }

        Ok(IndexedTransactionContext {
            record,
            block: TransactionBlockContext {
                position: record.position,
                block_hash,
                block_time: state_block_time,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;
    use tower::{service_fn, BoxError};
    use zakura_chain::{
        block::{genesis::regtest_genesis_block, Height},
        parameters::{testnet::RegtestParameters, Network},
        serialization::ZcashSerialize,
    };
    use zakura_state::{MinedTx, ReadRequest, ReadResponse};

    use crate::{
        types::{TransactionKind, TransactionStatus},
        Indexer,
    };

    #[tokio::test]
    async fn assembles_canonical_transaction_details_without_an_rpc_call() {
        let directory = TempDir::new().expect("temporary index directory should be created");
        let network = Network::new_regtest(RegtestParameters::default());
        let indexer =
            Indexer::open(directory.path(), network).expect("temporary index should open");
        let block = regtest_genesis_block();
        let transaction = block
            .transactions
            .first()
            .expect("genesis block contains a coinbase transaction")
            .clone();
        let txid = transaction.hash();
        let block_hash = block.hash();
        let block_time = block.header.time;
        let serialized_size = block.zcash_serialized_size();
        indexer
            .index_blocks(vec![(
                Height(0),
                block.clone(),
                serialized_size,
                Default::default(),
            )])
            .expect("valid genesis block should be indexed");

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
                    ReadRequest::Depth(requested_hash) => {
                        assert_eq!(requested_hash, block_hash);
                        ReadResponse::Depth(Some(0))
                    }
                    ReadRequest::FinalizedTip => {
                        ReadResponse::FinalizedTip(Some((Height(0), block_hash)))
                    }
                    ReadRequest::IsTransparentOutputSpent(_) => {
                        ReadResponse::IsTransparentOutputSpent(false)
                    }
                    request => panic!("unexpected test state request: {request:?}"),
                })
            }
        });

        let details = indexer
            .transaction_details(read_state, txid)
            .await
            .expect("transaction details should be assembled")
            .expect("genesis transaction should exist");

        assert_eq!(details.transaction.transaction.txid, txid.to_string());
        assert_eq!(details.transaction.block_hash, block_hash.to_string());
        assert_eq!(details.transaction.transaction_index, 0);
        assert_eq!(details.status, TransactionStatus::Confirmed);
        assert_eq!(details.kind, TransactionKind::Coinbase);
        assert_eq!(details.confirmations, 1);
        assert!(details.canonical);
        assert!(details.finalized);
        assert!(details.coinbase_hex.is_some());

        let json = serde_json::to_value(details).expect("transaction details serialize as JSON");
        assert_eq!(json["txid"], txid.to_string());
        assert_eq!(json["tx_index"], 0);
        assert!(json.get("transaction_index").is_none());
        assert!(json.get("finality_status").is_none());
    }
}
