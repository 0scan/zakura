//! Durable, validated artifact format for a prepared transaction metadata refill.

use std::{
    fs::File,
    io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};
use zakura_chain::{
    block::{self, Height},
    parameters::NetworkKind,
};

use crate::{
    explorer::storage::disk_format::EXPLORER_TRANSACTION_RECORD_BYTES,
    service::finalized_state::{FromDisk, IntoDisk, RawBytes, TransactionLocation},
};

use super::super::{
    count_as_u64, source_transactions::UpgradedTransactionEntry, RefillTransactionAmountsError,
};

const MAGIC: [u8; 8] = *b"ZKRFILL2";
const FORMAT_VERSION: u32 = 1;
const RESERVED_BYTES: [u8; 3] = [0; 3];
const CHECKSUM_BYTES: usize = 32;
const TRANSACTION_LOCATION_BYTES: usize = 5;
const RECORD_COUNT_OFFSET: u64 = 52;
const HEADER_BYTES: u64 = 60;
const ENTRY_BYTES: u64 = 129;

/// Identity and coverage information encoded in a prepared refill artifact.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct PreparedRefillManifest {
    pub(super) source_tip_height: Height,
    pub(super) source_tip_hash: block::Hash,
    pub(super) record_count: u64,
}

/// Atomically creates one prepared refill artifact without replacing an existing file.
pub(super) struct PreparedRefillWriter {
    output_path: PathBuf,
    writer: BufWriter<tempfile::NamedTempFile>,
    checksum: Sha256,
    record_count: u64,
}

impl PreparedRefillWriter {
    pub(super) fn create(
        output_path: &Path,
        network_kind: NetworkKind,
        source_tip_height: Height,
        source_tip_hash: block::Hash,
    ) -> Result<Self, RefillTransactionAmountsError> {
        if output_path
            .try_exists()
            .map_err(|source| file_io_error(output_path, source))?
        {
            return Err(RefillTransactionAmountsError::PreparedFileAlreadyExists(
                output_path.to_path_buf(),
            ));
        }

        let parent = output_path.parent().unwrap_or_else(|| Path::new("."));
        let temporary_file = tempfile::NamedTempFile::new_in(parent)
            .map_err(|source| file_io_error(output_path, source))?;
        let mut writer = BufWriter::new(temporary_file);
        write_header(
            &mut writer,
            network_kind,
            source_tip_height,
            source_tip_hash,
            0,
        )
        .map_err(|source| file_io_error(output_path, source))?;

        Ok(Self {
            output_path: output_path.to_path_buf(),
            writer,
            checksum: Sha256::new(),
            record_count: 0,
        })
    }

    pub(super) fn write_entries(
        &mut self,
        entries: Vec<UpgradedTransactionEntry>,
    ) -> Result<(), RefillTransactionAmountsError> {
        for entry in entries {
            let location_bytes = entry.location.as_bytes();
            let record_bytes = entry.record.raw_bytes();
            if record_bytes.len() != EXPLORER_TRANSACTION_RECORD_BYTES {
                return Err(invalid_file_error(
                    &self.output_path,
                    format!(
                        "calculated record at {:?} has length {}, expected {}",
                        entry.location,
                        record_bytes.len(),
                        EXPLORER_TRANSACTION_RECORD_BYTES
                    ),
                ));
            }

            self.writer
                .write_all(&location_bytes)
                .and_then(|()| self.writer.write_all(record_bytes))
                .map_err(|source| file_io_error(&self.output_path, source))?;
            self.checksum.update(location_bytes);
            self.checksum.update(record_bytes);
            self.record_count = self
                .record_count
                .checked_add(1)
                .expect("transaction count fits in u64");
        }

        Ok(())
    }

    pub(super) fn finish(mut self) -> Result<u64, RefillTransactionAmountsError> {
        let checksum = self.checksum.finalize();
        self.writer
            .write_all(&checksum)
            .and_then(|()| self.writer.flush())
            .map_err(|source| file_io_error(&self.output_path, source))?;
        let mut temporary_file = self
            .writer
            .into_inner()
            .map_err(|error| file_io_error(&self.output_path, error.into_error()))?;
        temporary_file
            .as_file_mut()
            .seek(SeekFrom::Start(RECORD_COUNT_OFFSET))
            .and_then(|_| {
                temporary_file
                    .as_file_mut()
                    .write_all(&self.record_count.to_be_bytes())
            })
            .and_then(|()| temporary_file.as_file_mut().sync_all())
            .map_err(|source| file_io_error(&self.output_path, source))?;

        temporary_file
            .persist_noclobber(&self.output_path)
            .map_err(|error| {
                if error.error.kind() == std::io::ErrorKind::AlreadyExists {
                    RefillTransactionAmountsError::PreparedFileAlreadyExists(
                        self.output_path.clone(),
                    )
                } else {
                    file_io_error(&self.output_path, error.error)
                }
            })?;

        Ok(self.record_count)
    }
}

