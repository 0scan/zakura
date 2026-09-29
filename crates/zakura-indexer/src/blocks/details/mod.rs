//! On-demand canonical block details assembled from state and indexed data.

mod response;
mod state;
mod value_pools;

use zakura_chain::block::{Block, Hash, Height};
use zakura_state::{HashOrHeight, ReadRequest, ReadResponse, ReadState};

use crate::{
    transactions::build_block_transactions,
    types::{BlockDetails, BlockRecord},
    Error, Indexer,
};

impl Indexer {
    /// Returns complete explorer details for one canonical block.
    ///
    /// Consensus data is read directly from the in-process state service. Only
    /// explorer-specific summaries and historical transparent outputs come
    /// from the indexer's rebuildable RocksDB.
    pub async fn block_details<State>(
        &self,
        read_state: State,
        identifier: HashOrHeight,
    ) -> Result<Option<BlockDetails>, Error>
    where
        State: ReadState,
    {
        let response =
            state::call(read_state.clone(), ReadRequest::BlockAndSize(identifier)).await?;
        let ReadResponse::BlockAndSize(block_and_size) = response else {
            return Err(state::unexpected_response("BlockAndSize"));
        };
        let Some((block, serialized_size)) = block_and_size else {
            return Ok(None);
        };

        let hash = block.hash();
        let height = block.coinbase_height().ok_or_else(|| {
            Error::StateResponse("canonical block is missing its coinbase height".to_string())
        })?;
        let mut state_details = state::load(
            read_state.clone(),
            &self.network,
            hash,
            height,
            block.header.previous_block_hash,
        )
        .await?;
        let spent_outputs = state::load_spent_output_statuses(read_state.clone(), &block).await?;
        state_details.confirmations = state::confirmations(read_state, hash).await?;

        let indexer = self.clone();
        let network = self.network.clone();
        let block_for_index = block.clone();
        let (summary, block_transactions) = tokio::task::spawn_blocking(move || {
            let summary =
                indexer.indexed_summary(hash, height, serialized_size, &block_for_index)?;
            let block_transactions = build_block_transactions(
                &indexer,
                &network,
                &block_for_index,
                height,
                hash,
                &spent_outputs,
            )?;
            Ok::<_, Error>((summary, block_transactions))
        })
        .await
        .map_err(|error| Error::Task(error.to_string()))??;

        Ok(Some(response::assemble(
            &self.network,
            block,
            summary,
            height,
            state_details,
            block_transactions,
        )?))
    }

    fn indexed_summary(
        &self,
        hash: Hash,
        height: Height,
        serialized_size: usize,
        block: &Block,
    ) -> Result<BlockRecord, Error> {
        if self.canonical_block_hash(height)? != Some(hash) {
            return Err(Error::BlockNotIndexed(format!(
                "canonical block {hash} at height {}",
                height.0
            )));
        }

        let summary = self.block_record(hash)?.ok_or_else(|| {
            Error::BlockNotIndexed(format!("block record for canonical block {hash}"))
        })?;
        let expected_size = u32::try_from(serialized_size)
            .map_err(|_| Error::Calculation("serialized block size exceeds u32".to_string()))?;
        let expected_transaction_count = u32::try_from(block.transactions.len())
            .map_err(|_| Error::Calculation("block transaction count exceeds u32".to_string()))?;
        if summary.height != height.0.to_string()
            || summary.hash != hash.to_string()
            || summary.size != expected_size
            || summary.transaction_count != expected_transaction_count
        {
            return Err(Error::CorruptData(format!(
                "indexed summary does not match canonical block {hash}"
            )));
        }

        Ok(summary)
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;
    use tower::{service_fn, BoxError};
    use zakura_chain::{
        block::{genesis::regtest_genesis_block, Height},
        block_info::BlockInfo,
        parameters::{testnet::RegtestParameters, Network},
        serialization::ZcashSerialize,
    };
    use zakura_state::{ReadRequest, ReadResponse};

    use crate::Indexer;

    #[tokio::test]
    async fn assembles_rpc_and_explorer_fields_without_an_rpc_call() {
        let directory = TempDir::new().expect("temporary index directory should be created");
        let network = Network::new_regtest(RegtestParameters::default());
        let indexer =
            Indexer::open(directory.path(), network).expect("temporary index should open");
        let block = regtest_genesis_block();
        let hash = block.hash();
        let serialized_size = block.zcash_serialized_size();
        indexer
            .index_blocks(vec![(Height(0), block.clone(), serialized_size)])
            .expect("valid genesis block should be indexed");

        let state_block = block.clone();
        let read_state = service_fn(move |request: ReadRequest| {
            let block = state_block.clone();
            async move {
                Ok::<_, BoxError>(match request {
                    ReadRequest::BlockAndSize(_) => {
                        ReadResponse::BlockAndSize(Some((block.clone(), serialized_size)))
                    }
                    ReadRequest::BlockHeader(_) => ReadResponse::BlockHeader {
                        header: block.header.clone(),
                        hash: block.hash(),
                        height: Height(0),
                        next_block_hash: None,
                    },
                    ReadRequest::Depth(_) => ReadResponse::Depth(Some(0)),
                    ReadRequest::FinalizedTip => {
                        ReadResponse::FinalizedTip(Some((Height(0), block.hash())))
                    }
                    ReadRequest::SaplingTree(_) => {
                        ReadResponse::SaplingTree(Some(Default::default()))
                    }
                    ReadRequest::OrchardTree(_) => {
                        ReadResponse::OrchardTree(Some(Default::default()))
                    }
                    ReadRequest::BlockInfo(_) => {
                        ReadResponse::BlockInfo(Some(BlockInfo::default()))
                    }
                    ReadRequest::IsTransparentOutputSpent(_) => {
                        ReadResponse::IsTransparentOutputSpent(false)
                    }
                    _ => panic!("unexpected test state request"),
                })
            }
        });

        let details = indexer
            .block_details(read_state, Height(0).into())
            .await
            .expect("block details should be assembled")
            .expect("genesis block should exist");

        assert_eq!(details.summary.hash, hash.to_string());
        assert_eq!(
            details.summary.size,
            u32::try_from(serialized_size).expect("test block size fits in u32")
        );
        assert_eq!(details.confirmations, 1);
        assert!(details.canonical);
        assert!(details.finalized);
        assert_eq!(details.finality_status, "Finalized");
        assert_eq!(details.transactions.len(), block.transactions.len());
        assert!(details.transactions[0].is_coinbase);
        assert!(details.coinbase_hex.is_some());
        assert!(details.chain_supply.is_some());
        assert_eq!(details.value_pools.len(), 6);

        let json = serde_json::to_value(details).expect("block details serialize as JSON");
        assert!(json["transactions"][0].get("tx_index").is_some());
        assert!(json["transactions"][0].get("transaction_index").is_none());
    }
}
