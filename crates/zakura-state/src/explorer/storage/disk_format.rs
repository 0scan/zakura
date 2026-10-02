//! Compact explorer transaction index encodings.

#![allow(missing_docs)]

use bincode::Options;
use serde::{de::DeserializeOwned, Serialize};
use zakura_chain::{
    parameters::NetworkKind,
    transaction::TransactionValueEndpoint,
    transparent::{self, Address},
};

use crate::service::finalized_state::{
    FromDisk, IntoDisk, TransactionLocation, TRANSACTION_LOCATION_DISK_BYTES,
};
use crate::{ExplorerBlockStats, ExplorerChainStats, ExplorerDailyStats};

/// Independently versioned explorer schema marker.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ExplorerSchemaVersion(pub(crate) u32);

impl ExplorerSchemaVersion {
    pub(crate) const CURRENT: Self = Self(2);
}

impl IntoDisk for ExplorerSchemaVersion {
    type Bytes = [u8; 4];

    fn as_bytes(&self) -> Self::Bytes {
        self.0.to_be_bytes()
    }
}

impl FromDisk for ExplorerSchemaVersion {
    fn from_bytes(bytes: impl AsRef<[u8]>) -> Self {
        Self(u32::from_be_bytes(
            bytes
                .as_ref()
                .try_into()
                .expect("explorer schema versions are four bytes"),
        ))
    }
}

/// Balance-first key. Inverting the balance makes the RocksDB forward order richest first.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ExplorerBalanceKey([u8; 29]);

/// UTC day number encoded in chronological key order.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ExplorerDayKey(pub u32);

impl IntoDisk for ExplorerDayKey {
    type Bytes = [u8; 4];

    fn as_bytes(&self) -> Self::Bytes {
        self.0.to_be_bytes()
    }
}

impl FromDisk for ExplorerDayKey {
    fn from_bytes(bytes: impl AsRef<[u8]>) -> Self {
        Self(u32::from_be_bytes(
            bytes
                .as_ref()
                .try_into()
                .expect("explorer day database keys are four bytes"),
        ))
    }
}

impl ExplorerBalanceKey {
    pub fn new(address: Address, balance_zat: u64) -> Self {
        let mut bytes = [0; 29];
        bytes[..8].copy_from_slice(&(u64::MAX - balance_zat).to_be_bytes());
        bytes[8..].copy_from_slice(&address.as_bytes());
        Self(bytes)
    }

    pub fn address(self) -> Address {
        address_from_disk_bytes(&self.0[8..])
    }

    pub fn balance_zat(self) -> u64 {
        u64::MAX
            - u64::from_be_bytes(
                self.0[..8]
                    .try_into()
                    .expect("explorer balance key starts with eight balance bytes"),
            )
    }
}

impl IntoDisk for ExplorerBalanceKey {
    type Bytes = [u8; 29];

    fn as_bytes(&self) -> Self::Bytes {
        self.0
    }
}

impl FromDisk for ExplorerBalanceKey {
    fn from_bytes(bytes: impl AsRef<[u8]>) -> Self {
        Self(
            bytes
                .as_ref()
                .try_into()
                .expect("explorer balance database keys are 29 bytes"),
        )
    }
}

fn address_from_disk_bytes(bytes: &[u8]) -> Address {
    let tag = *bytes
        .first()
        .expect("transparent address database key has a variant byte");
    let hash = bytes[1..]
        .try_into()
        .expect("transparent address database key has a 20-byte hash");
    let network = if matches!(tag, 0 | 1 | 4) {
        NetworkKind::Mainnet
    } else {
        NetworkKind::Testnet
    };
    match tag {
        0 | 2 => Address::from_pub_key_hash(network, hash),
        1 | 3 => Address::from_script_hash(network, hash),
        4 | 5 => transparent::Address::Tex {
            network_kind: network,
            validating_key_hash: hash,
        },
        _ => panic!("invalid transparent address database tag {tag}"),
    }
}

fn encode_analytics<T: Serialize>(value: &T) -> Vec<u8> {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .serialize(value)
        .expect("explorer analytics values have a stable bincode encoding")
}

