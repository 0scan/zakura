//! Best-effort attribution of a coinbase transaction to a miner and pool.

use zakura_chain::{parameters::Network, transaction::Transaction};

/// Stable presentation metadata for a known mining pool.
pub(super) struct PoolMetadata {
    pub(super) url: Option<&'static str>,
    pub(super) region: Option<&'static str>,
}

/// Returns a best-effort miner payout address and pool attribution.
pub(super) fn identify_miner(
    coinbase: &Transaction,
    network: &Network,
) -> (Option<String>, String) {
    let addresses = coinbase
        .outputs()
        .iter()
        .filter_map(|output| {
            output
                .address(network)
                .map(|address| (output.value().zatoshis(), address.to_string()))
        })
        .collect::<Vec<_>>();

    if let Some((address, pool)) = addresses
        .iter()
        .find_map(|(_, address)| pool_from_address(address).map(|pool| (address.clone(), pool)))
    {
        return (Some(address), pool.to_string());
    }

    let miner_address = addresses
        .into_iter()
        .max_by_key(|(value, _)| *value)
        .map(|(_, address)| address);

    let miner_data = coinbase
        .inputs()
        .first()
        .and_then(|input| input.miner_data())
        .map(Vec::as_slice)
        .unwrap_or_default();
    let miner_pool = pool_from_miner_data(miner_data).unwrap_or("Unknown");

    (miner_address, miner_pool.to_string())
}

fn pool_from_address(address: &str) -> Option<&'static str> {
    match address {
        "t1XQZdZMnzXBcL8yx2PR27dSNrqctgwLgux" => Some("Luxor"),
        "t1MKn34KBa8Xh4g8qU8psibBXvURafphVn7" => Some("ViaBTC"),
        "t1at7nVNsv6taLRrNRvnQdtfLNRDfsGc3Ak" => Some("ViaBTC"),
        "t1SEgZvXCu3ceE42qrq5pCeSq7HbLjX8NJv" => Some("ViaBTC-Solo"),
        "t1PEp2GJLSdhDfCKqc2J211WKDUS1NfoQNy" => Some("F2Pool"),
        "t1L2b66MXbgpVMXDfUa94GCBFAN4dCxGohM" => Some("AntPool"),
        "t1ZVi2YGk98tEGYcNpXYnJFWCoLG2oYwv3J" => Some("AntPool"),
        "t1SqwRAAdSig6dE4EBPLonAait219VmkUjP" => Some("Foundry USA"),
        "t1e6hceYHkzCbwcwGZzKeMfXXW7x7gr19Cw" => Some("Kryptex"),
        "t1Mofe2EigYNfgqSTPbK4k1iJTxyCEEQCEC" => Some("Kryptex"),
        "t1VTjv7XF3hYqxQkxKmHHErvus3bDrbbkGg"
        | "t1QxTHUputbmZRxd3EqP671sLqd6KNBQbXJ"
        | "t1fu6KgYtHEXk2ZhTpM1XD7jbnSmW6wokDM"
        | "t1bnxtY7aLCjWx9Ru1YcGwRWch3eEWUFK7u" => Some("2Miners"),
        "t1eBv4a3wBhVaFgWYjXrFYTU7pruCWaBpLW" => Some("NiceHash"),
        "t1Uo7EN1A3GN29UjQJbUFYvrhxQd6Gt7qdA" => Some("ZEC Mining Pool"),
        "t1egMFNkP7EfkK25y8s4GeiMkEGnqcMnTb1" => Some("Mining Dutch"),
        "t1Na7ykQ6vE4CbxBPuUDUQx5n6aEWXu1VQq" => Some("Binance Pool"),
        "t1Yw8NGbPDs7fgpxzJ8gzCgurAQ8GFQBkk2" => Some("MySoloPool"),
        "t1K79TgQbqu74d6rBmsMu2oFEXEwAmdYiT7" => Some("Unidentified #5"),
        "t1fpcZ2Dbwn4oj35oWBTUhtmUciSq7HG7LU" => Some("Private Miner B"),
        _ => None,
    }
}

