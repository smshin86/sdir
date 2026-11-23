use crate::index_policy::VolumeGuid;
use goz_core::{
    index::{FrnMap, NTFS_ROOT_FRN, VolumeIndex},
    layout::{LayoutFile, LayoutName},
    types::Frn,
};
use serde::Serialize;
use std::{
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(windows)]
mod service_storage;

const MAGIC: &[u8; 8] = b"SDIRIDX\0";
const SCHEMA_VERSION: u32 = 1;
const ENGINE_VERSION: u32 = 1;
const FIXED_HEADER_BYTES: u64 = 108;
const CRC_OFFSET: usize = 100;
const MAX_POLICY_BYTES: usize = 64 * 1024;
const MAX_NAME_UNITS: usize = 255;
const MAX_NAME_BYTES: usize = MAX_NAME_UNITS * 3;
const TRANSFER_CHUNK_BYTES: usize = 12 * 1024;
const DIR_ATTRIBUTE: u32 = 0x10;
const FLAG_DIRECTORY: u8 = 1;
const FLAG_SIZE: u8 = 2;
const FLAG_MTIME: u8 = 4;
const FLAG_LOSSY_NAME: u8 = 8;
const KNOWN_FLAGS: u8 = FLAG_DIRECTORY | FLAG_SIZE | FLAG_MTIME | FLAG_LOSSY_NAME;
const CACHE_EXTENSION: &str = "sidx";
const CACHE_SUBDIR: &str = "index-cache";
const CACHE_VERSION_DIR: &str = "v1";
const LOCK_FILE: &str = ".profile.lock";
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug)]
pub enum CacheError {
    InvalidInput(&'static str),
    Corrupt(&'static str),
    Io(io::Error),
    Busy,
    Quota,
    UnsafePath,
    UnsupportedLocation,
}

impl fmt::Display for CacheError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(reason) => write!(f, "invalid cache input: {reason}"),
            Self::Corrupt(reason) => write!(f, "invalid cache data: {reason}"),
            Self::Io(error) => write!(f, "cache I/O failed: {error}"),
            Self::Busy => f.write_str("cache profile is already in use"),
            Self::Quota => f.write_str("cache disk quota would be exceeded"),
            Self::UnsafePath => f.write_str("cache path is linked, reparse-backed, or not owned"),
            Self::UnsupportedLocation => {
                f.write_str("persistent cache requires an owned local profile directory")
            }
        }
    }
}

impl std::error::Error for CacheError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for CacheError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for CacheError {
    fn from(_: serde_json::Error) -> Self {
        Self::InvalidInput("policy could not be serialized")
    }
}

pub type CacheResult<T> = Result<T, CacheError>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheIdentity {
    volume_guid: VolumeGuid,
    volume_serial: u64,
    policy_key: Vec<u8>,
}

impl CacheIdentity {
    /// The policy value is serialized through serde_json's canonical object-key
    /// order. Scope arrays must already use the normalized order of settings.
    pub fn new<P: Serialize>(
        volume_guid: &str,
        volume_serial: u64,
        normalized_policy: &P,
    ) -> CacheResult<Self> {
        let volume_guid = VolumeGuid::parse(volume_guid)
            .map_err(|_| CacheError::InvalidInput("volume GUID is not canonical"))?;
        let policy_value = serde_json::to_value(normalized_policy)?;
        let policy_key = serde_json::to_vec(&policy_value)?;
        if policy_key.len() > MAX_POLICY_BYTES {
            return Err(CacheError::InvalidInput("normalized policy is too large"));
        }
        Ok(Self {
            volume_guid,
            volume_serial,
            policy_key,
        })
    }

    pub fn volume_guid(&self) -> &VolumeGuid {
        &self.volume_guid
    }

