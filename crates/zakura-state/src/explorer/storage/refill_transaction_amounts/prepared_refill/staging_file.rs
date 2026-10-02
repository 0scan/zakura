//! Durable, validated artifact format for a prepared transaction metadata refill.

use std::{
    fs::{self, File, OpenOptions},
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
    partial_path: PathBuf,
    writer: BufWriter<File>,
    checksum: Sha256,
    record_count: u64,
    source_tip_height: Height,
    source_tip_hash: block::Hash,
    last_location: Option<TransactionLocation>,
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

        let partial_path = partial_path_for(output_path);
        let file = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&partial_path)
            .map_err(|source| {
                if source.kind() == std::io::ErrorKind::AlreadyExists {
                    RefillTransactionAmountsError::PreparedFileAlreadyExists(partial_path.clone())
                } else {
                    file_io_error(&partial_path, source)
                }
            })?;
        let mut writer = BufWriter::new(file);
        write_header(
            &mut writer,
            network_kind,
            source_tip_height,
            source_tip_hash,
            0,
        )
        .and_then(|()| writer.flush())
        .and_then(|()| writer.get_ref().sync_all())
        .map_err(|source| file_io_error(&partial_path, source))?;

        Ok(Self {
            output_path: output_path.to_path_buf(),
            partial_path,
            writer,
            checksum: Sha256::new(),
            record_count: 0,
            source_tip_height,
            source_tip_hash,
            last_location: None,
        })
    }

    /// Reopens a partial artifact, validates every complete record, and removes a torn tail.
    pub(super) fn resume(
        output_path: &Path,
        partial_path: &Path,
        requested_network: NetworkKind,
    ) -> Result<Self, RefillTransactionAmountsError> {
        if output_path
            .try_exists()
            .map_err(|source| file_io_error(output_path, source))?
        {
            return Err(RefillTransactionAmountsError::PreparedFileAlreadyExists(
                output_path.to_path_buf(),
            ));
        }

        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(partial_path)
            .map_err(|source| file_io_error(partial_path, source))?;
        let file_length = file
            .metadata()
            .map_err(|source| file_io_error(partial_path, source))?
            .len();
        let (prepared_network, manifest) =
            read_header(&mut file).map_err(|source| file_io_error(partial_path, source))?;
        if prepared_network != requested_network {
            return Err(RefillTransactionAmountsError::PreparedNetworkMismatch {
                prepared: prepared_network,
                requested: requested_network,
            });
        }

        let payload_bytes = file_length.checked_sub(HEADER_BYTES).ok_or_else(|| {
            invalid_file_error(partial_path, "partial artifact is shorter than its header")
        })?;
        let available_record_count = payload_bytes / ENTRY_BYTES;
        if manifest.record_count > available_record_count {
            return Err(invalid_file_error(
                partial_path,
                format!(
                    "header commits {} records but only {available_record_count} are complete",
                    manifest.record_count
                ),
            ));
        }
        let recovered_record_count = if manifest.record_count == 0 {
            available_record_count
        } else {
            manifest.record_count
        };

        let mut checksum = Sha256::new();
        let mut last_location = None;
        let max_location = TransactionLocation::max_for_height(manifest.source_tip_height);
        for _ in 0..recovered_record_count {
            let (location, location_bytes, record_bytes) = read_entry_bytes(&mut file)
                .map_err(|source| file_io_error(partial_path, source))?;
            validate_location(partial_path, last_location, location, max_location)?;
            checksum.update(location_bytes);
            checksum.update(record_bytes);
            last_location = Some(location);
        }

        let committed_length = recovered_record_count
            .checked_mul(ENTRY_BYTES)
            .and_then(|entries| entries.checked_add(HEADER_BYTES))
            .ok_or_else(|| invalid_file_error(partial_path, "partial artifact length overflows"))?;
        file.set_len(committed_length)
            .and_then(|()| file.seek(SeekFrom::Start(RECORD_COUNT_OFFSET)).map(|_| ()))
            .and_then(|()| file.write_all(&recovered_record_count.to_be_bytes()))
            .and_then(|()| file.sync_all())
            .and_then(|()| file.seek(SeekFrom::Start(committed_length)).map(|_| ()))
            .map_err(|source| file_io_error(partial_path, source))?;

        Ok(Self {
            output_path: output_path.to_path_buf(),
            partial_path: partial_path.to_path_buf(),
            writer: BufWriter::new(file),
            checksum,
            record_count: recovered_record_count,
            source_tip_height: manifest.source_tip_height,
            source_tip_hash: manifest.source_tip_hash,
            last_location,
        })
    }

    pub(super) fn manifest(&self) -> PreparedRefillManifest {
        PreparedRefillManifest {
            source_tip_height: self.source_tip_height,
            source_tip_hash: self.source_tip_hash,
            record_count: self.record_count,
        }
    }

    pub(super) fn last_location(&self) -> Option<TransactionLocation> {
        self.last_location
    }

    pub(super) fn write_entries(
        &mut self,
        entries: Vec<UpgradedTransactionEntry>,
    ) -> Result<(), RefillTransactionAmountsError> {
        if entries.is_empty() {
            return Ok(());
        }

        let max_location = TransactionLocation::max_for_height(self.source_tip_height);
        for entry in entries {
            validate_location(
                &self.partial_path,
                self.last_location,
                entry.location,
                max_location,
            )?;
            let location_bytes = entry.location.as_bytes();
            let record_bytes = entry.record.raw_bytes();
            if record_bytes.len() != EXPLORER_TRANSACTION_RECORD_BYTES {
                return Err(invalid_file_error(
                    &self.partial_path,
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
                .map_err(|source| file_io_error(&self.partial_path, source))?;
            self.checksum.update(location_bytes);
            self.checksum.update(record_bytes);
            self.record_count = self
                .record_count
                .checked_add(1)
                .expect("transaction count fits in u64");
            self.last_location = Some(entry.location);
        }

        self.checkpoint()?;
        Ok(())
    }

    pub(super) fn finish(mut self) -> Result<u64, RefillTransactionAmountsError> {
        let checksum = self.checksum.finalize();
        self.writer
            .write_all(&checksum)
            .and_then(|()| self.writer.flush())
            .and_then(|()| self.writer.get_ref().sync_all())
            .map_err(|source| file_io_error(&self.partial_path, source))?;
        let file = self
            .writer
            .into_inner()
            .map_err(|error| file_io_error(&self.partial_path, error.into_error()))?;
        drop(file);

        fs::hard_link(&self.partial_path, &self.output_path).map_err(|source| {
            if source.kind() == std::io::ErrorKind::AlreadyExists {
                RefillTransactionAmountsError::PreparedFileAlreadyExists(self.output_path.clone())
            } else {
                file_io_error(&self.output_path, source)
            }
        })?;
        fs::remove_file(&self.partial_path)
            .map_err(|source| file_io_error(&self.partial_path, source))?;

        Ok(self.record_count)
    }

    fn checkpoint(&mut self) -> Result<(), RefillTransactionAmountsError> {
        self.writer
            .flush()
            .and_then(|()| self.writer.get_ref().sync_data())
            .and_then(|()| {
                self.writer
                    .get_mut()
                    .seek(SeekFrom::Start(RECORD_COUNT_OFFSET))
                    .map(|_| ())
            })
            .and_then(|()| {
                self.writer
                    .get_mut()
                    .write_all(&self.record_count.to_be_bytes())
            })
            .and_then(|()| self.writer.get_ref().sync_data())
            .and_then(|()| self.writer.get_mut().seek(SeekFrom::End(0)).map(|_| ()))
            .map_err(|source| file_io_error(&self.partial_path, source))?;
        tracing::info!(
            prepared_records = self.record_count,
            checkpoint = %self.partial_path.display(),
            "checkpointed prepared transaction metadata"
        );

        Ok(())
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
        let (location, location_bytes, record_bytes) =
            read_entry_bytes(reader).map_err(|source| file_io_error(path, source))?;
        checksum.update(location_bytes);
        checksum.update(record_bytes);

        validate_location(path, previous_location, location, max_location)?;
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
    let (location, _, record_bytes) = read_entry_bytes(reader)?;
    Ok(UpgradedTransactionEntry {
        location,
        record: RawBytes::new_raw_bytes(record_bytes.to_vec()),
    })
}

fn read_entry_bytes(
    reader: &mut impl Read,
) -> std::io::Result<(
    TransactionLocation,
    [u8; TRANSACTION_LOCATION_BYTES],
    [u8; EXPLORER_TRANSACTION_RECORD_BYTES],
)> {
    let mut location_bytes = [0; TRANSACTION_LOCATION_BYTES];
    let mut record_bytes = [0; EXPLORER_TRANSACTION_RECORD_BYTES];
    reader.read_exact(&mut location_bytes)?;
    reader.read_exact(&mut record_bytes)?;
    Ok((
        TransactionLocation::from_bytes(location_bytes),
        location_bytes,
        record_bytes,
    ))
}

fn validate_location(
    path: &Path,
    previous_location: Option<TransactionLocation>,
    location: TransactionLocation,
    max_location: TransactionLocation,
) -> Result<(), RefillTransactionAmountsError> {
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
                max_location.height
            ),
        ));
    }

    Ok(())
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

