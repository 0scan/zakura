//! Stable transaction record and ordered-index encodings.

use zakura_chain::{block::Height, transaction::Hash as TransactionHash};

use crate::{
    database::{
        decode_trailing_transaction_position, transaction_position_bytes,
        TRANSACTION_POSITION_BYTES,
    },
    models::{TransactionPosition, TransactionRecord},
    types::TransactionKind,
    Error,
};

use super::filter::{AmountFilter, ShieldedFlowFilter, ShieldedPoolFilter};

pub(super) const TRANSACTION_RECORD_BYTES: usize = 80;
const KIND_ORDER_KEY_BYTES: usize = 9;
const SHIELDED_ORDER_KEY_BYTES: usize = 11;

pub(super) fn transaction_record_key(txid: TransactionHash) -> [u8; 32] {
    txid.0
}

pub(super) fn transaction_position_key(
    position: TransactionPosition,
) -> [u8; TRANSACTION_POSITION_BYTES] {
    transaction_position_bytes(position)
}

pub(super) fn transaction_height_prefix(height: Height) -> [u8; 4] {
    height.0.to_be_bytes()
}

pub(super) fn newest_transaction_position_key() -> [u8; TRANSACTION_POSITION_BYTES] {
    [u8::MAX; TRANSACTION_POSITION_BYTES]
}

pub(super) fn encode_transaction_hash(txid: TransactionHash) -> [u8; 32] {
    txid.0
}

pub(super) fn decode_transaction_hash(bytes: &[u8]) -> Result<TransactionHash, Error> {
    let bytes = bytes
        .try_into()
        .map_err(|_| Error::CorruptData("stored transaction hash must be 32 bytes".to_string()))?;
    Ok(TransactionHash(bytes))
}

pub(super) fn transaction_kind_order_prefix(kind: TransactionKind) -> [u8; 1] {
    [transaction_kind_tag(kind)]
}

pub(super) fn transaction_kind_order_key(
    kind: TransactionKind,
    position: TransactionPosition,
) -> [u8; KIND_ORDER_KEY_BYTES] {
    let mut key = [0; KIND_ORDER_KEY_BYTES];
    key[0] = transaction_kind_tag(kind);
    key[1..].copy_from_slice(&transaction_position_key(position));
    key
}

pub(super) fn newest_transaction_kind_order_key(
    kind: TransactionKind,
) -> [u8; KIND_ORDER_KEY_BYTES] {
    let mut key = [u8::MAX; KIND_ORDER_KEY_BYTES];
    key[0] = transaction_kind_tag(kind);
    key
}

pub(super) fn shielded_order_prefix(
    flow: ShieldedFlowFilter,
    pool: ShieldedPoolFilter,
    amount: AmountFilter,
) -> [u8; 3] {
    [flow.tag(), pool.tag(), amount.tag()]
}

pub(super) fn shielded_order_key(
    flow: ShieldedFlowFilter,
    pool: ShieldedPoolFilter,
    amount: AmountFilter,
    position: TransactionPosition,
) -> [u8; SHIELDED_ORDER_KEY_BYTES] {
    let mut key = [0; SHIELDED_ORDER_KEY_BYTES];
    key[..3].copy_from_slice(&shielded_order_prefix(flow, pool, amount));
    key[3..].copy_from_slice(&transaction_position_key(position));
    key
}

pub(super) fn newest_shielded_order_key(
    flow: ShieldedFlowFilter,
    pool: ShieldedPoolFilter,
    amount: AmountFilter,
) -> [u8; SHIELDED_ORDER_KEY_BYTES] {
    let mut key = [u8::MAX; SHIELDED_ORDER_KEY_BYTES];
    key[..3].copy_from_slice(&shielded_order_prefix(flow, pool, amount));
    key
}

pub(super) fn decode_ordered_position(bytes: &[u8]) -> Result<TransactionPosition, Error> {
    decode_trailing_transaction_position(bytes)
}

pub(super) fn encode_transaction_record(
    record: TransactionRecord,
) -> [u8; TRANSACTION_RECORD_BYTES] {
    let mut bytes = [0; TRANSACTION_RECORD_BYTES];
    let mut offset = 0;

    encode_u32(&mut bytes, &mut offset, record.position.height.0);
    encode_u32(&mut bytes, &mut offset, record.position.transaction_index);
    encode_u32(&mut bytes, &mut offset, record.serialized_size);
    encode_u64(&mut bytes, &mut offset, record.fee_zat);
    encode_i64(
        &mut bytes,
        &mut offset,
        record.transparent_value_balance_zat,
    );
    encode_i64(&mut bytes, &mut offset, record.sapling_value_balance_zat);
    encode_i64(&mut bytes, &mut offset, record.orchard_value_balance_zat);
    encode_i64(&mut bytes, &mut offset, record.ironwood_value_balance_zat);
    encode_u32(&mut bytes, &mut offset, record.transparent_input_count);
    encode_u32(&mut bytes, &mut offset, record.transparent_output_count);
    encode_u32(&mut bytes, &mut offset, record.joinsplit_count);
    encode_u32(&mut bytes, &mut offset, record.sapling_spend_count);
    encode_u32(&mut bytes, &mut offset, record.sapling_output_count);
    encode_u32(&mut bytes, &mut offset, record.orchard_action_count);
    encode_u32(&mut bytes, &mut offset, record.ironwood_action_count);

    debug_assert_eq!(offset, TRANSACTION_RECORD_BYTES);
    bytes
}