    pub fn volume_serial(&self) -> u64 {
        self.volume_serial
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheMetadata {
    identity: CacheIdentity,
    journal_id: u64,
    committed_next_usn: i64,
    created_at_ms: u64,
}

impl CacheMetadata {
    pub fn new(
        identity: CacheIdentity,
        journal_id: u64,
        committed_next_usn: i64,
        created_at_ms: u64,
    ) -> CacheResult<Self> {
        if committed_next_usn < 0 {
            return Err(CacheError::InvalidInput(
                "committed USN must be nonnegative",
            ));
        }
        Ok(Self {
            identity,
            journal_id,
            committed_next_usn,
            created_at_ms,
        })
    }

    pub fn at_now(
        identity: CacheIdentity,
        journal_id: u64,
        committed_next_usn: i64,
    ) -> CacheResult<Self> {
        let created_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| CacheError::InvalidInput("system clock predates Unix epoch"))?
            .as_millis()
            .try_into()
            .map_err(|_| CacheError::InvalidInput("creation timestamp is out of range"))?;
        Self::new(identity, journal_id, committed_next_usn, created_at_ms)
    }

    pub fn identity(&self) -> &CacheIdentity {
        &self.identity
    }

    pub fn journal_id(&self) -> u64 {
        self.journal_id
    }

    pub fn committed_next_usn(&self) -> i64 {
        self.committed_next_usn
    }

    pub fn created_at_ms(&self) -> u64 {
        self.created_at_ms
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CacheLimits {
    pub max_total_bytes: u64,
    pub max_volume_bytes: u64,
    pub max_files: usize,
    pub max_entries: u64,
}

impl CacheLimits {
    pub fn new(
        max_total_bytes: u64,
        max_volume_bytes: u64,
        max_files: usize,
        max_entries: u64,
    ) -> CacheResult<Self> {
        if max_total_bytes == 0
            || max_volume_bytes == 0
            || max_volume_bytes > max_total_bytes
            || max_files == 0
            || max_entries == 0
        {
            return Err(CacheError::InvalidInput("cache limits are inconsistent"));
        }
        Ok(Self {
            max_total_bytes,
            max_volume_bytes,
            max_files,
            max_entries,
        })
    }
}

impl Default for CacheLimits {
    fn default() -> Self {
        Self {
            max_total_bytes: 512 * 1024 * 1024,
            max_volume_bytes: 256 * 1024 * 1024,
            max_files: 32,
            max_entries: 5_000_000,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CacheStats {
    pub rows: u64,
    pub payload_bytes: u64,
    pub file_bytes: u64,
    pub crc32: u32,
}

pub struct CachedVolume {
    pub metadata: CacheMetadata,
    pub index: VolumeIndex,
}

pub enum CacheLookup {
    Missing,
    Incompatible,
    Reader(CacheReader),
}

pub struct CacheReader {
    file: File,
    metadata: CacheMetadata,
    rows: u64,
    file_bytes: u64,
}

impl CacheReader {
    pub fn metadata(&self) -> &CacheMetadata {
        &self.metadata
    }

    pub fn rows(&self) -> u64 {
        self.rows
    }

    pub fn file_bytes(&self) -> u64 {
        self.file_bytes
    }
}

impl Read for CacheReader {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.file.read(bytes)
    }
}

pub struct CacheCandidate {
    directory: PathBuf,
    target: PathBuf,
    temporary: PathBuf,
    limits: CacheLimits,
    candidate_limit: u64,
    metadata: CacheMetadata,
    header_bytes: u64,
    end_bytes: u64,
    rewriting_header: bool,
    rewrite_cursor: u64,
    file: Option<File>,
    committed: bool,
}

impl CacheCandidate {
    pub fn write_chunk(&mut self, offset: u64, bytes: &[u8]) -> CacheResult<()> {
        if bytes.is_empty() || bytes.len() > TRANSFER_CHUNK_BYTES {
            return Err(CacheError::InvalidInput("cache transfer chunk is out of bounds"));
        }
        let end = offset
            .checked_add(bytes.len() as u64)
            .ok_or(CacheError::Quota)?;
        if end > self.candidate_limit {
            return Err(CacheError::Quota);
        }
        if self.rewriting_header {
            if offset != self.rewrite_cursor || end > self.header_bytes {
                return Err(CacheError::InvalidInput("cache header rewrite is out of order"));
            }
            self.rewrite_cursor = end;
        } else if offset == self.end_bytes {
            self.end_bytes = end;
        } else if offset == 0 && self.end_bytes >= self.header_bytes && end <= self.header_bytes {
            self.rewriting_header = true;
            self.rewrite_cursor = end;
        } else {
            return Err(CacheError::InvalidInput("cache transfer offset is out of order"));
        }
        let file = self
            .file
            .as_mut()
            .ok_or(CacheError::InvalidInput("cache candidate is closed"))?;
        file.seek(SeekFrom::Start(offset))?;
        file.write_all(bytes)?;
        Ok(())
    }

    pub fn commit(mut self) -> CacheResult<CacheStats> {
        if !self.rewriting_header || self.rewrite_cursor != self.header_bytes {
            return Err(CacheError::Corrupt("cache transfer did not finalize its header"));
        }
        let mut file = self
            .file
            .take()
            .ok_or(CacheError::InvalidInput("cache candidate is closed"))?;
        if file.metadata()?.len() != self.end_bytes {
            return Err(CacheError::Corrupt("cache transfer has a sparse or truncated tail"));
        }
        file.sync_all()?;
        file.seek(SeekFrom::Start(0))?;
        let header = read_header(&mut file, &self.limits)?;
        let file_bytes = FIXED_HEADER_BYTES
            .checked_add(header.metadata.identity.policy_key.len() as u64)
            .and_then(|value| value.checked_add(header.payload_bytes))
            .ok_or(CacheError::Quota)?;
        if file_bytes != self.end_bytes || header.metadata != self.metadata {
            return Err(CacheError::Corrupt("cache transfer metadata does not match its header"));
        }
        file.seek(SeekFrom::Start(0))?;
        let validated = decode_cache(&mut file, &self.limits)?;
        if validated.metadata != self.metadata {
            return Err(CacheError::Corrupt("cache transfer changed during validation"));
        }
        drop(validated);
        drop(file);
        fs::rename(&self.temporary, &self.target)?;
        sync_directory(&self.directory)?;
        self.committed = true;
        Ok(CacheStats {
            rows: header.row_count,
            payload_bytes: header.payload_bytes,
            file_bytes,
            crc32: header.crc32,
        })
    }
}

impl Drop for CacheCandidate {
    fn drop(&mut self) {
        if !self.committed {
            self.file.take();
            let _ = fs::remove_file(&self.temporary);
        }
    }
}

#[derive(Clone, Debug)]
struct CacheHeader {
    metadata: CacheMetadata,
    row_count: u64,
    payload_bytes: u64,
    crc32: u32,
}

impl CacheHeader {
    #[cfg(any(windows, test))]
    fn file_bytes(&self) -> CacheResult<u64> {
        FIXED_HEADER_BYTES
            .checked_add(self.metadata.identity.policy_key.len() as u64)
            .and_then(|bytes| bytes.checked_add(self.payload_bytes))
            .ok_or(CacheError::Quota)
    }
}

#[derive(Clone, Debug)]
struct CacheRow<N = Vec<u8>> {
    frn: u64,
    parent_frn: u64,
    is_directory: bool,
    name_lossy: bool,
    name: N,
    size: Option<u64>,
    mtime: Option<i64>,
}

pub fn encode_cache<W: Write + Seek>(
    index: &VolumeIndex,
    metadata: &CacheMetadata,
    limits: &CacheLimits,
    mut writer: W,
) -> CacheResult<CacheStats> {
    if index.may_have_unpresentable() {
        return Err(CacheError::InvalidInput(
            "index contains an orphan or parent cycle",
        ));
    }
    let header_len = FIXED_HEADER_BYTES
        .checked_add(metadata.identity.policy_key.len() as u64)
        .ok_or(CacheError::Quota)?;
    if header_len > limits.max_volume_bytes {
        return Err(CacheError::Quota);
    }
    write_header(&mut writer, metadata, 0, 0, 0)?;
    writer.seek(SeekFrom::Start(header_len))?;

    let mut payload = PayloadWriter::new(&mut writer, limits.max_volume_bytes - header_len);
    let mut rows = 0u64;
    for entry_index in index.live_entries() {
        if index.is_synthetic(entry_index) {
            continue;
        }
        if rows >= limits.max_entries {
            return Err(CacheError::Quota);
        }
        let entry = index.entry(entry_index);
        let parent = index.entry(entry.parent());
        let parent_frn = parent.frn().0;
        if index.is_synthetic(entry.parent()) && parent_frn != NTFS_ROOT_FRN.0 {
            return Err(CacheError::InvalidInput(
                "real entry has a synthetic parent",
            ));
        }
        let name = entry.name();
        let name_lossy = validate_name(name).ok_or(CacheError::InvalidInput(
            "index contains a malformed NTFS name",
        ))?;
        let is_directory = entry.is_dir();
        let size = if is_directory { None } else { entry.size() };
        let mtime = entry.mtime();
        let row = CacheRow {
            frn: entry.frn().0,
            parent_frn,
            is_directory,
            name_lossy,
            name,
            size,
            mtime,
        };
        write_row(&mut payload, &row)?;
        rows = rows.checked_add(1).ok_or(CacheError::Quota)?;
    }

    let payload_bytes = payload.bytes;
    let crc32 = payload.crc.finish();
    let file_bytes = header_len
        .checked_add(payload_bytes)
        .ok_or(CacheError::Quota)?;
    if file_bytes > limits.max_volume_bytes {
        return Err(CacheError::Quota);
    }
    writer.seek(SeekFrom::Start(0))?;
    write_header(&mut writer, metadata, rows, payload_bytes, crc32)?;
    writer.seek(SeekFrom::Start(file_bytes))?;
    Ok(CacheStats {
        rows,
        payload_bytes,
        file_bytes,
        crc32,
    })
}

fn write_header<W: Write>(
    writer: &mut W,
    metadata: &CacheMetadata,
    row_count: u64,
    payload_bytes: u64,
    crc32: u32,
) -> CacheResult<()> {
    let guid = metadata.identity.volume_guid.as_str().as_bytes();
    if guid.len() != 36 || metadata.identity.policy_key.len() > MAX_POLICY_BYTES {
        return Err(CacheError::InvalidInput("invalid cache identity"));
    }
    writer.write_all(MAGIC)?;
    write_u32(writer, SCHEMA_VERSION)?;
    write_u32(writer, ENGINE_VERSION)?;
    writer.write_all(guid)?;
    write_u64(writer, metadata.identity.volume_serial)?;
    write_u64(writer, metadata.journal_id)?;
    write_i64(writer, metadata.committed_next_usn)?;
    write_u64(writer, metadata.created_at_ms)?;
    write_u64(writer, row_count)?;
    write_u64(writer, payload_bytes)?;
    write_u32(writer, crc32)?;
    write_u32(
        writer,
        u32::try_from(metadata.identity.policy_key.len())
            .map_err(|_| CacheError::InvalidInput("policy key is too large"))?,
    )?;
    writer.write_all(&metadata.identity.policy_key)?;
    Ok(())
}

fn write_row<W: Write>(
    writer: &mut PayloadWriter<'_, W>,
    row: &CacheRow<impl AsRef<[u8]>>,
) -> CacheResult<()> {
    let name = row.name.as_ref();
    let mut flags = 0u8;
    if row.is_directory {
        flags |= FLAG_DIRECTORY;
    }
    if row.size.is_some() {
        flags |= FLAG_SIZE;
    }
    if row.mtime.is_some() {
        flags |= FLAG_MTIME;
    }
    if row.name_lossy {
        flags |= FLAG_LOSSY_NAME;
    }
    let name_len =
        u32::try_from(name.len()).map_err(|_| CacheError::InvalidInput("NTFS name is too long"))?;
    writer.write(&row.frn.to_le_bytes())?;
    writer.write(&row.parent_frn.to_le_bytes())?;
    writer.write(&[flags])?;
    writer.write(&name_len.to_le_bytes())?;
    writer.write(name)?;
    if let Some(size) = row.size {
        if row.is_directory {
            return Err(CacheError::InvalidInput("directory row has a file size"));
        }
        writer.write(&size.to_le_bytes())?;
    }
    if let Some(mtime) = row.mtime {
        if mtime < 0 {
            return Err(CacheError::InvalidInput("negative file timestamp"));
        }
        writer.write(&mtime.to_le_bytes())?;
    }
    Ok(())
}

struct PayloadWriter<'a, W> {
    writer: &'a mut W,
    crc: Crc32,
    bytes: u64,
    max_bytes: u64,
}

impl<'a, W: Write> PayloadWriter<'a, W> {
    fn new(writer: &'a mut W, max_bytes: u64) -> Self {
        Self {
            writer,
            crc: Crc32::new(),
            bytes: 0,
            max_bytes,
        }
    }

    fn write(&mut self, bytes: &[u8]) -> CacheResult<()> {
        let next = self
            .bytes
            .checked_add(bytes.len() as u64)
            .ok_or(CacheError::Quota)?;
        if next > self.max_bytes {
            return Err(CacheError::Quota);
        }
        self.writer.write_all(bytes)?;
        self.crc.update(bytes);
        self.bytes = next;
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct Crc32(u32);

impl Crc32 {
    fn new() -> Self {
        Self(u32::MAX)
    }

    fn update(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 ^= u32::from(byte);
            for _ in 0..8 {
                self.0 = if self.0 & 1 == 1 {
                    (self.0 >> 1) ^ 0xedb8_8320
                } else {
                    self.0 >> 1
                };
            }
        }
    }

    fn finish(self) -> u32 {
        !self.0
    }
}

fn write_u32<W: Write>(writer: &mut W, value: u32) -> io::Result<()> {
    writer.write_all(&value.to_le_bytes())
}

fn write_u64<W: Write>(writer: &mut W, value: u64) -> io::Result<()> {
    writer.write_all(&value.to_le_bytes())
}

fn write_i64<W: Write>(writer: &mut W, value: i64) -> io::Result<()> {
    writer.write_all(&value.to_le_bytes())
}

pub fn decode_cache<R: Read>(mut reader: R, limits: &CacheLimits) -> CacheResult<CachedVolume> {
    let header = read_header(&mut reader, limits)?;
    decode_cache_rows(reader, header, None)
}

fn decode_cache_rows<R: Read>(
    mut reader: R,
    header: CacheHeader,
    memory_budget: Option<u64>,
) -> CacheResult<CachedVolume> {
    let mut payload = PayloadReader::new(&mut reader, header.payload_bytes);
    let mut index = VolumeIndex::new(NTFS_ROOT_FRN, FrnMap::sparse());
    let mut name_keys = Vec::new();
    if let Some(budget) = memory_budget {
        // A row is at least 21 fixed bytes plus one byte of name.
        if header.row_count > header.payload_bytes / 22 {
            return Err(CacheError::Corrupt("row count cannot fit in cache payload"));
        }
        let keys_bytes = header.row_count.saturating_mul(size_of::<u128>() as u64);
        if keys_bytes.saturating_add(DECODE_SCRATCH_BYTES) > budget {
            return Err(CacheError::Quota);
        }
        let rows = usize::try_from(header.row_count).map_err(|_| CacheError::Quota)?;
        name_keys
            .try_reserve_exact(rows)
            .map_err(|_| CacheError::Quota)?;
        // Settle the pinned goz vectors before filling them so growth keeps
        // their documented 1/16 headroom instead of doubling whole columns.
        index.shrink_to_fit();
        check_decode_room(&index, &name_keys, budget)?;
    }

    for _ in 0..header.row_count {
        if let Some(budget) = memory_budget {
            check_decode_room(&index, &name_keys, budget)?;
        }
        let fixed = payload.read_array::<21>()?;
        let frn = u64::from_le_bytes(fixed[0..8].try_into().unwrap());
        let parent_frn = u64::from_le_bytes(fixed[8..16].try_into().unwrap());
        let flags = fixed[16];
        let name_len = u32::from_le_bytes(fixed[17..21].try_into().unwrap()) as usize;
        if flags & !KNOWN_FLAGS != 0
            || name_len == 0
            || name_len > MAX_NAME_BYTES
            || frn == 0
            || frn == NTFS_ROOT_FRN.0
            || frn == parent_frn
            || parent_frn == 0
        {
            return Err(CacheError::Corrupt("invalid row flags or identity"));
        }

        let is_directory = flags & FLAG_DIRECTORY != 0;
        let has_size = flags & FLAG_SIZE != 0;
        let has_mtime = flags & FLAG_MTIME != 0;
        if is_directory && has_size {
            return Err(CacheError::Corrupt("directory row contains file size"));
        }
        let name = payload.read_vec(name_len)?;
        let name_lossy = validate_name(&name).ok_or(CacheError::Corrupt("invalid WTF-8 name"))?;
        if name_lossy != (flags & FLAG_LOSSY_NAME != 0) {
            return Err(CacheError::Corrupt(
                "name encoding flag does not match name",
            ));
        }
        let size = if has_size {
            Some(u64::from_le_bytes(payload.read_array::<8>()?))
        } else {
            None
        };
        let mtime = if has_mtime {
            let value = i64::from_le_bytes(payload.read_array::<8>()?);
            if value < 0 {
                return Err(CacheError::Corrupt("negative file timestamp"));
            }
            Some(value)
        } else {
            None
        };

        let frn = Frn(frn);
        let parent_frn = Frn(parent_frn);
        if let Some(parent) = index.head_of(parent_frn)
            && !index.entry(parent).is_dir()
        {
            return Err(CacheError::Corrupt("file used as a parent directory"));
        }
        let append_hard_link = match index.head_of(frn) {
            Some(existing) if index.is_synthetic(existing) => {
                // The parent placeholder has no metadata from disk yet.
                if !is_directory {
                    return Err(CacheError::Corrupt("file used as a parent directory"));
                }
                false
            }
            Some(existing) => {
                let existing = index.entry(existing);
                if existing.is_dir() != is_directory
                    || (!is_directory && existing.size() != size)
                    || existing.mtime() != mtime
                {
                    return Err(CacheError::Corrupt("inconsistent rows for one FRN"));
                }
                !is_directory
            }
            None => false,
        };

        let key = (u128::from(parent_frn.0) << 64) | u128::from(hash_name(&name));
        name_keys.try_reserve(1).map_err(|_| CacheError::Quota)?;
        name_keys.push(key);

        let file = LayoutFile {
            frn,
            attributes: if is_directory { DIR_ATTRIBUTE } else { 0 },
            size,
            mtime_ft: mtime,
            ctime_ft: None,
            atime_ft: None,
            basic_attributes: None,
            names: vec![LayoutName {
                parent_frn,
                name,
                name_lossy,
                dos_only: false,
            }],
        };
        if append_hard_link {
            // insert_layout would rewrite a head that still has only one name.
            index.enrich(&file);
        } else {
            index.insert_layout(&file);
        }
        if memory_budget.is_some() && index.has_parent_cycle() {
            return Err(CacheError::Corrupt(
                "restored index contains a parent cycle",
            ));
        }
    }

    verify_payload_end(payload, header.crc32)?;

    if let Some(budget) = memory_budget {
        let held = index.memory().total().allocated
            + name_keys.capacity() as u64 * size_of::<u128>() as u64
            + DECODE_SCRATCH_BYTES;
        if held > budget {
            return Err(CacheError::Quota);
        }
    }
    name_keys.sort_unstable();
    if name_keys.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(CacheError::Corrupt("duplicate name in one directory"));
    }
    let real_rows = index
        .live_entries()
        .filter(|&entry| !index.is_synthetic(entry))
        .count() as u64;
    if real_rows != header.row_count {
        return Err(CacheError::Corrupt(
            "row count does not match restored index",
        ));
    }
    if index.may_have_unpresentable() {
        return Err(CacheError::Corrupt(
            "restored index contains orphan or cycle",
        ));
    }

    Ok(CachedVolume {
        metadata: header.metadata,
        index,
    })
}

fn verify_payload_end<R: Read>(
    payload: PayloadReader<'_, R>,
    expected_crc: u32,
) -> CacheResult<()> {
    if payload.remaining != 0 {
        return Err(CacheError::Corrupt(
            "payload length does not match row count",
        ));
    }
    if payload.crc.finish() != expected_crc {
        return Err(CacheError::Corrupt("payload CRC32 mismatch"));
    }
    let mut trailing = [0u8; 1];
    if payload.reader.read(&mut trailing)? != 0 {
        return Err(CacheError::Corrupt("trailing bytes after cache payload"));
    }
    Ok(())
}

#[cfg(any(windows, test))]
fn load_bounded_cache<R: Read>(
    mut reader: R,
    limits: &CacheLimits,
    expected: &CacheIdentity,
    disk_bytes: u64,
    memory_budget: u64,
) -> CacheResult<Option<CachedVolume>> {
    // read_header canonicalizes policy JSON (up to 64 KiB on disk). Leave room for
    // its DOM, serialized copy and buffered I/O before allocating the graph.
    if memory_budget < DECODE_SCRATCH_BYTES {
        return Err(CacheError::Quota);
    }
    let header = read_header(&mut reader, limits)?;
    if header.metadata.identity.volume_guid != expected.volume_guid {
        return Err(CacheError::Corrupt("filename and header GUID differ"));
    }
    if header.file_bytes()? != disk_bytes {
        return Err(CacheError::Corrupt(
            "cache file length does not match its header",
        ));
    }
    if header.metadata.identity != *expected {
        return Ok(None);
    }
    decode_cache_rows(reader, header, Some(memory_budget)).map(Some)
}

const DECODE_SCRATCH_BYTES: u64 = 8 * 1024 * 1024;

fn check_decode_room(index: &VolumeIndex, name_keys: &Vec<u128>, budget: u64) -> CacheResult<()> {
    let memory = index.memory();
    let allocated = memory.total().allocated;
    let largest = [
        memory.entries.allocated,
        memory.arena_raw.allocated,
        memory.arena_folded.allocated,
        memory.frn_map.allocated,
        memory.name_tables.allocated,
        memory.dir_children.allocated,
    ]
    .into_iter()
    .max()
    .unwrap_or(0);
    // The decoder settles entry columns and the name byte arenas up front.
    // A row adds at most one entry plus a parent placeholder. These settled
    // vectors grow by 1/16 or a small minimum in the pinned goz version.
    // Allow a quarter of their capacity for consecutive appends, and allow
    // doubling for hash tables and the unranked-name list. One old allocation
    // can overlap its replacement. For a corrupt FRN reuse, also allow the
    // free-list growth from reclaiming a chain of existing slots.
    let settled = memory
        .entries
        .allocated
        .saturating_add(memory.arena_raw.allocated)
        .saturating_add(memory.arena_folded.allocated);
    let reclaim = (index.allocated() as u64 + 2) * 3 * size_of::<u32>() as u64;
    let peak = allocated
        .saturating_add(settled / 4)
        .saturating_add(allocated - settled)
        .saturating_add(largest)
        .saturating_add(reclaim)
        .saturating_add(name_keys.capacity() as u64 * size_of::<u128>() as u64)
        .saturating_add(DECODE_SCRATCH_BYTES);
    if peak > budget {
        return Err(CacheError::Quota);
    }
    Ok(())
}

fn read_header<R: Read>(reader: &mut R, limits: &CacheLimits) -> CacheResult<CacheHeader> {
    let mut fixed = [0u8; FIXED_HEADER_BYTES as usize];
    read_exact(reader, &mut fixed, "truncated cache header")?;
    if &fixed[..8] != MAGIC {
        return Err(CacheError::Corrupt("cache magic mismatch"));
    }
    let schema = u32::from_le_bytes(fixed[8..12].try_into().unwrap());
    let engine = u32::from_le_bytes(fixed[12..16].try_into().unwrap());
    if schema != SCHEMA_VERSION || engine != ENGINE_VERSION {
        return Err(CacheError::Corrupt("unsupported cache or engine version"));
    }
    let guid_text = std::str::from_utf8(&fixed[16..52])
        .map_err(|_| CacheError::Corrupt("volume GUID is not ASCII"))?;
    let volume_guid = VolumeGuid::parse(guid_text)
        .map_err(|_| CacheError::Corrupt("invalid volume GUID in header"))?;
    if volume_guid.as_str() != guid_text {
        return Err(CacheError::Corrupt("volume GUID is not canonical"));
    }

    let volume_serial = u64::from_le_bytes(fixed[52..60].try_into().unwrap());
    let journal_id = u64::from_le_bytes(fixed[60..68].try_into().unwrap());
    let committed_next_usn = i64::from_le_bytes(fixed[68..76].try_into().unwrap());
    let created_at_ms = u64::from_le_bytes(fixed[76..84].try_into().unwrap());
    let row_count = u64::from_le_bytes(fixed[84..92].try_into().unwrap());
    let payload_bytes = u64::from_le_bytes(fixed[92..100].try_into().unwrap());
    let crc32 = u32::from_le_bytes(fixed[CRC_OFFSET..CRC_OFFSET + 4].try_into().unwrap());
    let policy_len = u32::from_le_bytes(fixed[104..108].try_into().unwrap()) as usize;
    if committed_next_usn < 0 || policy_len > MAX_POLICY_BYTES {
        return Err(CacheError::Corrupt("invalid header values"));
    }
    if row_count > limits.max_entries {
        return Err(CacheError::Quota);
    }
    let mut policy_key = vec![0u8; policy_len];
    read_exact(reader, &mut policy_key, "truncated normalized policy")?;
    let policy: serde_json::Value = serde_json::from_slice(&policy_key)
        .map_err(|_| CacheError::Corrupt("normalized policy is invalid JSON"))?;
    if serde_json::to_vec(&policy).map_err(|_| CacheError::Corrupt("invalid policy"))? != policy_key
    {
        return Err(CacheError::Corrupt(
            "normalized policy is not canonical JSON",
        ));
    }
    let header_bytes = FIXED_HEADER_BYTES
        .checked_add(policy_len as u64)
        .ok_or(CacheError::Quota)?;
    let file_bytes = header_bytes
        .checked_add(payload_bytes)
        .ok_or(CacheError::Quota)?;
    if file_bytes > limits.max_volume_bytes {
        return Err(CacheError::Quota);
    }

    Ok(CacheHeader {
        metadata: CacheMetadata {
            identity: CacheIdentity {
                volume_guid,
                volume_serial,
                policy_key,
            },
            journal_id,
            committed_next_usn,
            created_at_ms,
        },
        row_count,
        payload_bytes,
        crc32,
    })
}

fn read_exact<R: Read>(reader: &mut R, bytes: &mut [u8], reason: &'static str) -> CacheResult<()> {
    reader.read_exact(bytes).map_err(|error| {
        if error.kind() == io::ErrorKind::UnexpectedEof {
            CacheError::Corrupt(reason)
        } else {
            CacheError::Io(error)
        }
    })
}

struct PayloadReader<'a, R> {
    reader: &'a mut R,
    remaining: u64,
    crc: Crc32,
}

impl<'a, R: Read> PayloadReader<'a, R> {
    fn new(reader: &'a mut R, payload_bytes: u64) -> Self {
        Self {
            reader,
            remaining: payload_bytes,
            crc: Crc32::new(),
        }
    }

    fn read_vec(&mut self, len: usize) -> CacheResult<Vec<u8>> {
        let mut bytes = vec![0u8; len];
        self.read_exact(&mut bytes)?;
        Ok(bytes)
    }

    fn read_array<const N: usize>(&mut self) -> CacheResult<[u8; N]> {
        let mut bytes = [0u8; N];
        self.read_exact(&mut bytes)?;
        Ok(bytes)
    }

    fn read_exact(&mut self, bytes: &mut [u8]) -> CacheResult<()> {
        let len = bytes.len() as u64;
        if len > self.remaining {
            return Err(CacheError::Corrupt("row exceeds declared payload length"));
        }
        read_exact(self.reader, bytes, "truncated cache payload")?;
        self.crc.update(bytes);
        self.remaining -= len;
        Ok(())
    }
}

fn validate_name(bytes: &[u8]) -> Option<bool> {
    if bytes.is_empty() || bytes == b"." || bytes == b".." {
        return None;
    }
    let mut offset = 0usize;
    let mut utf16_units = 0usize;
    let mut has_lone_surrogate = false;
    let mut previous_high_surrogate = false;
    while offset < bytes.len() {
        let first = bytes[offset];
        let (length, mut codepoint) = match first {
            0x00..=0x7f => (1, u32::from(first)),
            0xc2..=0xdf => (2, u32::from(first & 0x1f)),
            0xe0..=0xef => (3, u32::from(first & 0x0f)),
            0xf0..=0xf4 => (4, u32::from(first & 0x07)),
            _ => return None,
        };
        let end = offset.checked_add(length)?;
        let sequence = bytes.get(offset..end)?;
        if length > 1 {
            for &continuation in &sequence[1..] {
                if continuation & 0xc0 != 0x80 {
                    return None;
                }
            }
            if (first == 0xe0 && sequence[1] < 0xa0)
                || (first == 0xf0 && sequence[1] < 0x90)
                || (first == 0xf4 && sequence[1] > 0x8f)
            {
                return None;
            }
            for &continuation in &sequence[1..] {
                codepoint = (codepoint << 6) | u32::from(continuation & 0x3f);
            }
        }
        if codepoint == 0 || codepoint == u32::from(b'/') || codepoint == u32::from(b'\\') {
            return None;
        }
        let is_high = (0xd800..=0xdbff).contains(&codepoint);
        let is_low = (0xdc00..=0xdfff).contains(&codepoint);
        if is_low && previous_high_surrogate {
            return None;
        }
        if is_high || is_low {
            has_lone_surrogate = true;
            utf16_units = utf16_units.checked_add(1)?;
        } else if codepoint <= 0xffff {
            utf16_units = utf16_units.checked_add(1)?;
        } else if codepoint <= 0x10ffff {
            utf16_units = utf16_units.checked_add(2)?;
        } else {
            return None;
        }
        previous_high_surrogate = is_high;
        if utf16_units > MAX_NAME_UNITS {
            return None;
        }
        offset = end;
    }
    Some(has_lone_surrogate)
}

fn hash_name(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

pub struct CacheStore {
    directory: PathBuf,
    _lock_file: File,
    limits: CacheLimits,
    #[cfg(windows)]
    service: Option<service_storage::ServiceStorage>,
}

impl CacheStore {
    pub fn open_default(limits: CacheLimits) -> CacheResult<Self> {
        Self::open_at(crate::settings::data_dir(), limits)
    }

    pub fn open_at(profile_root: impl AsRef<Path>, limits: CacheLimits) -> CacheResult<Self> {
        let profile_root = checked_profile_root(profile_root.as_ref())?;
        if is_network_location(&profile_root)? {
            return Err(CacheError::UnsupportedLocation);
        }
        let cache_root = ensure_child_directory(&profile_root, CACHE_SUBDIR)?;
        let directory = ensure_child_directory(&cache_root, CACHE_VERSION_DIR)?;
        let lock_path = cache_root.join(LOCK_FILE);
        let lock_file = open_lock_file(&lock_path)?;
        match lock_file.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => return Err(CacheError::Busy),
            Err(std::fs::TryLockError::Error(error)) => {
                return Err(CacheError::Io(error));
            }
        }
        let store = Self {
            directory,
            _lock_file: lock_file,
            limits,
            #[cfg(windows)]
            service: None,
        };
        store.remove_stale_temps()?;
        Ok(store)
    }

    pub fn open_transfer(&mut self, expected: &CacheIdentity) -> CacheResult<CacheLookup> {
        let path = self.cache_path(&expected.volume_guid);
        if !self.check_managed_file(&path)? {
            return Ok(CacheLookup::Missing);
        }
        let mut file = OpenOptions::new().read(true).open(&path)?;
        let disk_bytes = file.metadata()?.len();
        let header = read_header(&mut file, &self.limits)?;
        if header.metadata.identity.volume_guid != expected.volume_guid {
            return Err(CacheError::Corrupt("filename and header GUID differ"));
        }
        if header.metadata.identity.volume_serial != expected.volume_serial
            || header.metadata.identity.policy_key != expected.policy_key
        {
            drop(file);
            self.purge_guid(&expected.volume_guid)?;
            return Ok(CacheLookup::Incompatible);
        }
        let file_bytes = FIXED_HEADER_BYTES
            .checked_add(header.metadata.identity.policy_key.len() as u64)
            .and_then(|value| value.checked_add(header.payload_bytes))
            .ok_or(CacheError::Quota)?;
        if disk_bytes != file_bytes {
            return Err(CacheError::Corrupt("cache file length does not match its header"));
        }
        file.seek(SeekFrom::Start(0))?;
        Ok(CacheLookup::Reader(CacheReader {
            file,
            metadata: header.metadata,
            rows: header.row_count,
            file_bytes,
        }))
    }

    pub fn load(&mut self, expected: &CacheIdentity) -> CacheResult<Option<CachedVolume>> {
        let path = self.cache_path(&expected.volume_guid);
        if !self.check_managed_file(&path)? {
            return Ok(None);
        }
        let mut file = OpenOptions::new().read(true).open(&path)?;
        let cached = decode_cache(&mut file, &self.limits)?;
        if cached.metadata.identity.volume_guid != expected.volume_guid {
            return Err(CacheError::Corrupt("filename and header GUID differ"));
        }
        if cached.metadata.identity.volume_serial != expected.volume_serial
            || cached.metadata.identity.policy_key != expected.policy_key
        {
            return Ok(None);
        }
        Ok(Some(cached))
    }

    pub fn save(
        &mut self,
        index: &VolumeIndex,
        metadata: &CacheMetadata,
    ) -> CacheResult<CacheStats> {
        self.save_using(metadata, |file, limits| {
            encode_cache(index, metadata, limits, file)
        })
    }

    pub fn begin_candidate(&mut self, metadata: &CacheMetadata) -> CacheResult<CacheCandidate> {
        #[cfg(windows)]
        if self.service.is_some() {
            return Err(CacheError::UnsafePath);
        }
        self.remove_stale_temps()?;
        let target = self.cache_path(&metadata.identity.volume_guid);
        let target_exists = self.check_managed_file(&target)?;
        let usage = self.usage()?;
        if usage.files > self.limits.max_files {
            return Err(CacheError::Quota);
        }
        if !target_exists && usage.files >= self.limits.max_files {
            return Err(CacheError::Quota);
        }
        if usage.total_bytes >= self.limits.max_total_bytes {
            return Err(CacheError::Quota);
        }
        let available = self.limits.max_total_bytes - usage.total_bytes;
        let candidate_limit = available.min(self.limits.max_volume_bytes);
        if candidate_limit == 0 {
            return Err(CacheError::Quota);
        }
        let limits = CacheLimits {
            max_total_bytes: candidate_limit,
            max_volume_bytes: candidate_limit,
            max_files: self.limits.max_files,
            max_entries: self.limits.max_entries,
        };
        let temporary = self.new_temp_path(&metadata.identity.volume_guid);
        let file = create_new_file(&temporary)?;
        let header_bytes = FIXED_HEADER_BYTES
            .checked_add(metadata.identity.policy_key.len() as u64)
            .ok_or(CacheError::Quota)?;
        Ok(CacheCandidate {
            directory: self.directory.clone(),
            target,
            temporary,
            limits,
            candidate_limit,
            metadata: metadata.clone(),
            header_bytes,
            end_bytes: 0,
            rewriting_header: false,
            rewrite_cursor: 0,
            file: Some(file),
            committed: false,
        })
    }

    fn save_using<F>(&mut self, metadata: &CacheMetadata, encode: F) -> CacheResult<CacheStats>
    where
        F: FnOnce(&mut File, &CacheLimits) -> CacheResult<CacheStats>,
    {
        self.remove_stale_temps()?;
        let target = self.cache_path(&metadata.identity.volume_guid);
        let target_exists = self.check_managed_file(&target)?;
        let usage = self.usage()?;
        if usage.files > self.limits.max_files {
            return Err(CacheError::Quota);
        }
        if !target_exists && usage.files >= self.limits.max_files {
            return Err(CacheError::Quota);
        }
        if usage.total_bytes >= self.limits.max_total_bytes {
            return Err(CacheError::Quota);
        }
        let available = self.limits.max_total_bytes - usage.total_bytes;
        let candidate_limit = available.min(self.limits.max_volume_bytes);
        if candidate_limit == 0 {
            return Err(CacheError::Quota);
        }
        let staging_limits = CacheLimits {
            max_total_bytes: candidate_limit,
            max_volume_bytes: candidate_limit,
            max_files: self.limits.max_files,
            max_entries: self.limits.max_entries,
        };

        let temporary = self.new_temp_path(&metadata.identity.volume_guid);
        let mut file = self.create_save_file(&temporary)?;
        let result = (|| {
            let stats = encode(&mut file, &staging_limits)?;
            if stats.file_bytes > candidate_limit {
                return Err(CacheError::Quota);
            }
            file.sync_all()?;
            file.seek(SeekFrom::Start(0))?;
            self.verify_save_file(&mut file, &staging_limits, metadata, &stats)?;
            drop(file);
            self.commit_save_file(&temporary, &target)?;
            sync_directory(&self.directory)?;
            let committed_usage = self.usage()?;
            if committed_usage.total_bytes > self.limits.max_total_bytes
                || committed_usage.files > self.limits.max_files
            {
                return Err(CacheError::Quota);
            }
            Ok(stats)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    pub fn purge(&mut self, identity: &CacheIdentity) -> CacheResult<bool> {
        self.purge_guid(&identity.volume_guid)
    }

    pub fn purge_guid(&mut self, guid: &VolumeGuid) -> CacheResult<bool> {
        let target = self.cache_path(guid);
        let existed = self.check_managed_file(&target)?;
        if existed {
            fs::remove_file(&target)?;
        }
        self.remove_temps_for(guid)?;
        Ok(existed)
    }

    pub fn cached_guids(&self) -> CacheResult<Vec<VolumeGuid>> {
        let mut guids = Vec::new();
        for entry in fs::read_dir(&self.directory)? {
            let entry = entry?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if !matches!(classify_managed_name(&name), Some(ManagedName::Cache)) {
                continue;
            }
            let stem = name
                .strip_suffix(&format!(".{CACHE_EXTENSION}"))
                .ok_or(CacheError::UnsafePath)?;
            let guid = VolumeGuid::parse(stem).map_err(|_| CacheError::UnsafePath)?;
            if guid.as_str() != stem || !self.check_managed_file(&entry.path())? {
                return Err(CacheError::UnsafePath);
            }
            guids.push(guid);
        }
        guids.sort();
        guids.dedup();
        Ok(guids)
    }

    pub fn purge_all(&mut self) -> CacheResult<usize> {
        let guids = self.cached_guids()?;
        let mut removed = 0usize;
        for guid in guids {
            if self.purge_guid(&guid)? {
                removed += 1;
            }
        }
        self.remove_stale_temps()?;
        Ok(removed)
    }

    fn cache_path(&self, guid: &VolumeGuid) -> PathBuf {
        self.directory
            .join(format!("{}.{}", guid.as_str(), CACHE_EXTENSION))
    }

    fn new_temp_path(&self, guid: &VolumeGuid) -> PathBuf {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        self.directory.join(format!(
            ".tmp-{}-{}-{}",
            guid.as_str(),
            std::process::id(),
            sequence
        ))
    }

    fn check_managed_file(&self, path: &Path) -> CacheResult<bool> {
        #[cfg(windows)]
        if self.service.is_some() {
            return service_storage::check_private_file(path);
        }
        check_regular_file(path)
    }

    fn create_save_file(&self, path: &Path) -> CacheResult<File> {
        #[cfg(windows)]
        if self.service.is_some() {
            return service_storage::create_private_file(path);
        }
        create_new_file(path)
    }

    fn verify_save_file(
        &self,
        file: &mut File,
        limits: &CacheLimits,
        metadata: &CacheMetadata,
        _stats: &CacheStats,
    ) -> CacheResult<()> {
        #[cfg(windows)]
        if self.service.is_some() {
            return service_storage::verify_encoded_file(file, limits, metadata, _stats);
        }
        let validated = decode_cache(file, limits)?;
        if validated.metadata != *metadata {
            return Err(CacheError::Corrupt(
                "temporary header changed during validation",
            ));
        }
        Ok(())
    }

    fn commit_save_file(&self, source: &Path, target: &Path) -> CacheResult<()> {
        self.check_managed_file(target)?;
        #[cfg(windows)]
        if self.service.is_some() {
            return service_storage::replace_file(source, target);
        }
        fs::rename(source, target)?;
        Ok(())
    }

    fn usage(&self) -> CacheResult<CacheUsage> {
        let mut usage = CacheUsage::default();
        for entry in fs::read_dir(&self.directory)? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let Some(kind) = classify_managed_name(name) else {
                continue;
            };
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            #[cfg(windows)]
            if self.service.is_some() && !self.check_managed_file(&path)? {
                return Err(CacheError::UnsafePath);
            }
            verify_plain_metadata(&metadata)?;
            verify_owned_path(&path, &metadata)?;
            verify_private_file(&metadata)?;
            if !metadata.is_file() {
                return Err(CacheError::UnsafePath);
            }
            usage.total_bytes = usage
                .total_bytes
                .checked_add(metadata.len())
                .ok_or(CacheError::Quota)?;
            match kind {
                ManagedName::Cache => {
                    usage.files = usage.files.checked_add(1).ok_or(CacheError::Quota)?;
                }
                ManagedName::Temp(_) => {
                    usage.temps = usage.temps.checked_add(1).ok_or(CacheError::Quota)?;
                }
            }
        }
        if usage.temps > 1 {
            return Err(CacheError::UnsafePath);
        }
        Ok(usage)
    }

    fn remove_stale_temps(&self) -> CacheResult<()> {
        for entry in fs::read_dir(&self.directory)? {
            let entry = entry?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if let Some(ManagedName::Temp(guid)) = classify_managed_name(&name) {
                let path = entry.path();
                self.check_managed_file(&path)?;
                fs::remove_file(path)?;
                let _ = guid;
            }
        }
        Ok(())
    }

    fn remove_temps_for(&self, guid: &VolumeGuid) -> CacheResult<()> {
        let prefix = format!(".tmp-{}-", guid.as_str());
        for entry in fs::read_dir(&self.directory)? {
            let entry = entry?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if name.starts_with(&prefix)
                && matches!(classify_managed_name(&name), Some(ManagedName::Temp(_)))
            {
                let path = entry.path();
                self.check_managed_file(&path)?;
                fs::remove_file(path)?;
            }
        }
        Ok(())
    }
}

#[derive(Default)]
struct CacheUsage {
    total_bytes: u64,
    files: usize,
    temps: usize,
}

enum ManagedName {
    Cache,
    Temp(String),
}

fn classify_managed_name(name: &str) -> Option<ManagedName> {
    if let Some(stem) = name.strip_suffix(&format!(".{CACHE_EXTENSION}")) {
        let guid = VolumeGuid::parse(stem).ok()?;
        if guid.as_str() == stem {
            return Some(ManagedName::Cache);
        }
    }
    let tail = name.strip_prefix(".tmp-")?;
    let guid_text = tail.get(..36)?;
    if tail.as_bytes().get(36) != Some(&b'-') {
        return None;
    }
    let guid = VolumeGuid::parse(guid_text).ok()?;
    if guid.as_str() != guid_text {
        return None;
    }
    let mut parts = tail[37..].split('-');
    let pid = parts.next()?;
    let sequence = parts.next()?;
    if parts.next().is_some()
        || pid.is_empty()
        || sequence.is_empty()
        || !pid.bytes().all(|byte| byte.is_ascii_digit())
        || !sequence.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    Some(ManagedName::Temp(guid_text.to_owned()))
}

fn checked_profile_root(path: &Path) -> CacheResult<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let components: Vec<_> = absolute.components().collect();
    let mut current = PathBuf::new();
    for (index, component) in components.iter().enumerate() {
        match component {
            Component::ParentDir => return Err(CacheError::UnsafePath),
            Component::CurDir => continue,
            _ => current.push(component.as_os_str()),
        }
        let metadata = fs::symlink_metadata(&current).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                CacheError::UnsupportedLocation
            } else {
                CacheError::Io(error)
            }
        })?;
        verify_plain_metadata(&metadata)?;
        if index + 1 < components.len() && !metadata.is_dir() {
            return Err(CacheError::UnsafePath);
        }
    }
    let metadata = fs::symlink_metadata(&absolute)?;
    if !metadata.is_dir() {
        return Err(CacheError::UnsafePath);
    }
    verify_owned_path(&absolute, &metadata)?;
    Ok(absolute)
}

fn ensure_child_directory(parent: &Path, name: &str) -> CacheResult<PathBuf> {
    let path = parent.join(name);
    match fs::symlink_metadata(&path) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            create_private_directory(&path)?;
        }
        Err(error) => return Err(CacheError::Io(error)),
    }
    let metadata = fs::symlink_metadata(&path)?;
    verify_plain_metadata(&metadata)?;
    if !metadata.is_dir() {
        return Err(CacheError::UnsafePath);
    }
    verify_owned_path(&path, &metadata)?;
    verify_private_directory(&metadata)?;
    Ok(path)
}