/// A checksum-validated artifact reader positioned at its first prepared entry.
pub(super) struct PreparedRefillReader {
    path: PathBuf,
    reader: BufReader<File>,
    manifest: PreparedRefillManifest,
    remaining_records: u64,
}

impl PreparedRefillReader {
    pub(super) fn open_validated(
        path: &Path,
        requested_network: NetworkKind,
    ) -> Result<Self, RefillTransactionAmountsError> {
        let file = File::open(path).map_err(|source| file_io_error(path, source))?;
        let file_length = file
            .metadata()
            .map_err(|source| file_io_error(path, source))?
            .len();
        let mut reader = BufReader::new(file);
        let (prepared_network, manifest) =
            read_header(&mut reader).map_err(|source| file_io_error(path, source))?;
        if prepared_network != requested_network {
            return Err(RefillTransactionAmountsError::PreparedNetworkMismatch {
                prepared: prepared_network,
                requested: requested_network,
            });
        }

        let expected_length = manifest
            .record_count
            .checked_mul(ENTRY_BYTES)
            .and_then(|entries| entries.checked_add(HEADER_BYTES))
            .and_then(|bytes| {
                bytes.checked_add(
                    u64::try_from(CHECKSUM_BYTES).expect("checksum byte count fits in u64"),
                )
            })
            .ok_or_else(|| invalid_file_error(path, "artifact length overflows u64"))?;
        if file_length != expected_length {
            return Err(invalid_file_error(
                path,
                format!("file length is {file_length}, expected {expected_length}"),
            ));
        }

        validate_entries(path, &mut reader, manifest)?;
        reader
            .seek(SeekFrom::Start(HEADER_BYTES))
            .map_err(|source| file_io_error(path, source))?;

        Ok(Self {
            path: path.to_path_buf(),
            reader,
            manifest,
            remaining_records: manifest.record_count,
        })
    }

    pub(super) fn manifest(&self) -> PreparedRefillManifest {
        self.manifest
    }

    pub(super) fn read_batch(
        &mut self,
        batch_size: usize,
    ) -> Result<Vec<UpgradedTransactionEntry>, RefillTransactionAmountsError> {
        let remaining_records = usize::try_from(self.remaining_records).unwrap_or(usize::MAX);
        let record_count = remaining_records.min(batch_size);
        let mut entries = Vec::with_capacity(record_count);
        for _ in 0..record_count {
            entries.push(
                read_entry(&mut self.reader).map_err(|source| file_io_error(&self.path, source))?,
            );
        }
        self.remaining_records = self
            .remaining_records
            .checked_sub(count_as_u64(record_count))
            .expect("reader never consumes more records than remain");

        Ok(entries)
    }

    pub(super) fn is_finished(&self) -> bool {
        self.remaining_records == 0
    }
}

fn write_header(
    writer: &mut impl Write,
    network_kind: NetworkKind,
    source_tip_height: Height,
    source_tip_hash: block::Hash,
    record_count: u64,
) -> std::io::Result<()> {
    writer.write_all(&MAGIC)?;
    writer.write_all(&FORMAT_VERSION.to_be_bytes())?;
    writer.write_all(&[network_kind_byte(network_kind)])?;
    writer.write_all(&RESERVED_BYTES)?;
    writer.write_all(&source_tip_height.0.to_be_bytes())?;
    writer.write_all(&source_tip_hash.as_bytes())?;
    writer.write_all(&record_count.to_be_bytes())?;
    Ok(())
}