fn decode_analytics<T: DeserializeOwned>(bytes: impl AsRef<[u8]>) -> T {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .reject_trailing_bytes()
        .deserialize(bytes.as_ref())
        .expect("explorer analytics database values match the state format version")
}

macro_rules! impl_analytics_disk_value {
    ($type:ty) => {
        impl IntoDisk for $type {
            type Bytes = Vec<u8>;

            fn as_bytes(&self) -> Self::Bytes {
                encode_analytics(self)
            }
        }

        impl FromDisk for $type {
            fn from_bytes(bytes: impl AsRef<[u8]>) -> Self {
                decode_analytics(bytes)
            }
        }
    };
}

impl_analytics_disk_value!(ExplorerBlockStats);
impl_analytics_disk_value!(ExplorerChainStats);
impl_analytics_disk_value!(ExplorerDailyStats);

/// Opaque fixed-width transparent-address key used by explorer metadata.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct ExplorerAddressKey([u8; 21]);

impl From<zakura_chain::transparent::Address> for ExplorerAddressKey {
    fn from(address: zakura_chain::transparent::Address) -> Self {
        Self(address.as_bytes())
    }
}

impl IntoDisk for ExplorerAddressKey {
    type Bytes = [u8; 21];

    fn as_bytes(&self) -> Self::Bytes {
        self.0
    }
}

impl FromDisk for ExplorerAddressKey {
    fn from_bytes(bytes: impl AsRef<[u8]>) -> Self {
        Self(
            bytes
                .as_ref()
                .try_into()
                .expect("transparent address database keys are 21 bytes"),
        )
    }
}

/// The fixed-width transaction facts needed by explorer list and filter queries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExplorerTransactionRecord {
    pub serialized_size: u32,
    pub fee_zat: u64,
    pub transparent_value_balance_zat: i64,
    pub sapling_value_balance_zat: i64,
    pub orchard_value_balance_zat: i64,
    pub ironwood_value_balance_zat: i64,
    pub transparent_input_count: u32,
    pub transparent_output_count: u32,
    pub joinsplit_count: u32,
    pub sapling_spend_count: u32,
    pub sapling_output_count: u32,
    pub orchard_action_count: u32,
    pub ironwood_action_count: u32,
    pub transparent_output_total_zat: i64,
    pub primary_from: Option<TransactionValueEndpoint>,
    pub primary_to: Option<TransactionValueEndpoint>,
}

/// Compact canonical activity positions for one transparent address.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExplorerAddressRecord {
    /// Number of canonical transactions involving this address.
    pub transaction_count: u64,
    /// Earliest canonical transaction involving this address.
    pub first_location: TransactionLocation,
    /// Latest canonical transaction involving this address.
    pub last_location: TransactionLocation,
    /// Earliest canonical transaction with an output paying this address.
    pub first_funding_location: Option<TransactionLocation>,
}

/// The primary explorer transaction category.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum ExplorerTransactionKind {
    Shielded = 1,
    Transparent = 2,
    Coinbase = 3,
}

/// The observable direction of value crossing the transparent/shielded boundary.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum ExplorerShieldedFlow {
    Shield = 1,
    Deshield = 2,
    FullyShielded = 3,
    Complex = 4,
}

/// The shielded pool, or pool combination, touched by a transaction.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum ExplorerShieldedPool {
    Sprout = 1,
    Sapling = 2,
    Orchard = 3,
    Ironwood = 4,
    Mixed = 5,
}

/// A disjoint public-flow amount bucket.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
#[allow(clippy::enum_variant_names)]
pub enum ExplorerAmountBucket {
    BelowOneBillion = 0,
    AtLeastOneBillion = 1,
    AtLeastTenBillion = 2,
    AtLeastOneHundredBillion = 3,
}

impl ExplorerAmountBucket {
    pub fn from_amount(amount_zat: Option<u64>) -> Self {
        match amount_zat {
            Some(amount) if amount >= 100_000_000_000 => Self::AtLeastOneHundredBillion,
            Some(amount) if amount >= 10_000_000_000 => Self::AtLeastTenBillion,
            Some(amount) if amount >= 1_000_000_000 => Self::AtLeastOneBillion,
            Some(_) | None => Self::BelowOneBillion,
        }
    }

