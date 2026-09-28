//! Public indexer handle shared by every indexed domain.

use std::path::Path;

use zakura_chain::parameters::Network;

use crate::{database::IndexerDatabase, Error};

/// Cloneable handle to Zakura's rebuildable explorer indexes.
#[derive(Clone)]
pub struct Indexer {
    pub(crate) database: IndexerDatabase,
    pub(crate) network: Network,
}

impl Indexer {
    /// Opens or creates a persistent indexer database at `path`.
    pub fn open(path: impl AsRef<Path>, network: Network) -> Result<Self, Error> {
        Ok(Self {
            database: IndexerDatabase::open(path)?,
            network,
        })
    }

    /// Opens an indexer database that is deleted after its final handle closes.
    pub fn open_ephemeral(network: Network) -> Result<Self, Error> {
        let prefix = format!("zakura-indexer-{}-", network.lowercase_name());
        Ok(Self {
            database: IndexerDatabase::open_ephemeral(&prefix)?,
            network,
        })
    }
}