fn read_header(reader: &mut impl Read) -> std::io::Result<(NetworkKind, PreparedRefillManifest)> {
    let mut magic = [0; MAGIC.len()];
    reader.read_exact(&mut magic)?;
    if magic != MAGIC {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "unexpected prepared refill magic",
        ));
    }

    let format_version = read_u32(reader)?;
    if format_version != FORMAT_VERSION {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("unsupported prepared refill format {format_version}"),
        ));
    }

    let mut network = [0; 1];
    reader.read_exact(&mut network)?;
    let network = network_kind_from_byte(network[0])?;
    let mut reserved = RESERVED_BYTES;
    reader.read_exact(&mut reserved)?;
    if reserved != RESERVED_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "prepared refill reserved header bytes are non-zero",
        ));
    }

    let source_tip_height = Height(read_u32(reader)?);
    let mut source_tip_hash = [0; 32];
    reader.read_exact(&mut source_tip_hash)?;
    let source_tip_hash = block::Hash::from_bytes(source_tip_hash);
    let record_count = read_u64(reader)?;

    Ok((
        network,
        PreparedRefillManifest {
            source_tip_height,
            source_tip_hash,
            record_count,
        },
    ))
}

fn validate_entries(
    path: &Path,
    reader: &mut impl Read,
    manifest: PreparedRefillManifest,
) -> Result<(), RefillTransactionAmountsError> {
    let mut checksum = Sha256::new();
    let mut previous_location = None;
    let max_location = TransactionLocation::max_for_height(manifest.source_tip_height);
    for _ in 0..manifest.record_count {
        let mut location_bytes = [0; TRANSACTION_LOCATION_BYTES];
        let mut record_bytes = [0; EXPLORER_TRANSACTION_RECORD_BYTES];
        reader
            .read_exact(&mut location_bytes)
            .and_then(|()| reader.read_exact(&mut record_bytes))
            .map_err(|source| file_io_error(path, source))?;
        checksum.update(location_bytes);
        checksum.update(record_bytes);

        let location = TransactionLocation::from_bytes(location_bytes);
        if previous_location.is_some_and(|previous| previous >= location) {
            return Err(invalid_file_error(
                path,
                format!("record locations are not strictly ordered at {location:?}"),
            ));
        }
        if location > max_location {
            return Err(invalid_file_error(
                path,
                format!(
                    "record at {location:?} is above source tip {:?}",
                    manifest.source_tip_height
                ),
            ));
        }
        previous_location = Some(location);
    }

    let mut expected_checksum = [0; CHECKSUM_BYTES];
    reader
        .read_exact(&mut expected_checksum)
        .map_err(|source| file_io_error(path, source))?;
    let actual_checksum = checksum.finalize();
    if actual_checksum.as_slice() != expected_checksum {
        return Err(invalid_file_error(path, "SHA-256 checksum mismatch"));
    }

    Ok(())
}

fn read_entry(reader: &mut impl Read) -> std::io::Result<UpgradedTransactionEntry> {
    let mut location_bytes = [0; TRANSACTION_LOCATION_BYTES];
    let mut record_bytes = [0; EXPLORER_TRANSACTION_RECORD_BYTES];
    reader.read_exact(&mut location_bytes)?;
    reader.read_exact(&mut record_bytes)?;
    Ok(UpgradedTransactionEntry {
        location: TransactionLocation::from_bytes(location_bytes),
        record: RawBytes::new_raw_bytes(record_bytes.to_vec()),
    })
}

fn read_u32(reader: &mut impl Read) -> std::io::Result<u32> {
    let mut bytes = [0; size_of::<u32>()];
    reader.read_exact(&mut bytes)?;
    Ok(u32::from_be_bytes(bytes))
}

fn read_u64(reader: &mut impl Read) -> std::io::Result<u64> {
    let mut bytes = [0; size_of::<u64>()];
    reader.read_exact(&mut bytes)?;
    Ok(u64::from_be_bytes(bytes))
}

fn network_kind_byte(network: NetworkKind) -> u8 {
    match network {
        NetworkKind::Mainnet => 0,
        NetworkKind::Testnet => 1,
        NetworkKind::Regtest => 2,
    }
}

fn network_kind_from_byte(byte: u8) -> std::io::Result<NetworkKind> {
    match byte {
        0 => Ok(NetworkKind::Mainnet),
        1 => Ok(NetworkKind::Testnet),
        2 => Ok(NetworkKind::Regtest),
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("unknown prepared refill network kind {byte}"),
        )),
    }
}

fn file_io_error(path: &Path, source: std::io::Error) -> RefillTransactionAmountsError {
    RefillTransactionAmountsError::PreparedFileIo {
        path: path.to_path_buf(),
        source,
    }
}

fn invalid_file_error(path: &Path, reason: impl Into<String>) -> RefillTransactionAmountsError {
    RefillTransactionAmountsError::InvalidPreparedFile {
        path: path.to_path_buf(),
        reason: reason.into(),
    }
}
