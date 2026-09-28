//! Conversion of canonical state and indexed summaries into the REST contract.

use std::sync::Arc;

use hex::ToHex;
use zakura_chain::{
    block::{Block, Commitment, Height},
    parameters::{Network, NetworkUpgrade},
    serialization::BytesInDisplayOrder,
};

use super::{state::Details as StateDetails, value_pools};
use crate::{
    blocks::miner_attribution::{is_funding_stream_address, pool_metadata, PoolMetadata},
    types::{BlockDetails, BlockRecord, BlockTransaction, BlockTrees, TreeSize},
    Error,
};

pub(super) fn assemble(
    network: &Network,
    block: Arc<Block>,
    summary: BlockRecord,
    height: Height,
    state: StateDetails,
    transactions: Vec<BlockTransaction>,
) -> Result<BlockDetails, Error> {
    let sapling_active = NetworkUpgrade::Sapling
        .activation_height(network)
        .is_some_and(|activation_height| height >= activation_height);
    let sapling_root = if sapling_active {
        state.sapling_tree.root().bytes_in_display_order()
    } else {
        [0; 32]
    };
    let orchard_active = NetworkUpgrade::Nu5
        .activation_height(network)
        .is_some_and(|activation_height| height >= activation_height);
    let orchard_root =
        orchard_active.then(|| hex::encode(state.orchard_tree.root().bytes_in_display_order()));
    let ironwood_root = state
        .ironwood_tree
        .as_ref()
        .map(|tree| hex::encode(tree.root().bytes_in_display_order()));

    let block_commitments = match block
        .header
        .commitment(network, height)
        .map_err(|error| Error::Calculation(error.to_string()))?
    {
        Commitment::PreSaplingReserved(bytes) => bytes,
        Commitment::FinalSaplingRoot(_) => sapling_root,
        Commitment::ChainHistoryActivationReserved => [0; 32],
        Commitment::ChainHistoryRoot(root) => root.bytes_in_display_order(),
        Commitment::ChainHistoryBlockTxAuthCommitment(hash) => hash.bytes_in_display_order(),
    };

    let (chain_supply, value_pools) = match state.current_block_info {
        Some(current) => {
            let previous = state.previous_block_info.map(|info| *info.value_pools());
            let (supply, pools) = value_pools::responses(*current.value_pools(), previous)?;
            (Some(supply), pools)
        }
        None => (None, Vec::new()),
    };

    let coinbase_script = block
        .transactions
        .first()
        .and_then(|transaction| transaction.inputs().first())
        .and_then(|input| input.coinbase_script());
    let coinbase_hex = coinbase_script.as_deref().map(hex::encode);
    let coinbase_text = coinbase_script.as_deref().map(decode_coinbase_text);
    let PoolMetadata {
        url: miner_pool_url,
        region: miner_pool_region,
    } = pool_metadata(&summary.miner_pool);
    let miner_pool_is_funding_stream = is_funding_stream_address(summary.miner_address.as_deref());
    let mut nonce = *block.header.nonce;
    nonce.reverse();

    Ok(BlockDetails {
        summary,
        confirmations: state.confirmations,
        canonical: true,
        finalized: state.finalized,
        finality_status: if state.finalized {
            "Finalized".to_string()
        } else {
            "NotYetFinalized".to_string()
        },
        is_orphaned: false,
        version: block.header.version,
        merkle_root: block.header.merkle_root.encode_hex(),
        block_commitments: hex::encode(block_commitments),
        final_sapling_root: hex::encode(sapling_root),
        final_orchard_root: orchard_root,
        final_ironwood_root: ironwood_root,
        bits: block.header.difficulty_threshold.to_string(),
        nonce: hex::encode(nonce),
        solution: block.header.solution.encode_hex(),
        previous_block_hash: block.header.previous_block_hash.to_string(),
        next_block_hash: state.next_block_hash.map(|hash| hash.to_string()),
        chain_supply,
        value_pools,
        trees: BlockTrees {
            sapling: TreeSize {
                size: state.sapling_tree.count(),
            },
            orchard: TreeSize {
                size: state.orchard_tree.count(),
            },
            ironwood: state
                .ironwood_tree
                .map(|tree| TreeSize { size: tree.count() }),
        },
        miner_pool_url: miner_pool_url.map(str::to_string),
        miner_pool_region: miner_pool_region.map(str::to_string),
        miner_pool_is_funding_stream,
        coinbase_hex,
        coinbase_text,
        transactions,
    })
}

fn decode_coinbase_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .chars()
        .map(|character| {
            if character.is_control() || character == '\u{fffd}' {
                '.'
            } else {
                character
            }
        })
        .collect()
}