    const fn tag(self) -> u8 {
        match self {
            Self::BelowOneBillion => 0,
            Self::AtLeastOneBillion => 1,
            Self::AtLeastTenBillion => 2,
            Self::AtLeastOneHundredBillion => 3,
        }
    }
}

impl ExplorerTransactionKind {
    const fn tag(self) -> u8 {
        match self {
            Self::Shielded => 1,
            Self::Transparent => 2,
            Self::Coinbase => 3,
        }
    }
}

impl ExplorerShieldedFlow {
    const fn tag(self) -> u8 {
        match self {
            Self::Shield => 1,
            Self::Deshield => 2,
            Self::FullyShielded => 3,
            Self::Complex => 4,
        }
    }
}

impl ExplorerShieldedPool {
    const fn tag(self) -> u8 {
        match self {
            Self::Sprout => 1,
            Self::Sapling => 2,
            Self::Orchard => 3,
            Self::Ironwood => 4,
            Self::Mixed => 5,
        }
    }
}

/// One kind-ordered transaction entry.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ExplorerTransactionKindLocation {
    pub kind: ExplorerTransactionKind,
    pub location: TransactionLocation,
}

/// One exact shielded classification entry.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ExplorerShieldedClassLocation {
    pub flow: ExplorerShieldedFlow,
    pub pool: ExplorerShieldedPool,
    pub amount_bucket: ExplorerAmountBucket,
    pub location: TransactionLocation,
}

const EXPLORER_TRANSACTION_ENDPOINT_BYTES: usize = 22;
const EXPLORER_TRANSACTION_RECORD_BYTES: usize = 124;
const EXPLORER_ADDRESS_RECORD_BYTES: usize = 24;

impl IntoDisk for ExplorerAddressRecord {
    type Bytes = [u8; EXPLORER_ADDRESS_RECORD_BYTES];

    fn as_bytes(&self) -> Self::Bytes {
        let mut bytes = [0; EXPLORER_ADDRESS_RECORD_BYTES];
        bytes[..8].copy_from_slice(&self.transaction_count.to_be_bytes());
        bytes[8..13].copy_from_slice(&self.first_location.as_bytes());
        bytes[13..18].copy_from_slice(&self.last_location.as_bytes());
        if let Some(location) = self.first_funding_location {
            bytes[18] = 1;
            bytes[19..24].copy_from_slice(&location.as_bytes());
        }
        bytes
    }
}

impl FromDisk for ExplorerAddressRecord {
    fn from_bytes(bytes: impl AsRef<[u8]>) -> Self {
        let bytes = bytes.as_ref();
        assert_eq!(bytes.len(), EXPLORER_ADDRESS_RECORD_BYTES);
        Self {
            transaction_count: u64::from_be_bytes(
                bytes[..8]
                    .try_into()
                    .expect("explorer address count is eight bytes"),
            ),
            first_location: TransactionLocation::from_bytes(&bytes[8..13]),
            last_location: TransactionLocation::from_bytes(&bytes[13..18]),
            first_funding_location: (bytes[18] == 1)
                .then(|| TransactionLocation::from_bytes(&bytes[19..24])),
        }
    }
}

impl IntoDisk for ExplorerTransactionRecord {
    type Bytes = [u8; EXPLORER_TRANSACTION_RECORD_BYTES];