fn pool_from_miner_data(data: &[u8]) -> Option<&'static str> {
    let text = String::from_utf8_lossy(data).to_ascii_lowercase();

    [
        ("viabtc", "ViaBTC"),
        ("luxor", "Luxor"),
        ("f2pool", "F2Pool"),
        ("antpool", "AntPool"),
        ("foundry", "Foundry USA"),
        ("kryptex", "Kryptex"),
        ("poolin", "Poolin"),
        ("binance", "Binance Pool"),
        ("nicehash", "NiceHash"),
        ("2miners", "2Miners"),
        ("slush", "Braiins Pool"),
        ("mysolopool", "MySoloPool"),
        ("sluicey", "Sluicey Pool"),
    ]
    .into_iter()
    .find_map(|(tag, pool)| text.contains(tag).then_some(pool))
}

/// Returns non-consensus presentation metadata for a known pool label.
pub(super) fn pool_metadata(pool: &str) -> PoolMetadata {
    let (url, region) = match pool {
        "ViaBTC" | "ViaBTC-Solo" => (Some("https://www.viabtc.com"), Some("US/CN")),
        "F2Pool" => (Some("https://f2pool.com"), Some("HK")),
        "Foundry USA" => (Some("https://foundrydigital.com"), Some("US")),
        "Luxor" => (Some("https://luxor.tech"), Some("US")),
        "2Miners" => (Some("https://2miners.com"), Some("EU")),
        "NiceHash" => (Some("https://www.nicehash.com"), None),
        "AntPool" => (Some("https://www.antpool.com"), Some("JP")),
        "Kryptex" => (Some("https://www.kryptex.com"), Some("EU")),
        "ZEC Mining Pool" => (Some("https://zecminingpool.com"), None),
        "Mining Dutch" => (Some("https://www.mining-dutch.nl"), Some("EU")),
        "Binance Pool" => (Some("https://pool.binance.com"), None),
        "MySoloPool" => (Some("https://zcash.mysolopool.com"), None),
        "Sluicey Pool" => (Some("https://sluicey.xyz/"), None),
        "Braiins Pool" => (Some("https://braiins.com/pool"), None),
        _ => (None, None),
    };

    PoolMetadata { url, region }
}

/// Returns true when an address is a known funding-stream recipient, not a miner.
pub(super) fn is_funding_stream_address(address: Option<&str>) -> bool {
    matches!(
        address,
        Some("t3cFfPt1Bcvgez9ZbMBFWeZsskxTkPzGCow") | Some("t2HifwjUj9uyxr9bknR8LFuQbc98c3vkXtu")
    )
}

#[cfg(test)]
mod tests {
    use super::{is_funding_stream_address, pool_from_address, pool_from_miner_data};

    #[test]
    fn identifies_known_pool_addresses() {
        assert_eq!(
            pool_from_address("t1PEp2GJLSdhDfCKqc2J211WKDUS1NfoQNy"),
            Some("F2Pool")
        );
        assert_eq!(pool_from_address("unknown"), None);
    }

    #[test]
    fn identifies_case_insensitive_coinbase_tags() {
        assert_eq!(pool_from_miner_data(b"/ViaBTC/Mined"), Some("ViaBTC"));
        assert_eq!(pool_from_miner_data(b"no known tag"), None);
    }

    #[test]
    fn does_not_treat_funding_stream_recipients_as_miners() {
        assert!(is_funding_stream_address(Some(
            "t3cFfPt1Bcvgez9ZbMBFWeZsskxTkPzGCow"
        )));
        assert!(!is_funding_stream_address(Some(
            "t1MKn34KBa8Xh4g8qU8psibBXvURafphVn7"
        )));
    }
}