fn open_lock_file(path: &Path) -> CacheResult<File> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            verify_plain_metadata(&metadata)?;
            if !metadata.is_file() {
                return Err(CacheError::UnsafePath);
            }
            verify_owned_path(path, &metadata)?;
            verify_private_file(&metadata)?;
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(path)
                .map_err(CacheError::Io)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => create_new_file(path),
        Err(error) => Err(CacheError::Io(error)),
    }
}

fn check_regular_file(path: &Path) -> CacheResult<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            verify_plain_metadata(&metadata)?;
            if !metadata.is_file() {
                return Err(CacheError::UnsafePath);
            }
            verify_owned_path(path, &metadata)?;
            verify_private_file(&metadata)?;
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(CacheError::Io(error)),
    }
}

#[cfg(unix)]
fn create_private_directory(path: &Path) -> CacheResult<()> {
    use std::os::unix::fs::DirBuilderExt;
    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    builder.create(path)?;
    Ok(())
}

#[cfg(not(unix))]
fn create_private_directory(path: &Path) -> CacheResult<()> {
    fs::create_dir(path)?;
    Ok(())
}

#[cfg(unix)]
fn create_new_file(path: &Path) -> CacheResult<File> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true).mode(0o600);
    options.open(path).map_err(CacheError::Io)
}