    fn as_bytes(&self) -> Self::Bytes {
        let mut bytes = [0; EXPLORER_TRANSACTION_RECORD_BYTES];
        let mut offset = 0;
        put(&mut bytes, &mut offset, &self.serialized_size.to_be_bytes());
        put(&mut bytes, &mut offset, &self.fee_zat.to_be_bytes());
        for value in [
            self.transparent_value_balance_zat,
            self.sapling_value_balance_zat,
            self.orchard_value_balance_zat,
            self.ironwood_value_balance_zat,
        ] {
            put(&mut bytes, &mut offset, &value.to_be_bytes());
        }
        for value in [
            self.transparent_input_count,
            self.transparent_output_count,
            self.joinsplit_count,
            self.sapling_spend_count,
            self.sapling_output_count,
            self.orchard_action_count,
            self.ironwood_action_count,
        ] {
            put(&mut bytes, &mut offset, &value.to_be_bytes());
        }
        put(
            &mut bytes,
            &mut offset,
            &self.transparent_output_total_zat.to_be_bytes(),
        );
        put(
            &mut bytes,
            &mut offset,
            &transaction_endpoint_bytes(self.primary_from),
        );
        put(
            &mut bytes,
            &mut offset,
            &transaction_endpoint_bytes(self.primary_to),
        );
        debug_assert_eq!(offset, EXPLORER_TRANSACTION_RECORD_BYTES);
        bytes
    }
}

impl FromDisk for ExplorerTransactionRecord {
    fn from_bytes(bytes: impl AsRef<[u8]>) -> Self {
        let bytes = bytes.as_ref();
        assert_eq!(bytes.len(), EXPLORER_TRANSACTION_RECORD_BYTES);
        let mut offset = 0;
        Self {
            serialized_size: u32::from_be_bytes(take(bytes, &mut offset)),
            fee_zat: u64::from_be_bytes(take(bytes, &mut offset)),
            transparent_value_balance_zat: i64::from_be_bytes(take(bytes, &mut offset)),
            sapling_value_balance_zat: i64::from_be_bytes(take(bytes, &mut offset)),
            orchard_value_balance_zat: i64::from_be_bytes(take(bytes, &mut offset)),
            ironwood_value_balance_zat: i64::from_be_bytes(take(bytes, &mut offset)),
            transparent_input_count: u32::from_be_bytes(take(bytes, &mut offset)),
            transparent_output_count: u32::from_be_bytes(take(bytes, &mut offset)),
            joinsplit_count: u32::from_be_bytes(take(bytes, &mut offset)),
            sapling_spend_count: u32::from_be_bytes(take(bytes, &mut offset)),
            sapling_output_count: u32::from_be_bytes(take(bytes, &mut offset)),
            orchard_action_count: u32::from_be_bytes(take(bytes, &mut offset)),
            ironwood_action_count: u32::from_be_bytes(take(bytes, &mut offset)),
            transparent_output_total_zat: i64::from_be_bytes(take(bytes, &mut offset)),
            primary_from: transaction_endpoint_from_bytes(take(bytes, &mut offset)),
            primary_to: transaction_endpoint_from_bytes(take(bytes, &mut offset)),
        }
    }
}

fn transaction_endpoint_bytes(
    endpoint: Option<TransactionValueEndpoint>,
) -> [u8; EXPLORER_TRANSACTION_ENDPOINT_BYTES] {
    let mut bytes = [0; EXPLORER_TRANSACTION_ENDPOINT_BYTES];
    bytes[0] = match endpoint {
        None => 0,
        Some(TransactionValueEndpoint::Coinbase) => 1,
        Some(TransactionValueEndpoint::Transparent(address)) => {
            bytes[1..].copy_from_slice(&address.as_bytes());
            2
        }
        Some(TransactionValueEndpoint::Sprout) => 3,
        Some(TransactionValueEndpoint::Sapling) => 4,
        Some(TransactionValueEndpoint::Orchard) => 5,
        Some(TransactionValueEndpoint::Ironwood) => 6,
    };
    bytes
}

fn transaction_endpoint_from_bytes(
    bytes: [u8; EXPLORER_TRANSACTION_ENDPOINT_BYTES],
) -> Option<TransactionValueEndpoint> {
    match bytes[0] {
        0 => None,
        1 => Some(TransactionValueEndpoint::Coinbase),
        2 => Some(TransactionValueEndpoint::Transparent(
            address_from_disk_bytes(&bytes[1..]),
        )),
        3 => Some(TransactionValueEndpoint::Sprout),
        4 => Some(TransactionValueEndpoint::Sapling),
        5 => Some(TransactionValueEndpoint::Orchard),
        6 => Some(TransactionValueEndpoint::Ironwood),
        tag => panic!("invalid explorer transaction endpoint tag {tag}"),
    }
}