pub(super) fn partial_path_for(output_path: &Path) -> PathBuf {
    let mut partial_path = output_path.as_os_str().to_os_string();
    partial_path.push(".partial");
    partial_path.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_entry(location: TransactionLocation) -> UpgradedTransactionEntry {
        UpgradedTransactionEntry {
            location,
            record: RawBytes::new_raw_bytes(vec![0; EXPLORER_TRANSACTION_RECORD_BYTES]),
        }
    }

    #[test]
    fn resume_keeps_complete_records_and_discards_a_torn_tail() {
        let directory = tempfile::tempdir().expect("temporary directory is created");
        let output_path = directory.path().join("transaction-refill.zkr");
        let partial_path = partial_path_for(&output_path);
        let source_tip_height = Height(1);
        let source_tip_hash = block::Hash([7; 32]);
        let location = TransactionLocation::from_usize(source_tip_height, 0);

        let mut writer = PreparedRefillWriter::create(
            &output_path,
            NetworkKind::Mainnet,
            source_tip_height,
            source_tip_hash,
        )
        .expect("partial artifact is created");
        writer
            .write_entries(vec![test_entry(location)])
            .expect("complete record is checkpointed");
        drop(writer);

        let mut partial = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&partial_path)
            .expect("partial artifact reopens");
        partial
            .seek(SeekFrom::Start(RECORD_COUNT_OFFSET))
            .and_then(|_| partial.write_all(&0_u64.to_be_bytes()))
            .and_then(|()| partial.seek(SeekFrom::End(0)).map(|_| ()))
            .and_then(|()| partial.write_all(&location.as_bytes()))
            .and_then(|()| partial.sync_all())
            .expect("legacy zero count and torn location are written");
        drop(partial);

        let writer =
            PreparedRefillWriter::resume(&output_path, &partial_path, NetworkKind::Mainnet)
                .expect("partial artifact is recovered");
        assert_eq!(writer.manifest().record_count, 1);
        assert_eq!(writer.last_location(), Some(location));
        assert_eq!(
            fs::metadata(&partial_path)
                .expect("partial artifact exists")
                .len(),
            HEADER_BYTES + ENTRY_BYTES
        );
        writer.finish().expect("recovered artifact is finalized");

        assert!(!partial_path.exists());
        let mut reader = PreparedRefillReader::open_validated(&output_path, NetworkKind::Mainnet)
            .expect("final artifact validates");
        let entries = reader.read_batch(10).expect("record is readable");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].location, location);
        assert!(reader.is_finished());
    }

    #[test]
    fn resume_discards_complete_records_after_the_committed_count() {
        let directory = tempfile::tempdir().expect("temporary directory is created");
        let output_path = directory.path().join("transaction-refill.zkr");
        let partial_path = partial_path_for(&output_path);
        let source_tip_height = Height(1);
        let committed_location = TransactionLocation::from_usize(source_tip_height, 0);
        let uncommitted_location = TransactionLocation::from_usize(source_tip_height, 1);

        let mut writer = PreparedRefillWriter::create(
            &output_path,
            NetworkKind::Mainnet,
            source_tip_height,
            block::Hash([7; 32]),
        )
        .expect("partial artifact is created");
        writer
            .write_entries(vec![test_entry(committed_location)])
            .expect("first record is checkpointed");
        drop(writer);

        let mut partial = OpenOptions::new()
            .append(true)
            .open(&partial_path)
            .expect("partial artifact reopens");
        partial
            .write_all(&uncommitted_location.as_bytes())
            .and_then(|()| partial.write_all(&[0; EXPLORER_TRANSACTION_RECORD_BYTES]))
            .and_then(|()| partial.sync_all())
            .expect("uncommitted record is appended");
        drop(partial);

        let writer =
            PreparedRefillWriter::resume(&output_path, &partial_path, NetworkKind::Mainnet)
                .expect("committed checkpoint is recovered");
        assert_eq!(writer.manifest().record_count, 1);
        assert_eq!(writer.last_location(), Some(committed_location));
        assert_eq!(
            fs::metadata(&partial_path)
                .expect("partial artifact exists")
                .len(),
            HEADER_BYTES + ENTRY_BYTES
        );
    }
}