pub(super) fn decode_transaction_record(bytes: &[u8]) -> Result<TransactionRecord, Error> {
    if bytes.len() != TRANSACTION_RECORD_BYTES {
        return Err(Error::CorruptData(format!(
            "stored transaction record must be {TRANSACTION_RECORD_BYTES} bytes"
        )));
    }
    let mut offset = 0;

    let record = TransactionRecord {
        position: TransactionPosition {
            height: Height(take_u32(bytes, &mut offset)?),
            transaction_index: take_u32(bytes, &mut offset)?,
        },
        serialized_size: take_u32(bytes, &mut offset)?,
        fee_zat: take_u64(bytes, &mut offset)?,
        transparent_value_balance_zat: take_i64(bytes, &mut offset)?,
        sapling_value_balance_zat: take_i64(bytes, &mut offset)?,
        orchard_value_balance_zat: take_i64(bytes, &mut offset)?,
        ironwood_value_balance_zat: take_i64(bytes, &mut offset)?,
        transparent_input_count: take_u32(bytes, &mut offset)?,
        transparent_output_count: take_u32(bytes, &mut offset)?,
        joinsplit_count: take_u32(bytes, &mut offset)?,
        sapling_spend_count: take_u32(bytes, &mut offset)?,
        sapling_output_count: take_u32(bytes, &mut offset)?,
        orchard_action_count: take_u32(bytes, &mut offset)?,
        ironwood_action_count: take_u32(bytes, &mut offset)?,
    };

    debug_assert_eq!(offset, TRANSACTION_RECORD_BYTES);
    Ok(record)
}

fn transaction_kind_tag(kind: TransactionKind) -> u8 {
    match kind {
        TransactionKind::Shielded => 1,
        TransactionKind::Transparent => 2,
        TransactionKind::Coinbase => 3,
    }
}

fn encode_u32(bytes: &mut [u8], offset: &mut usize, value: u32) {
    bytes[*offset..*offset + 4].copy_from_slice(&value.to_be_bytes());
    *offset += 4;
}

fn encode_u64(bytes: &mut [u8], offset: &mut usize, value: u64) {
    bytes[*offset..*offset + 8].copy_from_slice(&value.to_be_bytes());
    *offset += 8;
}

fn encode_i64(bytes: &mut [u8], offset: &mut usize, value: i64) {
    bytes[*offset..*offset + 8].copy_from_slice(&value.to_be_bytes());
    *offset += 8;
}

fn take_u32(bytes: &[u8], offset: &mut usize) -> Result<u32, Error> {
    let value = decode_u32(&bytes[*offset..*offset + 4])?;
    *offset += 4;
    Ok(value)
}

fn take_u64(bytes: &[u8], offset: &mut usize) -> Result<u64, Error> {
    let value = u64::from_be_bytes(bytes[*offset..*offset + 8].try_into().map_err(|_| {
        Error::CorruptData("stored transaction u64 field is truncated".to_string())
    })?);
    *offset += 8;
    Ok(value)
}

fn take_i64(bytes: &[u8], offset: &mut usize) -> Result<i64, Error> {
    let value = i64::from_be_bytes(bytes[*offset..*offset + 8].try_into().map_err(|_| {
        Error::CorruptData("stored transaction i64 field is truncated".to_string())
    })?);
    *offset += 8;
    Ok(value)
}

fn decode_u32(bytes: &[u8]) -> Result<u32, Error> {
    Ok(u32::from_be_bytes(bytes.try_into().map_err(|_| {
        Error::CorruptData("stored transaction u32 field must be 4 bytes".to_string())
    })?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transaction_record_has_a_stable_fixed_width_encoding() {
        let record = TransactionRecord {
            position: TransactionPosition {
                height: Height(42),
                transaction_index: 7,
            },
            serialized_size: 1_234,
            fee_zat: 10_000,
            transparent_value_balance_zat: -50,
            sapling_value_balance_zat: 20,
            orchard_value_balance_zat: -30,
            ironwood_value_balance_zat: 40,
            transparent_input_count: 1,
            transparent_output_count: 2,
            joinsplit_count: 3,
            sapling_spend_count: 4,
            sapling_output_count: 5,
            orchard_action_count: 6,
            ironwood_action_count: 7,
        };

        let encoded = encode_transaction_record(record);
        assert_eq!(encoded.len(), 80);
        assert_eq!(decode_transaction_record(&encoded).unwrap(), record);
        assert!(decode_transaction_record(&encoded[..79]).is_err());
    }
}