#[cfg(not(unix))]
fn create_new_file(path: &Path) -> CacheResult<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(CacheError::Io)
}

fn verify_plain_metadata(metadata: &fs::Metadata) -> CacheResult<()> {
    if metadata.file_type().is_symlink() || is_reparse_point(metadata) {
        return Err(CacheError::UnsafePath);
    }
    Ok(())
}

#[cfg(windows)]
fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn is_reparse_point(_: &fs::Metadata) -> bool {
    false
}

#[cfg(unix)]
fn verify_owned_path(_: &Path, metadata: &fs::Metadata) -> CacheResult<()> {
    use std::os::unix::fs::MetadataExt;
    unsafe extern "C" {
        fn geteuid() -> u32;
    }
    let current_uid = unsafe { geteuid() };
    if metadata.uid() != current_uid {
        return Err(CacheError::UnsupportedLocation);
    }
    Ok(())
}

#[cfg(windows)]
fn verify_owned_path(path: &Path, _: &fs::Metadata) -> CacheResult<()> {
    use std::{ffi::c_void, os::windows::ffi::OsStrExt, ptr};
    #[repr(C)]
    struct SidAndAttributes {
        sid: *mut c_void,
        attributes: u32,
    }
    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn OpenProcessToken(process: *mut c_void, access: u32, token: *mut *mut c_void) -> i32;
        fn GetTokenInformation(
            token: *mut c_void,
            class: u32,
            information: *mut c_void,
            length: u32,
            returned: *mut u32,
        ) -> i32;
        fn EqualSid(left: *const c_void, right: *const c_void) -> i32;
        fn GetNamedSecurityInfoW(
            name: *const u16,
            object_type: u32,
            security_info: u32,
            owner: *mut *mut c_void,
            group: *mut *mut c_void,
            dacl: *mut *mut c_void,
            sacl: *mut *mut c_void,
            descriptor: *mut *mut c_void,
        ) -> u32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> *mut c_void;
        fn CloseHandle(handle: *mut c_void) -> i32;
        fn LocalFree(memory: *mut c_void) -> *mut c_void;
    }

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut owner = ptr::null_mut();
    let mut descriptor = ptr::null_mut();
    let status = unsafe {
        GetNamedSecurityInfoW(
            wide.as_ptr(),
            1,
            1,
            &mut owner,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 {
        return Err(CacheError::Io(io::Error::from_raw_os_error(status as i32)));
    }

    let mut token = ptr::null_mut();
    let mut result = Err(CacheError::UnsupportedLocation);
    if unsafe { OpenProcessToken(GetCurrentProcess(), 0x0008, &mut token) } != 0 {
        let mut required = 0u32;
        unsafe {
            GetTokenInformation(token, 1, ptr::null_mut(), 0, &mut required);
        }
        if required >= std::mem::size_of::<SidAndAttributes>() as u32 {
            let mut buffer = vec![0u64; required.div_ceil(8) as usize];
            if unsafe {
                GetTokenInformation(
                    token,
                    1,
                    buffer.as_mut_ptr().cast(),
                    required,
                    &mut required,
                )
            } != 0
            {
                let user = unsafe { &*(buffer.as_ptr().cast::<SidAndAttributes>()) };
                if unsafe { EqualSid(owner, user.sid) } != 0 {
                    result = Ok(());
                }
            }
        }
    } else {
        result = Err(CacheError::Io(io::Error::last_os_error()));
    }
    if !token.is_null() {
        unsafe {
            CloseHandle(token);
        }
    }
    if !descriptor.is_null() {
        unsafe {
            LocalFree(descriptor);
        }
    }
    result
}

#[cfg(unix)]
fn verify_private_directory(metadata: &fs::Metadata) -> CacheResult<()> {
    use std::os::unix::fs::PermissionsExt;
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(CacheError::UnsafePath);
    }
    Ok(())
}