impl IntoDisk for ExplorerTransactionKindLocation {
    type Bytes = [u8; 1 + TRANSACTION_LOCATION_DISK_BYTES];

    fn as_bytes(&self) -> Self::Bytes {
        let mut bytes = [0; 1 + TRANSACTION_LOCATION_DISK_BYTES];
        bytes[0] = self.kind.tag();
        bytes[1..].copy_from_slice(&self.location.as_bytes());
        bytes
    }
}

impl FromDisk for ExplorerTransactionKindLocation {
    fn from_bytes(bytes: impl AsRef<[u8]>) -> Self {
        let bytes = bytes.as_ref();
        assert_eq!(bytes.len(), 1 + TRANSACTION_LOCATION_DISK_BYTES);
        Self {
            kind: transaction_kind_from_tag(bytes[0]),
            location: TransactionLocation::from_bytes(&bytes[1..]),
        }
    }
}

impl IntoDisk for ExplorerShieldedClassLocation {
    type Bytes = [u8; 3 + TRANSACTION_LOCATION_DISK_BYTES];

    fn as_bytes(&self) -> Self::Bytes {
        let mut bytes = [0; 3 + TRANSACTION_LOCATION_DISK_BYTES];
        bytes[0] = self.flow.tag();
        bytes[1] = self.pool.tag();
        bytes[2] = self.amount_bucket.tag();
        bytes[3..].copy_from_slice(&self.location.as_bytes());
        bytes
    }
}

impl FromDisk for ExplorerShieldedClassLocation {
    fn from_bytes(bytes: impl AsRef<[u8]>) -> Self {
        let bytes = bytes.as_ref();
        assert_eq!(bytes.len(), 3 + TRANSACTION_LOCATION_DISK_BYTES);
        Self {
            flow: shielded_flow_from_tag(bytes[0]),
            pool: shielded_pool_from_tag(bytes[1]),
            amount_bucket: amount_bucket_from_tag(bytes[2]),
            location: TransactionLocation::from_bytes(&bytes[3..]),
        }
    }
}

fn put<const N: usize>(bytes: &mut [u8], offset: &mut usize, value: &[u8; N]) {
    bytes[*offset..*offset + N].copy_from_slice(value);
    *offset += N;
}

fn take<const N: usize>(bytes: &[u8], offset: &mut usize) -> [u8; N] {
    let value = bytes[*offset..*offset + N]
        .try_into()
        .expect("fixed explorer record field has the expected length");
    *offset += N;
    value
}

fn transaction_kind_from_tag(tag: u8) -> ExplorerTransactionKind {
    match tag {
        1 => ExplorerTransactionKind::Shielded,
        2 => ExplorerTransactionKind::Transparent,
        3 => ExplorerTransactionKind::Coinbase,
        _ => panic!("invalid explorer transaction kind tag {tag}"),
    }
}

fn shielded_flow_from_tag(tag: u8) -> ExplorerShieldedFlow {
    match tag {
        1 => ExplorerShieldedFlow::Shield,
        2 => ExplorerShieldedFlow::Deshield,
        3 => ExplorerShieldedFlow::FullyShielded,
        4 => ExplorerShieldedFlow::Complex,
        _ => panic!("invalid explorer shielded flow tag {tag}"),
    }
}

fn shielded_pool_from_tag(tag: u8) -> ExplorerShieldedPool {
    match tag {
        1 => ExplorerShieldedPool::Sprout,
        2 => ExplorerShieldedPool::Sapling,
        3 => ExplorerShieldedPool::Orchard,
        4 => ExplorerShieldedPool::Ironwood,
        5 => ExplorerShieldedPool::Mixed,
        _ => panic!("invalid explorer shielded pool tag {tag}"),
    }
}

fn amount_bucket_from_tag(tag: u8) -> ExplorerAmountBucket {
    match tag {
        0 => ExplorerAmountBucket::BelowOneBillion,
        1 => ExplorerAmountBucket::AtLeastOneBillion,
        2 => ExplorerAmountBucket::AtLeastTenBillion,
        3 => ExplorerAmountBucket::AtLeastOneHundredBillion,
        _ => panic!("invalid explorer amount bucket tag {tag}"),
    }
}

