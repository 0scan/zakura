//! Best-effort attribution of a coinbase transaction to a miner and pool.

use zakura_chain::{parameters::Network, transaction::Transaction};

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
        "t1SEgZvXCu3ceE42qrq5pCeSq7HbLjX8NJv" => Some("ViaBTC-Solo"),
        "t1PEp2GJLSdhDfCKqc2J211WKDUS1NfoQNy" => Some("F2Pool"),
        "t1L2b66MXbgpVMXDfUa94GCBFAN4dCxGohM" => Some("AntPool"),
        "t1SqwRAAdSig6dE4EBPLonAait219VmkUjP" => Some("Foundry USA"),
        "t1e6hceYHkzCbwcwGZzKeMfXXW7x7gr19Cw" => Some("Kryptex"),
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
    ]
    .into_iter()
    .find_map(|(tag, pool)| text.contains(tag).then_some(pool))
}

#[cfg(test)]
mod tests {
    use super::{pool_from_address, pool_from_miner_data};

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
}