#[cfg(not(unix))]
fn verify_private_directory(_: &fs::Metadata) -> CacheResult<()> {
    Ok(())
}

#[cfg(unix)]
fn verify_private_file(metadata: &fs::Metadata) -> CacheResult<()> {
    use std::os::unix::fs::PermissionsExt;
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(CacheError::UnsafePath);
    }
    Ok(())
}

#[cfg(not(unix))]
fn verify_private_file(_: &fs::Metadata) -> CacheResult<()> {
    Ok(())
}

#[cfg(target_os = "linux")]
fn is_network_location(path: &Path) -> CacheResult<bool> {
    let canonical = fs::canonicalize(path)?;
    let mounts = fs::read_to_string("/proc/self/mounts")?;
    let mut selected: Option<(usize, String)> = None;
    for line in mounts.lines() {
        let mut fields = line.split_whitespace();
        let _source = fields.next();
        let Some(mount_field) = fields.next() else {
            continue;
        };
        let Some(filesystem) = fields.next() else {
            continue;
        };
        let mount = PathBuf::from(unescape_mount_field(mount_field));
        if canonical.starts_with(&mount)
            && selected
                .as_ref()
                .is_none_or(|(length, _)| mount.as_os_str().len() > *length)
        {
            selected = Some((mount.as_os_str().len(), filesystem.to_owned()));
        }
    }
    let Some((_, filesystem)) = selected else {
        return Err(CacheError::UnsupportedLocation);
    };
    Ok(matches!(
        filesystem.as_str(),
        "9p" | "afs" | "ceph" | "cifs" | "glusterfs" | "nfs" | "nfs4" | "smb3"
    ) || filesystem.starts_with("fuse.sshfs"))
}