#[cfg(test)]
mod tests {
    use zakura_chain::{block::Height, parameters::NetworkKind};

    use super::*;

    #[test]
    fn transaction_record_round_trips_fixed_width_encoding() {
        let record = ExplorerTransactionRecord {
            serialized_size: 1234,
            fee_zat: 10_000,
            transparent_value_balance_zat: -1,
            sapling_value_balance_zat: 2,
            orchard_value_balance_zat: -3,
            ironwood_value_balance_zat: 4,
            transparent_input_count: 5,
            transparent_output_count: 6,
            joinsplit_count: 7,
            sapling_spend_count: 8,
            sapling_output_count: 9,
            orchard_action_count: 10,
            ironwood_action_count: 11,
            transparent_output_total_zat: 12,
            primary_from: Some(TransactionValueEndpoint::Transparent(
                Address::from_pub_key_hash(NetworkKind::Mainnet, [13; 20]),
            )),
            primary_to: Some(TransactionValueEndpoint::Ironwood),
        };

        assert_eq!(
            ExplorerTransactionRecord::from_bytes(record.as_bytes()),
            record
        );
    }

    #[test]
    fn address_record_round_trips_fixed_width_encoding() {
        let record = ExplorerAddressRecord {
            transaction_count: 42,
            first_location: TransactionLocation::from_usize(Height(10), 1),
            last_location: TransactionLocation::from_usize(Height(20), 2),
            first_funding_location: Some(TransactionLocation::from_usize(Height(12), 3)),
        };

        assert_eq!(ExplorerAddressRecord::from_bytes(record.as_bytes()), record);
    }

    #[test]
    fn ordered_keys_round_trip() {
        let location = TransactionLocation::from_usize(Height(42), 7);
        let kind = ExplorerTransactionKindLocation {
            kind: ExplorerTransactionKind::Shielded,
            location,
        };
        let class = ExplorerShieldedClassLocation {
            flow: ExplorerShieldedFlow::Deshield,
            pool: ExplorerShieldedPool::Orchard,
            amount_bucket: ExplorerAmountBucket::AtLeastTenBillion,
            location,
        };

        assert_eq!(
            ExplorerTransactionKindLocation::from_bytes(kind.as_bytes()),
            kind
        );
        assert_eq!(
            ExplorerShieldedClassLocation::from_bytes(class.as_bytes()),
            class
        );
    }

    #[test]
    fn balance_keys_sort_richest_first_and_round_trip() {
        let lower_address = Address::from_pub_key_hash(NetworkKind::Mainnet, [1; 20]);
        let higher_address = Address::from_pub_key_hash(NetworkKind::Mainnet, [2; 20]);
        let richest = ExplorerBalanceKey::new(higher_address, 20);
        let poorer = ExplorerBalanceKey::new(lower_address, 10);

        assert!(richest.as_bytes() < poorer.as_bytes());
        assert_eq!(ExplorerBalanceKey::from_bytes(richest.as_bytes()), richest);
        assert_eq!(richest.address(), higher_address);
        assert_eq!(richest.balance_zat(), 20);
    }

    #[test]
    fn day_keys_preserve_chronological_order_and_round_trip() {
        let earlier = ExplorerDayKey(1);
        let later = ExplorerDayKey(256);

        assert!(earlier.as_bytes() < later.as_bytes());
        assert_eq!(ExplorerDayKey::from_bytes(later.as_bytes()), later);
    }

    #[test]
    fn analytics_values_round_trip_stable_encoding() {
        let chain = ExplorerChainStats {
            block_count: 42,
            total_fees_zat: 1_000,
            ..Default::default()
        };
        let daily = ExplorerDailyStats {
            day: 20_000,
            end_height: 2_000_000,
            ..Default::default()
        };

        assert_eq!(ExplorerChainStats::from_bytes(chain.as_bytes()), chain);
        assert_eq!(ExplorerDailyStats::from_bytes(daily.as_bytes()), daily);
    }
}
