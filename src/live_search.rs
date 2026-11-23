//! Optional, read-only NTFS search. The file manager itself remains unelevated.
use crate::{
    index_policy::VolumeGuid,
    model::NativePath,
    search::SearchResult,
    worker::{Event, Request},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Indexing,
    CatchingUp,
    Live,
    Offline,
    Excluded,
    Error,
    Unsupported,
    Unavailable,
    Snapshot,
    Limited,
}
impl Phase {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Indexing => "초기 색인 중",
            Self::CatchingUp => "변경분 반영 중",
            Self::Live => "실시간 추적 중",
            Self::Offline => "연결 끊김",
            Self::Excluded => "검색 제외",
            Self::Error => "사용 불가",
            Self::Unsupported => "고속 색인 미지원",
            Self::Unavailable => "조회 불가",
            Self::Snapshot => "스냅샷만",
            Self::Limited => "상한 도달",
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VolumeStatus {
    pub root: NativePath,
    pub phase: Phase,
    pub entries: usize,
    pub allocated_bytes: u64,
    pub message: Option<String>,
    #[serde(default)]
    pub guid: Option<VolumeGuid>,
    #[serde(default)]
    pub serial: Option<u32>,
    #[serde(default)]
    pub mount: Option<NativePath>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InventoryClass {
    Fixed,
    Removable,
    Unsupported,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InventoryVolume {
    pub guid: VolumeGuid,
    pub serial: Option<u32>,
    pub mounts: Vec<NativePath>,
    pub classification: InventoryClass,
    pub message: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MountedInventory {
    pub volumes: Vec<InventoryVolume>,
    pub network_drives: Vec<NativePath>,
}

const MAX_COMMAND_FRAME_BYTES: usize = 128 * 1024;
const MAX_QUERY_FRAME_BYTES: usize = 32_000;
const MAX_PENDING_CONTROLS: usize = 8;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum HostCommand {
    Configure {
        session: u64,
        generation: u64,
        settings: Box<crate::index_policy::NtfsIndexingSettings>,
    },
    Rebuild {
        session: u64,
        generation: u64,
        request_id: u64,
        guid: crate::index_policy::VolumeGuid,
    },
    Query {
        session: u64,
        generation: u64,
        query_id: u64,
        text: String,
    },
    CacheLoadBegin {
        session: u64,
        generation: u64,
        transfer_id: u64,
        guid: crate::index_policy::VolumeGuid,
        total_bytes: u64,
    },
    CacheLoadChunk {
        session: u64,
        generation: u64,
        transfer_id: u64,
        guid: crate::index_policy::VolumeGuid,
        sequence: u32,
        offset: u64,
        data: String,
    },
    CacheLoadEnd {
        session: u64,
        generation: u64,
        transfer_id: u64,
        guid: crate::index_policy::VolumeGuid,
        chunks: u32,
        total_bytes: u64,
    },
    CacheLoadUnavailable {
        session: u64,
        generation: u64,
        transfer_id: u64,
        guid: crate::index_policy::VolumeGuid,
        reason: CacheReason,
    },
    CacheExportReady {
        session: u64,
        generation: u64,
        transfer_id: u64,
        guid: crate::index_policy::VolumeGuid,
        accepted: bool,
        reason: Option<CacheReason>,
    },
    CacheExportChunkAck {
        session: u64,
        generation: u64,
        transfer_id: u64,
        guid: crate::index_policy::VolumeGuid,
        sequence: u32,
        accepted: bool,
        reason: Option<CacheReason>,
    },
    CacheExportResult {
        session: u64,
        generation: u64,
        transfer_id: u64,
        guid: crate::index_policy::VolumeGuid,
        accepted: bool,
        reason: Option<CacheReason>,
    },
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CacheReason {
    Disabled,
    Missing,
    Incompatible,
    Corrupt,
    Unavailable,
    Quota,
    UnsafeLocation,
    TooOld,
    FutureTimestamp,
    JournalChanged,
    CursorOutsideWindow,
    FutureCursor,
    Cancelled,
    ExportTimeout,
    MemoryLimit,
}

impl CacheReason {
    pub fn message(self) -> &'static str {
        match self {
            Self::Disabled => "영구 캐시가 꺼져 있어 전체 스캔을 시작합니다.",
            Self::Missing => "저장 캐시가 없어 전체 스캔을 시작합니다.",
            Self::Incompatible => "저장 캐시의 볼륨 또는 정책이 달라 전체 스캔을 시작합니다.",
            Self::Corrupt => "저장 캐시를 검증하지 못해 전체 스캔을 시작합니다.",
            Self::Unavailable => "저장 위치를 사용할 수 없어 전체 스캔을 시작합니다.",
            Self::Quota => "캐시 저장 상한에 도달해 기존 캐시를 유지합니다.",
            Self::UnsafeLocation => "캐시 위치 소유권을 확인할 수 없어 전체 스캔을 시작합니다.",
            Self::TooOld => "저장 캐시가 7일보다 오래되어 전체 스캔을 시작합니다.",
            Self::FutureTimestamp => "저장 캐시 시각이 유효하지 않아 전체 스캔을 시작합니다.",
            Self::JournalChanged => "저장 캐시의 USN 저널이 달라 전체 스캔을 시작합니다.",
            Self::CursorOutsideWindow => {
                "저장 커서가 현재 USN 창 밖에 있어 전체 스캔을 시작합니다."
            }
            Self::FutureCursor => "저장 커서가 현재 저널보다 앞서 있어 전체 스캔을 시작합니다.",
            Self::Cancelled => "캐시 작업이 취소되어 기존 캐시를 유지합니다.",
            Self::ExportTimeout => "캐시 내보내기가 5초 안에 끝나지 않아 기존 캐시를 유지합니다.",
            Self::MemoryLimit => "저장 캐시가 메모리 상한을 넘어 전체 스캔을 시작합니다.",
        }
    }
}

pub(super) const CACHE_TRANSFER_CHUNK_BYTES: usize = 12 * 1024;
const CACHE_TRANSFER_B64_BYTES: usize = CACHE_TRANSFER_CHUNK_BYTES.div_ceil(3) * 4;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "frame", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum CacheFrame {
    LoadRequest {
        session: u64,
        generation: u64,
        transfer_id: u64,
        guid: crate::index_policy::VolumeGuid,
        serial: u32,
    },
    LoadChunkAck {
        session: u64,
        generation: u64,
        transfer_id: u64,
        guid: crate::index_policy::VolumeGuid,
        sequence: u32,
        accepted: bool,
        reason: Option<CacheReason>,
    },
    LoadAbort {
        session: u64,
        generation: u64,
        transfer_id: u64,
        guid: crate::index_policy::VolumeGuid,
        reason: CacheReason,
    },
    ExportStart {
        session: u64,
        generation: u64,
        transfer_id: u64,
        guid: crate::index_policy::VolumeGuid,
        serial: u32,
        journal_id: u64,
        cursor: i64,
        created_at_ms: u64,
    },
    ExportChunk {
        session: u64,
        generation: u64,
        transfer_id: u64,
        guid: crate::index_policy::VolumeGuid,
        sequence: u32,
        offset: u64,
        data: String,
    },
    ExportEnd {
        session: u64,
        generation: u64,
        transfer_id: u64,
        guid: crate::index_policy::VolumeGuid,
        chunks: u32,
        total_bytes: u64,
    },
    ExportAbort {
        session: u64,
        generation: u64,
        transfer_id: u64,
        guid: crate::index_policy::VolumeGuid,
        reason: CacheReason,
    },
}

pub(super) fn encode_cache_chunk(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let a = chunk[0];
        let b = *chunk.get(1).unwrap_or(&0);
        let c = *chunk.get(2).unwrap_or(&0);
        out.push(TABLE[usize::from(a >> 2)] as char);
        out.push(TABLE[usize::from(((a & 0x03) << 4) | (b >> 4))] as char);
        out.push(if chunk.len() > 1 {
            TABLE[usize::from(((b & 0x0f) << 2) | (c >> 6))] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[usize::from(c & 0x3f)] as char
        } else {
            '='
        });
    }
    out
}

pub(super) fn decode_cache_chunk(encoded: &str) -> Result<Vec<u8>, CacheReason> {
    if encoded.is_empty() || encoded.len() > CACHE_TRANSFER_B64_BYTES || encoded.len() % 4 != 0 {
        return Err(CacheReason::Corrupt);
    }
    let value = |byte: u8| -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    };
    let bytes = encoded.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for (index, block) in bytes.chunks_exact(4).enumerate() {
        let last = index + 1 == bytes.len() / 4;
        let a = value(block[0]).ok_or(CacheReason::Corrupt)?;
        let b = value(block[1]).ok_or(CacheReason::Corrupt)?;
        let c = if block[2] == b'=' && last {
            0
        } else {
            value(block[2]).ok_or(CacheReason::Corrupt)?
        };
        let d = if block[3] == b'=' && last {
            0
        } else {
            value(block[3]).ok_or(CacheReason::Corrupt)?
        };
        if (!last && (block[2] == b'=' || block[3] == b'='))
            || (block[2] == b'=' && block[3] != b'=')
            || (block[2] == b'=' && b & 0x0f != 0)
            || (block[3] == b'=' && block[2] != b'=' && c & 0x03 != 0)
        {
            return Err(CacheReason::Corrupt);
        }
        out.push((a << 2) | (b >> 4));
        if block[2] != b'=' {
            out.push((b << 4) | (c >> 2));
        }
        if block[3] != b'=' {
            out.push((c << 6) | d);
        }
    }
    if out.is_empty() || out.len() > CACHE_TRANSFER_CHUNK_BYTES {
        return Err(CacheReason::Corrupt);
    }
    Ok(out)
}

pub(super) fn cache_identity(
    settings: &crate::index_policy::NtfsIndexingSettings,
    guid: &crate::index_policy::VolumeGuid,
    serial: u32,
) -> Result<crate::index_cache::CacheIdentity, CacheReason> {
    let value = serde_json::to_value(settings).map_err(|_| CacheReason::Incompatible)?;
    let mut policy = serde_json::Map::new();
    for key in ["auto_fixed", "auto_removable", "offline", "persistent"] {
        policy.insert(
            key.to_string(),
            value.get(key).cloned().ok_or(CacheReason::Incompatible)?,
        );
    }
    let volume_policy = match settings.volume_policy(guid) {
        Some(policy) => serde_json::to_value(policy).map_err(|_| CacheReason::Incompatible)?,
        None => serde_json::Value::Null,
    };
    policy.insert("volume".into(), volume_policy);
    crate::index_cache::CacheIdentity::new(
        guid.as_str(),
        u64::from(serial),
        &serde_json::Value::Object(policy),
    )
    .map_err(|_| CacheReason::Incompatible)
}

static CACHE_TRANSFER_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

pub(super) fn next_cache_transfer_id() -> u64 {
    CACHE_TRANSFER_SEQUENCE
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        .max(1)
}

impl HostCommand {
    fn validate(&self) -> Result<(), String> {
        match self {
            Self::Configure {
                session, settings, ..
            } => {
                if *session == 0 {
                    return Err("잘못된 검색 세션 ID입니다.".into());
                }
                if !settings.is_valid() {
                    return Err("검색 설정이 올바르지 않습니다.".into());
                }
            }
            Self::Rebuild {
                session,
                request_id,
                ..
            } => {
                if *session == 0 || *request_id == 0 {
                    return Err("잘못된 재구성 명령 ID입니다.".into());
                }
            }
            Self::Query {
                session,
                query_id,
                text,
                ..
            } => {
                if *session == 0 || *query_id == 0 {
                    return Err("잘못된 검색 명령 ID입니다.".into());
                }
                crate::search::validate_query_text(text)?;
            }
            Self::CacheLoadBegin {
                session,
                transfer_id,
                total_bytes,
                ..
            }
            | Self::CacheLoadEnd {
                session,
                transfer_id,
                total_bytes,
                ..
            } => {
                validate_cache_ids(*session, *transfer_id)?;
                if *total_bytes == 0
                    || *total_bytes > crate::index_cache::CacheLimits::default().max_volume_bytes
                {
                    return Err("캐시 전송 크기가 허용 범위를 벗어났습니다.".into());
                }
            }
            Self::CacheLoadChunk {
                session,
                transfer_id,
                offset,
                data,
                ..
            } => {
                validate_cache_ids(*session, *transfer_id)?;
                let chunk =
                    decode_cache_chunk(data).map_err(|_| "캐시 chunk가 올바르지 않습니다.")?;
                if offset.checked_add(chunk.len() as u64).is_none_or(|end| {
                    end > crate::index_cache::CacheLimits::default().max_volume_bytes
                }) {
                    return Err("캐시 chunk 오프셋이 허용 범위를 벗어났습니다.".into());
                }
            }
            Self::CacheLoadUnavailable {
                session,
                transfer_id,
                ..
            }
            | Self::CacheExportReady {
                session,
                transfer_id,
                ..
            }
            | Self::CacheExportChunkAck {
                session,
                transfer_id,
                ..
            }
            | Self::CacheExportResult {
                session,
                transfer_id,
                ..
            } => {
                validate_cache_ids(*session, *transfer_id)?;
            }
        }
        Ok(())
    }

    fn is_query(&self) -> bool {
        matches!(self, Self::Query { .. })
    }

    fn is_cache_reply(&self) -> bool {
        matches!(
            self,
            Self::CacheExportReady { .. }
                | Self::CacheExportChunkAck { .. }
                | Self::CacheExportResult { .. }
        )
    }
}

fn validate_cache_ids(session: u64, transfer_id: u64) -> Result<(), String> {
    if session == 0 || transfer_id == 0 {
        Err("잘못된 캐시 전송 ID입니다.".into())
    } else {
        Ok(())
    }
}

#[derive(Debug)]
pub(super) struct HostCommandReadError {
    pub(super) command: Option<HostCommand>,
    pub(super) message: String,
}

pub(super) fn same_generation(
    expected_session: u64,
    expected_generation: u64,
    session: u64,
    generation: u64,
) -> bool {
    session == expected_session && generation == expected_generation
}

#[cfg(any(windows, test))]
fn read_query(
    reader: &mut impl std::io::BufRead,
) -> Result<Option<HostCommand>, HostCommandReadError> {
    use std::io::{BufRead, Read};
    let mut bytes = Vec::new();
    let read = (&mut *reader)
        .take((MAX_COMMAND_FRAME_BYTES + 1) as u64)
        .read_until(b'\n', &mut bytes)
        .map_err(|e| HostCommandReadError {
            command: None,
            message: e.to_string(),
        })?;
    if read == 0 {
        return Ok(None);
    }
    if bytes.len() > MAX_COMMAND_FRAME_BYTES || bytes.last() != Some(&b'\n') {
        return Err(HostCommandReadError {
            command: None,
            message: "검색 명령 프레임 크기 또는 줄 끝이 올바르지 않습니다.".into(),
        });
    }
    let line = String::from_utf8(bytes).map_err(|e| HostCommandReadError {
        command: None,
        message: format!("검색 명령이 UTF-8이 아닙니다: {e}"),
    })?;
    let command: HostCommand = serde_json::from_str(&line).map_err(|e| HostCommandReadError {
        command: None,
        message: format!("검색 명령 형식이 올바르지 않습니다: {e}"),
    })?;
    let frame_limit = if command.is_query() {
        MAX_QUERY_FRAME_BYTES
    } else {
        MAX_COMMAND_FRAME_BYTES
    };
    if line.len() > frame_limit {
        return Err(HostCommandReadError {
            command: Some(command),
            message: "검색 명령 프레임이 허용 크기를 넘었습니다.".into(),
        });
    }
    if let Err(message) = command.validate() {
        return Err(HostCommandReadError {
            command: Some(command),
            message,
        });
    }
    Ok(Some(command))
}

#[cfg(any(windows, test))]
pub(super) fn write_command(
    writer: &mut impl std::io::Write,
    command: &HostCommand,
) -> Result<(), String> {
    command.validate()?;
    let mut frame = serde_json::to_vec(command).map_err(|e| e.to_string())?;
    frame.push(b'\n');
    let limit = if command.is_query() {
        MAX_QUERY_FRAME_BYTES
    } else {
        MAX_COMMAND_FRAME_BYTES
    };
    if frame.len() > limit {
        return Err("검색 명령 프레임이 허용 크기를 넘었습니다.".into());
    }
    writer
        .write_all(&frame)
        .and_then(|_| writer.flush())
        .map_err(|e| e.to_string())
}

#[cfg(any(windows, test))]
pub(super) struct CommandInbox {
    pending: std::sync::Mutex<std::collections::VecDeque<HostCommand>>,
    closed: std::sync::atomic::AtomicBool,
    cache_reply_tx: crossbeam_channel::Sender<HostCommand>,
    cache_reply_rx: crossbeam_channel::Receiver<HostCommand>,
}

#[cfg(any(windows, test))]
impl Default for CommandInbox {
    fn default() -> Self {
        let (cache_reply_tx, cache_reply_rx) = crossbeam_channel::unbounded();
        Self {
            pending: std::sync::Mutex::default(),
            closed: std::sync::atomic::AtomicBool::default(),
            cache_reply_tx,
            cache_reply_rx,
        }
    }
}

#[cfg(any(windows, test))]
impl CommandInbox {
    pub(super) fn enqueue(&self, command: HostCommand) -> Result<Option<HostCommand>, HostCommand> {
        if command.is_cache_reply() {
            return self
                .cache_reply_tx
                .send(command)
                .map(|()| None)
                .map_err(|error| error.0);
        }
        let mut pending = self.pending.lock().unwrap();
        let replaced = if command.is_query() {
            pending
                .iter()
                .position(HostCommand::is_query)
                .and_then(|position| pending.remove(position))
        } else {
            let controls = pending.iter().filter(|item| !item.is_query()).count();
            if controls >= MAX_PENDING_CONTROLS {
                return Err(command);
            }
            None
        };
        pending.push_back(command);
        Ok(replaced)
    }

    pub(super) fn pop_batch(&self, limit: usize) -> Vec<HostCommand> {
        let mut pending = self.pending.lock().unwrap();
        (0..limit).filter_map(|_| pending.pop_front()).collect()
    }

    pub(super) fn close(&self) {
        self.closed
            .store(true, std::sync::atomic::Ordering::Release);
    }

    pub(super) fn cache_replies(&self) -> crossbeam_channel::Receiver<HostCommand> {
        self.cache_reply_rx.clone()
    }

    pub(super) fn closed_and_empty(&self) -> bool {
        self.closed.load(std::sync::atomic::Ordering::Acquire)
            && self.pending.lock().unwrap().is_empty()
            && self.cache_reply_rx.is_empty()
    }
}

pub(super) fn rejected_command(
    command: HostCommand,
    host_pid: Option<u32>,
    message: String,
) -> Event {
    match command {
        HostCommand::Configure {
            session,
            generation,
            ..
        } => Event::ConfigAck {
            session,
            generation,
            host_pid,
            accepted: false,
            message: Some(message),
        },
        HostCommand::Rebuild {
            session,
            generation,
            request_id,
            guid,
        } => Event::RebuildAck {
            session,
            generation,
            request_id,
            guid,
            host_pid,
            accepted: false,
            message: Some(message),
        },
        HostCommand::Query {
            session,
            generation,
            query_id,
            ..
        } => Event::VolumeSearch {
            session,
            generation,
            query_id,
            host_pid,
            result: Err(message),
        },
        _ => Event::Notice { message },
    }
}
#[cfg(test)]
fn journal_current(expected: u64, cursor: i64, actual: u64, first: i64, lowest: i64) -> bool {
    expected == actual && cursor >= first.max(lowest)
}
#[cfg(test)]
fn merge(parts: impl IntoIterator<Item = SearchResult>, complete: bool) -> SearchResult {
    merge_with_budget(parts, complete, &goz_core::query::Budget::unlimited())
        .expect("unlimited query merge")
}

#[cfg(any(windows, test))]
fn merge_with_budget(
    parts: impl IntoIterator<Item = SearchResult>,
    complete: bool,
    budget: &goz_core::query::Budget<'_>,
) -> Result<SearchResult, String> {
    const MAX_MERGED_HITS: usize = crate::index_policy::MAX_VOLUME_POLICIES * 200;
    let mut result = SearchResult {
        hits: Vec::new(),
        total: 0,
        elapsed_ms: 0.0,
        complete,
    };
    for part in parts {
        if budget.check() {
            return Err("검색 시간이 초과됐습니다. 검색 범위를 줄여 주세요.".into());
        }
        if result.hits.len().saturating_add(part.hits.len()) > MAX_MERGED_HITS {
            return Err("검색 결과 작업공간이 너무 큽니다. 검색 범위를 줄여 주세요.".into());
        }
        result.total = result
            .total
            .checked_add(part.total)
            .ok_or_else(|| "검색 결과 수가 허용 범위를 넘었습니다.".to_string())?;
        result.elapsed_ms += part.elapsed_ms;
        result.complete &= part.complete;
        for hit in part.hits {
            if budget.check() {
                return Err("검색 시간이 초과됐습니다. 검색 범위를 줄여 주세요.".into());
            }
            result.hits.push(hit);
        }
    }
    if budget.check() {
        return Err("검색 시간이 초과됐습니다. 검색 범위를 줄여 주세요.".into());
    }
    result
        .hits
        .sort_by_cached_key(|hit| (hit.name.to_lowercase(), hit.path.clone()));
    if budget.check() {
        return Err("검색 시간이 초과됐습니다. 검색 범위를 줄여 주세요.".into());
    }
    result.hits.truncate(200);
    Ok(result)
}
fn emit(event: &Event) -> Result<(), String> {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    serde_json::to_writer(&mut out, event).map_err(|e| e.to_string())?;
    out.write_all(b"\n")
        .and_then(|_| out.flush())
        .map_err(|e| e.to_string())
}
pub fn proxy(
    id: u64,
    memory_mib: u16,
    service_only: bool,
    settings: crate::index_policy::NtfsIndexingSettings,
    queued: Vec<Request>,
    requests: crossbeam_channel::Receiver<Request>,
) -> Result<(), String> {
    #[cfg(windows)]
    {
        windows::proxy(id, memory_mib, service_only, settings, queued, requests)
    }
    #[cfg(not(windows))]
    {
        let _ = (requests, memory_mib, service_only, settings, queued);
        emit(&Event::Failure {
            lane: 4,
            id,
            message: "전체 드라이브 실시간 검색은 Windows NTFS 전용입니다.".into(),
        })
    }
}
pub fn host(args: &[String]) -> Result<(), String> {
    #[cfg(windows)]
    {
        windows::host(args)
    }
    #[cfg(not(windows))]
    {
        let _ = args;
        Err("Windows NTFS 전용입니다.".into())
    }
}

#[path = "live_search_service.rs"]
pub mod service;

pub fn service_host(
    pipe_id: &str,
    parent_pid: u32,
    parent_created_filetime: u64,
    index_id: u64,
    memory_mib: u16,
) -> Result<(), String> {
    #[cfg(windows)]
    {
        windows::service_host(
            pipe_id,
            parent_pid,
            parent_created_filetime,
            index_id,
            memory_mib,
        )
    }
    #[cfg(not(windows))]
    {
        let _ = (
            pipe_id,
            parent_pid,
            parent_created_filetime,
            index_id,
            memory_mib,
        );
        Err("Windows NTFS 전용입니다.".into())
    }
}

pub fn request_service_stop() {
    #[cfg(windows)]
    windows::request_service_stop();
}

pub(crate) fn service_stop_requested() -> bool {
    #[cfg(windows)]
    {
        windows::service_stop_requested()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

pub(crate) fn reset_service_stop() {
    #[cfg(windows)]
    windows::reset_service_stop();
}
pub fn mounted_inventory() -> Result<MountedInventory, String> {
    #[cfg(windows)]
    {
        windows::mounted_inventory()
    }
    #[cfg(not(windows))]
    {
        Err("Windows NTFS 전용입니다.".into())
    }
}

pub fn result_mount_matches(guid: &VolumeGuid, mount: &NativePath) -> bool {
    #[cfg(windows)]
    {
        windows::result_mount_matches(guid, mount)
    }
    #[cfg(not(windows))]
    {
        let _ = (guid, mount);
        false
    }
}

#[cfg(windows)]
#[path = "live_search_windows.rs"]
mod windows;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::SearchHit;
    fn query(session: u64, generation: u64, query_id: u64, text: &str) -> HostCommand {
        HostCommand::Query {
            session,
            generation,
            query_id,
            text: text.into(),
        }
    }

    fn rebuild(request_id: u64) -> HostCommand {
        HostCommand::Rebuild {
            session: 7,
            generation: 0,
            request_id,
            guid: crate::index_policy::VolumeGuid::parse("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee")
                .unwrap(),
        }
    }

    #[test]
    fn bounded_host_frames_preserve_order_and_reject_invalid_commands() {
        use std::io::Cursor;
        let a = serde_json::to_string(&query(7, 0, 1, "보고서")).unwrap();
        let b = serde_json::to_string(&query(7, 0, 2, "ext:txt")).unwrap();
        let mut reader = Cursor::new(format!("{a}\n{b}\n"));
        assert!(matches!(
            read_query(&mut reader).unwrap(),
            Some(HostCommand::Query { query_id: 1, .. })
        ));
        assert!(matches!(
            read_query(&mut reader).unwrap(),
            Some(HostCommand::Query { query_id: 2, .. })
        ));
        assert!(read_query(&mut reader).unwrap().is_none());

        let max_query = serde_json::to_string(&query(7, 0, 3, "x")).unwrap();
        let exact_query = format!(
            "{}{}\n",
            " ".repeat(MAX_QUERY_FRAME_BYTES - max_query.len() - 1),
            max_query
        );
        assert_eq!(exact_query.len(), MAX_QUERY_FRAME_BYTES);
        assert!(
            read_query(&mut Cursor::new(exact_query.as_bytes()))
                .unwrap()
                .is_some()
        );
        let mut too_large_query = exact_query.as_bytes().to_vec();
        too_large_query.insert(too_large_query.len() - 1, b' ');
        assert_eq!(too_large_query.len(), MAX_QUERY_FRAME_BYTES + 1);
        assert!(read_query(&mut Cursor::new(too_large_query)).is_err());

        let configure = serde_json::to_string(&HostCommand::Configure {
            session: 7,
            generation: 0,
            settings: Box::new(crate::index_policy::NtfsIndexingSettings::default()),
        })
        .unwrap();
        let exact_control = format!(
            "{}{}\n",
            " ".repeat(MAX_COMMAND_FRAME_BYTES - configure.len() - 1),
            configure
        );
        assert_eq!(exact_control.len(), MAX_COMMAND_FRAME_BYTES);
        assert!(matches!(
            read_query(&mut Cursor::new(exact_control.as_bytes())).unwrap(),
            Some(HostCommand::Configure { .. })
        ));
        let mut too_large_control = exact_control.as_bytes().to_vec();
        too_large_control.insert(too_large_control.len() - 1, b' ');
        assert_eq!(too_large_control.len(), MAX_COMMAND_FRAME_BYTES + 1);
        assert!(read_query(&mut Cursor::new(too_large_control)).is_err());

        let invalid_text =
            serde_json::to_string(&query(7, 0, 4, &"x".repeat(4097))).unwrap() + "\n";
        assert!(read_query(&mut Cursor::new(invalid_text)).is_err());
        let invalid_id = serde_json::to_string(&query(7, 0, 0, "x")).unwrap() + "\n";
        assert!(read_query(&mut Cursor::new(invalid_id)).is_err());
        let external_request = serde_json::to_string(&Request::Open {
            path: NativePath::new("C:/"),
        })
        .unwrap()
            + "\n";
        assert!(read_query(&mut Cursor::new(external_request)).is_err());
        assert!(read_query(&mut Cursor::new(vec![b'x'; MAX_COMMAND_FRAME_BYTES + 2])).is_err());
    }

    #[test]
    fn command_inbox_coalesces_only_queries_and_keeps_control_fifo_and_eof() {
        let inbox = CommandInbox::default();
        let first = HostCommand::Configure {
            session: 7,
            generation: 0,
            settings: Box::new(crate::index_policy::NtfsIndexingSettings::default()),
        };
        assert!(inbox.enqueue(first.clone()).unwrap().is_none());
        assert!(inbox.enqueue(query(7, 0, 1, "first")).unwrap().is_none());
        assert!(matches!(inbox.enqueue(rebuild(1)).unwrap(), None));
        assert!(matches!(
            inbox.enqueue(query(7, 0, 2, "latest")).unwrap(),
            Some(HostCommand::Query { query_id: 1, .. })
        ));
        let commands = inbox.pop_batch(8);
        assert!(matches!(
            commands.as_slice(),
            [
                HostCommand::Configure { .. },
                HostCommand::Rebuild { request_id: 1, .. },
                HostCommand::Query { query_id: 2, .. }
            ]
        ));
        for request_id in 10..10 + MAX_PENDING_CONTROLS as u64 {
            assert!(inbox.enqueue(rebuild(request_id)).is_ok());
        }
        assert!(inbox.enqueue(rebuild(99)).is_err());
        let remaining = inbox.pop_batch(MAX_PENDING_CONTROLS + 1);
        assert_eq!(remaining.len(), MAX_PENDING_CONTROLS);
        assert!(remaining.iter().enumerate().all(|(i, command)| matches!(
            command,
            HostCommand::Rebuild { request_id, .. } if *request_id == 10 + i as u64
        )));
        inbox.close();
        assert!(inbox.closed_and_empty());
    }
    #[test]
    fn stale_host_commands_do_not_match_a_new_session_or_generation() {
        assert!(same_generation(7, 3, 7, 3));
        assert!(!same_generation(7, 3, 7, 2));
        assert!(!same_generation(7, 3, 7, 4));
        assert!(!same_generation(7, 3, 8, 3));
    }

    #[test]
    fn journal_replacement_and_gaps_require_rebuild() {
        assert!(journal_current(7, 100, 7, 10, 20));
        assert!(!journal_current(7, 100, 8, 10, 20));
        assert!(!journal_current(7, 19, 7, 10, 20));
        assert!(!journal_current(7, 100, 7, 101, 20));
    }
    #[test]
    fn multiple_volumes_keep_exact_paths_total_and_bounded_results() {
        let part = |drive: &str, n| SearchResult {
            hits: (0..n)
                .map(|i| SearchHit {
                    path: NativePath::new(format!("{drive}/보고서-{i:03}.txt")),
                    name: format!("보고서-{i:03}.txt"),
                    is_dir: false,
                    volume_guid: None,
                    volume_mount: None,
                })
                .collect(),
            total: 1000,
            elapsed_ms: 2.,
            complete: true,
        };
        let r = merge([part("C:", 150), part("D:", 150)], true);
        assert_eq!(r.total, 2000);
        assert_eq!(r.hits.len(), 200);
        assert_eq!(r.elapsed_ms, 4.);
        assert!(r.complete);
        assert!(r.hits.iter().any(|h| h.path.display().starts_with("C:")));
        assert!(r.hits.iter().any(|h| h.path.display().starts_with("D:")));
        assert!(!merge([part("C:", 1)], false).complete);
    }
}

#[cfg(test)]
mod journal_tests {
    use super::*;
    use goz_core::{
        index::{FrnMap, NTFS_ROOT_FRN, VolumeIndex},
        types::Frn,
        usn::record::*,
    };
    fn record(frn: u64, parent: Frn, name: &str, reason: u32, dir: bool) -> ParsedUsnRecord {
        ParsedUsnRecord {
            major_version: 2,
            frn: Frn((1u64 << 48) | frn),
            parent_frn: parent,
            usn: frn as i64,
            timestamp_ft: 0,
            reason,
            attributes: if dir { FILE_ATTRIBUTE_DIRECTORY } else { 0 },
            name: name.as_bytes().to_vec(),
            name_lossy: false,
        }
    }
    #[test]
    fn native_journal_operations_update_descendant_paths_without_rescanning() {
        let mut index = VolumeIndex::new(NTFS_ROOT_FRN, FrnMap::sparse());
        let folder = record(100, NTFS_ROOT_FRN, "work", USN_REASON_FILE_CREATE, true);
        let file = record(101, folder.frn, "보고서.txt", USN_REASON_FILE_CREATE, false);
        index.insert_enum(&folder);
        index.insert_enum(&file);
        let root = NativePath::new("C:/");
        let before = crate::search::query_index(&index, &root, "보고서", true).unwrap();
        assert_eq!(before.hits.len(), 1);
        assert!(before.hits[0].path.display().contains("work"));
        let renamed = record(
            100,
            NTFS_ROOT_FRN,
            "renamed",
            USN_REASON_RENAME_NEW_NAME,
            true,
        );
        index.apply_batch(&[renamed]);
        let after = crate::search::query_index(&index, &root, "보고서", true).unwrap();
        assert_eq!(after.hits.len(), 1);
        assert!(after.hits[0].path.display().contains("renamed"));
        assert!(!after.hits[0].path.display().contains("work"));
        let deleted = record(101, folder.frn, "보고서.txt", USN_REASON_FILE_DELETE, false);
        index.apply_batch(&[deleted]);
        assert_eq!(
            crate::search::query_index(&index, &root, "보고서", true)
                .unwrap()
                .total,
            0
        );
    }
}