#[cfg(target_os = "linux")]
fn unescape_mount_field(value: &str) -> String {
    value
        .replace("\\040", " ")
        .replace("\\011", "\t")
        .replace("\\134", "\\")
}

#[cfg(windows)]
fn is_network_location(path: &Path) -> CacheResult<bool> {
    use std::os::windows::ffi::OsStrExt;
    if path.to_string_lossy().starts_with(r"\\") {
        return Ok(true);
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetDriveTypeW(root: *const u16) -> u32;
    }
    let root: PathBuf = path.components().take(2).collect();
    let mut wide: Vec<u16> = root.as_os_str().encode_wide().collect();
    if wide.last().is_none_or(|unit| *unit != u16::from(b'\\')) {
        wide.push(u16::from(b'\\'));
    }
    wide.push(0);
    let drive_type = unsafe { GetDriveTypeW(wide.as_ptr()) };
    if drive_type == 0 {
        return Err(CacheError::Io(io::Error::last_os_error()));
    }
    Ok(drive_type == 4)
}

#[cfg(not(any(target_os = "linux", windows)))]
fn is_network_location(_: &Path) -> CacheResult<bool> {
    Ok(false)
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> CacheResult<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_directory(_: &Path) -> CacheResult<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use tempfile::TempDir;

    const GUID: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    const ROOT: u64 = NTFS_ROOT_FRN.0;

    fn identity(guid: &str, serial: u64, policy: &serde_json::Value) -> CacheIdentity {
        CacheIdentity::new(guid, serial, policy).unwrap()
    }

    fn metadata_for(
        guid: &str,
        serial: u64,
        policy: &serde_json::Value,
        journal_id: u64,
        next_usn: i64,
    ) -> CacheMetadata {
        CacheMetadata::new(identity(guid, serial, policy), journal_id, next_usn, 1234).unwrap()
    }

    fn limits() -> CacheLimits {
        CacheLimits::new(2 * 1024 * 1024, 1024 * 1024, 32, 100_000).unwrap()
    }

    fn row(
        frn: u64,
        parent_frn: u64,
        name: &[u8],
        is_directory: bool,
        size: Option<u64>,
        mtime: Option<i64>,
    ) -> CacheRow {
        CacheRow {
            frn,
            parent_frn,
            is_directory,
            name_lossy: validate_name(name).unwrap_or(false),
            name: name.to_vec(),
            size,
            mtime,
        }
    }

    fn insert_row(index: &mut VolumeIndex, row: &CacheRow) {
        index.insert_layout(&LayoutFile {
            frn: Frn(row.frn),
            attributes: if row.is_directory { DIR_ATTRIBUTE } else { 0 },
            size: row.size,
            mtime_ft: row.mtime,
            ctime_ft: None,
            atime_ft: None,
            basic_attributes: None,
            names: vec![LayoutName {
                parent_frn: Frn(row.parent_frn),
                name: row.name.clone(),
                name_lossy: row.name_lossy,
                dos_only: false,
            }],
        });
    }

    #[test]
    fn bounded_cache_checks_budget_identity_and_payload() {
        let index = fixture_index();
        let policy = serde_json::json!({});
        let metadata = metadata_for(GUID, 3, &policy, 42, 100);
        let mut disk = Cursor::new(Vec::new());
        let stats = encode_cache(&index, &metadata, &limits(), &mut disk).unwrap();
        let bytes = disk.into_inner();
        let restore = |data: &[u8], disk_bytes, expected: &CacheIdentity, budget| {
            load_bounded_cache(data, &limits(), expected, disk_bytes, budget)
        };
        let budget = 16 * 1024 * 1024;
        let loaded = restore(&bytes, stats.file_bytes, metadata.identity(), budget)
            .unwrap()
            .unwrap();
        assert_eq!(snapshot(&loaded.index), snapshot(&index));
        assert!(matches!(
            restore(&bytes, stats.file_bytes, metadata.identity(), 0),
            Err(CacheError::Quota)
        ));
        assert!(matches!(
            restore(
                &bytes,
                stats.file_bytes,
                metadata.identity(),
                DECODE_SCRATCH_BYTES
            ),
            Err(CacheError::Quota)
        ));
        let mismatch = identity(GUID, 4, &policy);
        assert!(
            restore(&bytes, stats.file_bytes, &mismatch, budget)
                .unwrap()
                .is_none()
        );
        assert!(matches!(
            restore(&bytes, stats.file_bytes + 1, metadata.identity(), budget),
            Err(CacheError::Corrupt(_))
        ));
        let mut corrupted = bytes;
        corrupted[CRC_OFFSET] ^= 1;
        assert!(matches!(
            restore(&corrupted, stats.file_bytes, metadata.identity(), budget),
            Err(CacheError::Corrupt("payload CRC32 mismatch"))
        ));
    }

    fn fixture_index() -> VolumeIndex {
        let mut index = VolumeIndex::new(NTFS_ROOT_FRN, FrnMap::sparse());
        let folder = row(
            0x0001_0000_0000_002a,
            ROOT,
            b"Work",
            true,
            None,
            Some(132_100),
        );
        let first = row(
            0x0001_0000_0000_0010,
            folder.frn,
            b"report-a.txt",
            false,
            Some(42),
            Some(132_200),
        );
        let second = row(
            first.frn,
            folder.frn,
            b"report-b.txt",
            false,
            Some(42),
            Some(132_200),
        );
        let unusual = row(
            0x0001_0000_0000_0011,
            folder.frn,
            b"ill-\xed\xa0\x80.txt",
            false,
            None,
            None,
        );
        insert_row(&mut index, &folder);
        index.insert_layout(&LayoutFile {
            frn: Frn(first.frn),
            attributes: 0,
            size: first.size,
            mtime_ft: first.mtime,
            ctime_ft: None,
            atime_ft: None,
            basic_attributes: None,
            names: [first, second]
                .into_iter()
                .map(|row| LayoutName {
                    parent_frn: Frn(row.parent_frn),
                    name: row.name,
                    name_lossy: row.name_lossy,
                    dos_only: false,
                })
                .collect(),
        });
        insert_row(&mut index, &unusual);
        index
    }

    fn snapshot(index: &VolumeIndex) -> Vec<(u64, u64, Vec<u8>, bool, Option<u64>, Option<i64>)> {
        let mut rows = index
            .live_entries()
            .filter(|&entry| !index.is_synthetic(entry))
            .map(|entry| {
                let view = index.entry(entry);
                let parent = index.entry(view.parent()).frn().0;
                (
                    view.frn().0,
                    parent,
                    view.name().to_vec(),
                    view.is_dir(),
                    if view.is_dir() { None } else { view.size() },
                    view.mtime(),
                )
            })
            .collect::<Vec<_>>();
        rows.sort();
        rows
    }

    fn encode_rows(rows: &[CacheRow], metadata: &CacheMetadata) -> Vec<u8> {
        let limits = limits();
        let mut writer = Cursor::new(Vec::new());
        write_header(&mut writer, metadata, 0, 0, 0).unwrap();
        let header_bytes = FIXED_HEADER_BYTES + metadata.identity.policy_key.len() as u64;
        writer.seek(SeekFrom::Start(header_bytes)).unwrap();
        let mut payload = PayloadWriter::new(&mut writer, limits.max_volume_bytes);
        for row in rows {
            write_row(&mut payload, row).unwrap();
        }
        let payload_bytes = payload.bytes;
        let crc32 = payload.crc.finish();
        drop(payload);
        let file_bytes = header_bytes + payload_bytes;
        writer.seek(SeekFrom::Start(0)).unwrap();
        write_header(
            &mut writer,
            metadata,
            rows.len() as u64,
            payload_bytes,
            crc32,
        )
        .unwrap();
        writer.seek(SeekFrom::Start(file_bytes)).unwrap();
        writer.into_inner()
    }

    fn recompute_crc(bytes: &mut [u8]) {
        let policy_len = u32::from_le_bytes(bytes[104..108].try_into().unwrap()) as usize;
        let start = FIXED_HEADER_BYTES as usize + policy_len;
        let mut crc = Crc32::new();
        crc.update(&bytes[start..]);
        bytes[CRC_OFFSET..CRC_OFFSET + 4].copy_from_slice(&crc.finish().to_le_bytes());
    }

    fn fixture_metadata() -> CacheMetadata {
        let policy = serde_json::json!({
            "include_roots": [],
            "exclude_roots": [],
            "monitor": true
        });
        metadata_for(GUID, 0x1234, &policy, 0x9999, 500)
    }

    #[test]
    fn codec_round_trips_hardlinks_and_wtf8_names() {
        let index = fixture_index();
        let metadata = fixture_metadata();
        let stats = encode_cache(&index, &metadata, &limits(), Cursor::new(Vec::new())).unwrap();
        let encoded = encode_cache_bytes(&index, &metadata);
        assert_eq!(stats.file_bytes, encoded.len() as u64);
        assert_eq!(stats.rows, 4);
        let decoded = decode_cache(Cursor::new(encoded), &limits()).unwrap();
        assert_eq!(decoded.metadata, metadata);
        let rows = snapshot(&decoded.index);
        assert_eq!(rows, snapshot(&index));
        let first = rows.iter().find(|row| row.2 == b"report-a.txt").unwrap();
        let second = rows.iter().find(|row| row.2 == b"report-b.txt").unwrap();
        assert_eq!(first.0, second.0);
        assert!(rows.iter().any(|row| row.2 == b"ill-\xed\xa0\x80.txt"));

        let mut first = row(first.0, first.1, &first.2, false, first.4, first.5);
        let second = row(second.0, second.1, &second.2, false, second.4, second.5);
        let parent = row(first.parent_frn, ROOT, b"Work", true, None, Some(132_100));
        first.size = Some(43);
        let inconsistent = encode_rows(&[first, parent, second], &metadata);
        assert!(matches!(
            decode_cache(Cursor::new(inconsistent), &limits()),
            Err(CacheError::Corrupt("inconsistent rows for one FRN"))
        ));
    }

    fn encode_cache_bytes(index: &VolumeIndex, metadata: &CacheMetadata) -> Vec<u8> {
        let mut writer = Cursor::new(Vec::new());
        encode_cache(index, metadata, &limits(), &mut writer).unwrap();
        writer.into_inner()
    }

    #[test]
    fn decoder_restores_a_child_before_its_parent() {
        let metadata = fixture_metadata();
        let parent = row(0x0001_0000_0000_0050, ROOT, b"Parent", true, None, Some(10));
        let child = row(
            0x0001_0000_0000_0051,
            parent.frn,
            b"child.txt",
            false,
            Some(8),
            Some(11),
        );
        let bytes = encode_rows(&[child.clone(), parent.clone()], &metadata);
        let decoded = decode_cache(Cursor::new(bytes), &limits()).unwrap();
        assert_eq!(
            snapshot(&decoded.index),
            vec![
                (parent.frn, ROOT, parent.name, true, None, Some(10)),
                (
                    child.frn,
                    parent.frn,
                    child.name.clone(),
                    false,
                    Some(8),
                    Some(11)
                ),
            ]
        );

        let file_parent = row(parent.frn, ROOT, b"not-a-directory", false, None, None);
        for rows in [[child.clone(), file_parent.clone()], [file_parent, child]] {
            let bytes = encode_rows(&rows, &metadata);
            assert!(matches!(
                decode_cache(Cursor::new(bytes), &limits()),
                Err(CacheError::Corrupt("file used as a parent directory"))
            ));
        }
    }

    #[test]
    fn decoder_rejects_cycles_and_duplicate_names() {
        let metadata = fixture_metadata();
        let first = row(
            0x0001_0000_0000_0060,
            0x0001_0000_0000_0061,
            b"one",
            true,
            None,
            None,
        );
        let second = row(0x0001_0000_0000_0061, first.frn, b"two", true, None, None);
        assert!(matches!(
            decode_cache(
                Cursor::new(encode_rows(&[first.clone(), second.clone()], &metadata)),
                &limits()
            ),
            Err(CacheError::Corrupt(_))
        ));

        let first = row(
            0x0001_0000_0000_0070,
            ROOT,
            b"same.txt",
            false,
            Some(1),
            None,
        );
        let second = row(
            0x0001_0000_0000_0071,
            ROOT,
            b"same.txt",
            false,
            Some(2),
            None,
        );
        assert!(matches!(
            decode_cache(
                Cursor::new(encode_rows(&[first, second], &metadata)),
                &limits()
            ),
            Err(CacheError::Corrupt(_))
        ));
    }

    #[test]
    fn decoder_rejects_reused_frn_slots_and_duplicate_rows() {
        let metadata = fixture_metadata();
        let slot_a = (1u64 << 48) | 80;
        let slot_b = (2u64 << 48) | 80;
        let first = row(slot_a, ROOT, b"old.txt", false, Some(1), None);
        let second = row(slot_b, ROOT, b"new.txt", false, Some(2), None);
        assert!(matches!(
            decode_cache(
                Cursor::new(encode_rows(&[first.clone(), second], &metadata)),
                &limits()
            ),
            Err(CacheError::Corrupt(_))
        ));
        assert!(matches!(
            decode_cache(
                Cursor::new(encode_rows(&[first.clone(), first], &metadata)),
                &limits()
            ),
            Err(CacheError::Corrupt(_))
        ));
    }

    #[test]
    fn decoder_rejects_malformed_headers_payloads_and_names() {
        let metadata = fixture_metadata();
        let valid = encode_cache_bytes(&fixture_index(), &metadata);

        let mut bad_magic = valid.clone();
        bad_magic[0] ^= 0xff;
        assert!(matches!(
            decode_cache(Cursor::new(bad_magic), &limits()),
            Err(CacheError::Corrupt(_))
        ));

        let mut truncated = valid.clone();
        truncated.truncate(truncated.len() - 1);
        assert!(matches!(
            decode_cache(Cursor::new(truncated), &limits()),
            Err(CacheError::Corrupt(_))
        ));

        let mut trailing = valid.clone();
        trailing.push(0);
        assert!(matches!(
            decode_cache(Cursor::new(trailing), &limits()),
            Err(CacheError::Corrupt(_))
        ));

        let mut bad_crc = valid.clone();
        bad_crc[CRC_OFFSET] ^= 1;
        assert!(matches!(
            decode_cache(Cursor::new(bad_crc), &limits()),
            Err(CacheError::Corrupt(_))
        ));

        let mut bad_flags = valid.clone();
        let payload_start = FIXED_HEADER_BYTES as usize
            + u32::from_le_bytes(valid[104..108].try_into().unwrap()) as usize;
        bad_flags[payload_start + 16] |= 0x80;
        recompute_crc(&mut bad_flags);
        assert!(matches!(
            decode_cache(Cursor::new(bad_flags), &limits()),
            Err(CacheError::Corrupt(_))
        ));

        let mut bad_name_len = valid.clone();
        bad_name_len[payload_start + 17..payload_start + 21]
            .copy_from_slice(&u32::MAX.to_le_bytes());
        recompute_crc(&mut bad_name_len);
        assert!(matches!(
            decode_cache(Cursor::new(bad_name_len), &limits()),
            Err(CacheError::Corrupt(_))
        ));

        let invalid_name = row(0x0001_0000_0000_0090, ROOT, &[0xff], false, None, None);
        assert!(matches!(
            decode_cache(
                Cursor::new(encode_rows(&[invalid_name], &metadata)),
                &limits()
            ),
            Err(CacheError::Corrupt(_))
        ));

        let mut unsupported_schema = valid;
        unsupported_schema[8..12].copy_from_slice(&99u32.to_le_bytes());
        assert!(matches!(
            decode_cache(Cursor::new(unsupported_schema), &limits()),
            Err(CacheError::Corrupt(_))
        ));
    }

    #[test]
    fn decoder_enforces_row_and_file_limits_and_crc32_standard_vector() {
        let metadata = fixture_metadata();
        let valid = encode_cache_bytes(&fixture_index(), &metadata);
        let small_limits = CacheLimits::new(1024, 1024, 2, 1).unwrap();
        assert!(matches!(
            decode_cache(Cursor::new(valid.clone()), &small_limits),
            Err(CacheError::Quota)
        ));

        let mut oversized_payload = valid;
        oversized_payload[92..100].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(matches!(
            decode_cache(Cursor::new(oversized_payload), &limits()),
            Err(CacheError::Quota)
        ));

        let mut crc = Crc32::new();
        crc.update(b"123456789");
        assert_eq!(crc.finish(), 0xcbf4_3926);
    }

    #[test]
    fn profile_store_round_trips_and_rejects_scope_or_volume_mismatch() {
        let profile = TempDir::new().unwrap();
        let mut store = CacheStore::open_at(profile.path(), limits()).unwrap();
        let index = fixture_index();
        let metadata = fixture_metadata();
        let stats = store.save(&index, &metadata).unwrap();
        let loaded = store.load(metadata.identity()).unwrap().unwrap();
        assert_eq!(loaded.metadata, metadata);
        assert_eq!(snapshot(&loaded.index), snapshot(&index));
        assert_eq!(store.usage().unwrap().total_bytes, stats.file_bytes);

        let policy = serde_json::json!({
            "include_roots": ["different"],
            "exclude_roots": [],
            "monitor": true
        });
        let mismatch = identity(GUID, 0x1234, &policy);
        assert!(store.load(&mismatch).unwrap().is_none());

        let same_policy = serde_json::json!({
            "include_roots": [],
            "exclude_roots": [],
            "monitor": true
        });
        let wrong_serial = identity(GUID, 0x1235, &same_policy);
        assert!(store.load(&wrong_serial).unwrap().is_none());

        let other_guid = identity("bbbbbbbb-cccc-dddd-eeee-ffffffffffff", 0x1234, &same_policy);
        assert!(store.load(&other_guid).unwrap().is_none());
    }

    #[test]
    fn profile_lock_is_exclusive_and_released_on_close() {
        let profile = TempDir::new().unwrap();
        let first = CacheStore::open_at(profile.path(), limits()).unwrap();
        assert!(matches!(
            CacheStore::open_at(profile.path(), limits()),
            Err(CacheError::Busy)
        ));
        drop(first);
        assert!(CacheStore::open_at(profile.path(), limits()).is_ok());
    }

    #[test]
    fn interrupted_candidate_keeps_the_previous_commit_and_success_replaces_it() {
        let profile = TempDir::new().unwrap();
        let mut store = CacheStore::open_at(profile.path(), limits()).unwrap();
        let index = fixture_index();
        let old = fixture_metadata();
        store.save(&index, &old).unwrap();
        let target = store.cache_path(old.identity().volume_guid());
        let before = fs::read(&target).unwrap();

        let newer =
            CacheMetadata::new(old.identity().clone(), old.journal_id(), 700, 2000).unwrap();
        let failed = store.save_using(&newer, |file, _| {
            file.write_all(b"partial-candidate")
                .map_err(CacheError::Io)?;
            Err(CacheError::Corrupt("injected interrupted candidate"))
        });
        assert!(matches!(failed, Err(CacheError::Corrupt(_))));
        assert_eq!(fs::read(&target).unwrap(), before);
        assert_eq!(store.usage().unwrap().temps, 0);

        store.save(&index, &newer).unwrap();
        let loaded = store.load(newer.identity()).unwrap().unwrap();
        assert_eq!(loaded.metadata, newer);
        assert_eq!(snapshot(&loaded.index), snapshot(&index));
    }

    #[test]
    fn actual_disk_bytes_enforce_total_and_per_volume_quotas() {
        let index = fixture_index();
        let metadata = fixture_metadata();
        let encoded_size = encode_cache_bytes(&index, &metadata).len() as u64;

        let total_profile = TempDir::new().unwrap();
        let total_limit = encoded_size * 2 - 1;
        let total_limits = CacheLimits::new(total_limit, total_limit, 32, 100_000).unwrap();
        let mut store = CacheStore::open_at(total_profile.path(), total_limits).unwrap();
        store.save(&index, &metadata).unwrap();
        let path = store.cache_path(metadata.identity().volume_guid());
        let committed = fs::read(&path).unwrap();
        let policy: serde_json::Value =
            serde_json::from_slice(&metadata.identity().policy_key).unwrap();
        let second_metadata = metadata_for(
            "bbbbbbbb-cccc-dddd-eeee-ffffffffffff",
            0x5678,
            &policy,
            0x7777,
            800,
        );
        let second_size = encode_cache_bytes(&index, &second_metadata).len() as u64;
        assert_eq!(committed.len() as u64, encoded_size);
        assert_eq!(second_size, encoded_size);
        assert!(second_size <= total_limits.max_volume_bytes);
        assert_eq!(encoded_size + second_size, total_limits.max_total_bytes + 1);
        assert!(matches!(
            store.save(&index, &second_metadata),
            Err(CacheError::Quota)
        ));
        assert_eq!(fs::read(&path).unwrap(), committed);
        assert_eq!(store.usage().unwrap().files, 1);
        assert_eq!(store.usage().unwrap().temps, 0);

        let volume_profile = TempDir::new().unwrap();
        let volume_limits =
            CacheLimits::new(encoded_size * 2, encoded_size - 1, 32, 100_000).unwrap();
        let mut volume_store = CacheStore::open_at(volume_profile.path(), volume_limits).unwrap();
        assert!(matches!(
            volume_store.save(&index, &metadata),
            Err(CacheError::Quota)
        ));
        assert!(volume_store.load(metadata.identity()).unwrap().is_none());
    }

    #[test]
    fn file_limit_counts_only_committed_known_cache_files() {
        let profile = TempDir::new().unwrap();
        let index = fixture_index();
        let metadata = fixture_metadata();
        let constrained = CacheLimits::new(1024 * 1024, 1024 * 1024, 1, 100_000).unwrap();
        let mut store = CacheStore::open_at(profile.path(), constrained).unwrap();
        store.save(&index, &metadata).unwrap();
        let second = metadata_for(
            "bbbbbbbb-cccc-dddd-eeee-ffffffffffff",
            0x5678,
            &serde_json::json!({"scope": "all"}),
            0x7777,
            800,
        );
        assert!(matches!(
            store.save(&index, &second),
            Err(CacheError::Quota)
        ));
        assert!(store.load(metadata.identity()).unwrap().is_some());
        assert_eq!(store.usage().unwrap().files, 1);
    }

    #[cfg(unix)]
    #[test]
    fn profile_and_cache_links_are_rejected_without_touching_the_target() {
        use std::os::unix::fs::symlink;

        let profile = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let outside_file = outside.path().join("keep.bin");
        fs::write(&outside_file, b"preserve").unwrap();
        symlink(outside.path(), profile.path().join(CACHE_SUBDIR)).unwrap();
        assert!(matches!(
            CacheStore::open_at(profile.path(), limits()),
            Err(CacheError::UnsafePath)
        ));
        assert_eq!(fs::read(&outside_file).unwrap(), b"preserve");

        let second_profile = TempDir::new().unwrap();
        let mut store = CacheStore::open_at(second_profile.path(), limits()).unwrap();
        let metadata = fixture_metadata();
        let link_path = store.cache_path(metadata.identity().volume_guid());
        symlink(&outside_file, &link_path).unwrap();
        assert!(matches!(
            store.load(metadata.identity()),
            Err(CacheError::UnsafePath)
        ));
        assert!(matches!(
            store.purge(metadata.identity()),
            Err(CacheError::UnsafePath)
        ));
        assert!(matches!(
            store.save(&fixture_index(), &metadata),
            Err(CacheError::UnsafePath)
        ));
        assert!(
            fs::symlink_metadata(&link_path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read(&outside_file).unwrap(), b"preserve");
    }

    #[cfg(unix)]
    #[test]
    fn profile_root_and_lock_links_are_rejected() {
        use std::os::unix::fs::symlink;

        let profile = TempDir::new().unwrap();
        let alias = profile.path().join("alias");
        symlink(profile.path(), &alias).unwrap();
        assert!(matches!(
            CacheStore::open_at(&alias, limits()),
            Err(CacheError::UnsafePath)
        ));

        let second_profile = TempDir::new().unwrap();
        let cache_root = second_profile.path().join(CACHE_SUBDIR);
        fs::create_dir(&cache_root).unwrap();
        let outside = TempDir::new().unwrap();
        let outside_lock = outside.path().join("external-lock");
        fs::write(&outside_lock, b"untouched").unwrap();
        symlink(&outside_lock, cache_root.join(LOCK_FILE)).unwrap();
        assert!(matches!(
            CacheStore::open_at(second_profile.path(), limits()),
            Err(CacheError::UnsafePath)
        ));
        assert_eq!(fs::read(&outside_lock).unwrap(), b"untouched");
    }

    #[test]
    fn stale_owned_temporary_files_are_cleaned_and_unknown_files_are_preserved() {
        let profile = TempDir::new().unwrap();
        let metadata = fixture_metadata();
        let temporary;
        let unknown;
        {
            let store = CacheStore::open_at(profile.path(), limits()).unwrap();
            temporary = store.new_temp_path(metadata.identity().volume_guid());
            create_new_file(&temporary).unwrap();
            unknown = store.directory.join("user-note.txt");
            fs::write(&unknown, b"leave this file alone").unwrap();
        }
        let mut reopened = CacheStore::open_at(profile.path(), limits()).unwrap();
        assert!(!temporary.exists());
        assert!(unknown.exists());
        assert!(!reopened.purge(metadata.identity()).unwrap());
        assert_eq!(fs::read(&unknown).unwrap(), b"leave this file alone");
    }
}
