//! Session-local, bounded undo records for this app's completed native operations.
use crate::model::NativePath;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::Path,
    time::{Duration, Instant, UNIX_EPOCH},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecycledItem {
    pub path: NativePath,
    pub pidl: Vec<u8>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ItemSource {
    File(NativePath),
    Recycle(RecycledItem),
}
impl ItemSource {
    pub fn path(&self) -> &NativePath {
        match self {
            Self::File(p) => p,
            Self::Recycle(r) => &r.path,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ItemTarget {
    Path(NativePath),
    Recycle,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum ChangeKind {
    Copy,
    Move,
    New,
    Recycle,
    Restore,
}
#[derive(Clone, Debug)]
pub struct Change {
    pub kind: ChangeKind,
    pub source: Option<ItemSource>,
    pub destination: Option<NativePath>,
    pub recycled: Option<RecycledItem>,
    pub overwritten: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fingerprint {
    pub identity: Option<(u32, u64)>,
    pub metadata: u64,
    pub entries: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryItem {
    pub source: ItemSource,
    pub target: ItemTarget,
    pub expected: Fingerprint,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Record {
    pub items: Vec<HistoryItem>,
}
impl Record {
    pub fn folders(&self) -> Vec<NativePath> {
        self.items
            .iter()
            .flat_map(|i| {
                let mut p: Vec<_> = i.source.path().parent().into_iter().collect();
                if let ItemTarget::Path(t) = &i.target {
                    p.extend(t.parent());
                }
                p
            })
            .collect()
    }
    pub fn destination(&self) -> Option<NativePath> {
        self.items.iter().find_map(|i| {
            if let ItemTarget::Path(p) = &i.target {
                p.parent()
            } else {
                None
            }
        })
    }
}
fn mix(hash: &mut u64, bytes: &[u8]) {
    for b in bytes {
        *hash = (*hash ^ u64::from(*b)).wrapping_mul(0x100000001b3);
    }
}
fn link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}
#[cfg(windows)]
fn identity(path: &Path) -> Result<Option<(u32, u64)>, String> {
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    use windows::Win32::{
        Foundation::HANDLE,
        Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
            FILE_READ_ATTRIBUTES, GetFileInformationByHandle,
        },
    };
    let file = fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES.0)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)
        .map_err(|e| e.to_string())?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    unsafe {
        GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info)
            .map_err(|e| e.to_string())?;
    }
    Ok(Some((
        info.dwVolumeSerialNumber,
        (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    )))
}
#[cfg(target_os = "macos")]
fn identity(path: &Path) -> Result<Option<(u32, u64)>, String> {
    use std::os::unix::fs::MetadataExt;
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    Ok(Some((metadata.dev() as u32, metadata.ino())))
}
#[cfg(not(any(windows, target_os = "macos")))]
fn identity(_path: &Path) -> Result<Option<(u32, u64)>, String> {
    Ok(None)
}
/// Metadata only: no contents, thumbnails, symlink traversal, or indefinite scans.
pub fn fingerprint(path: &NativePath) -> Result<Fingerprint, String> {
    let root = path.to_path();
    let started = Instant::now();
    let mut stack = vec![(root.clone(), 0usize)];
    let mut hash = 0xcbf29ce484222325;
    let mut count = 0;
    while let Some((p, depth)) = stack.pop() {
        if count >= 20_000 || depth > 128 || started.elapsed() > Duration::from_secs(2) {
            return Err("실행 취소 메타데이터 확인 범위를 초과했습니다.".into());
        }
        let meta = fs::symlink_metadata(&p).map_err(|e| e.to_string())?;
        if link(&meta) {
            return Err("연결·클라우드 재분석 항목은 실행 취소 기록에서 제외합니다.".into());
        }
        let relative = NativePath::new(p.strip_prefix(&root).unwrap_or(&p));
        for unit in relative.0 {
            mix(&mut hash, &unit.to_le_bytes());
        }
        mix(&mut hash, &[u8::from(meta.is_dir())]);
        mix(&mut hash, &meta.len().to_le_bytes());
        let modified = meta
            .modified()
            .map_err(|e| e.to_string())?
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        mix(&mut hash, &modified.to_le_bytes());
        count += 1;
        if meta.is_dir() {
            let mut children: Vec<_> = fs::read_dir(&p)
                .map_err(|e| e.to_string())?
                .map(|e| e.map(|e| e.path()))
                .collect::<Result<_, _>>()
                .map_err(|e| e.to_string())?;
            children.sort();
            stack.extend(children.into_iter().rev().map(|p| (p, depth + 1)));
        }
    }
    Ok(Fingerprint {
        identity: identity(&root)?,
        metadata: hash,
        entries: count,
    })
}
pub fn inverse(changes: Vec<Change>) -> Result<Option<Record>, String> {
    if changes.is_empty() {
        return Ok(None);
    }
    let mut items = vec![];
    for c in changes.into_iter().rev() {
        if c.overwritten {
            return Err("덮어쓰기·폴더 병합은 앱 실행 취소에서 지원하지 않습니다.".into());
        }
        let (source, target) = match c.kind {
            ChangeKind::Copy | ChangeKind::New | ChangeKind::Restore => (
                ItemSource::File(c.destination.ok_or("결과 경로가 없습니다.")?),
                ItemTarget::Recycle,
            ),
            ChangeKind::Move => (
                ItemSource::File(c.destination.ok_or("결과 경로가 없습니다.")?),
                ItemTarget::Path(c.source.ok_or("원본 경로가 없습니다.")?.path().clone()),
            ),
            ChangeKind::Recycle => (
                ItemSource::Recycle(
                    c.recycled
                        .ok_or("영구 삭제되거나 휴지통 복원 항목을 확인하지 못했습니다.")?,
                ),
                ItemTarget::Path(c.source.ok_or("원본 경로가 없습니다.")?.path().clone()),
            ),
        };
        let expected = fingerprint(source.path())?;
        items.push(HistoryItem {
            source,
            target,
            expected,
        });
    }
    Ok(Some(Record { items }))
}
pub fn validate(record: &Record) -> Result<(), String> {
    if record.items.is_empty() || record.items.len() > 20_000 {
        return Err("실행 취소 기록이 비어 있거나 너무 큽니다.".into());
    }
    for item in &record.items {
        if fingerprint(item.source.path())? != item.expected {
            return Err("작업 이후 항목이 변경됐습니다. 다른 변경을 보호하기 위해 실행 취소를 중단했습니다.".into());
        }
        if let ItemTarget::Path(target) = &item.target {
            if target.to_path().try_exists().map_err(|e| e.to_string())?
                && !target
                    .display()
                    .eq_ignore_ascii_case(&item.source.path().display())
            {
                return Err(
                    "원래 위치에 다른 항목이 있습니다. 실행 취소로 덮어쓰지 않습니다.".into(),
                );
            }
            crate::filesystem::validate_name(&target.name())?;
            if !target.parent().is_some_and(|p| p.to_path().is_dir()) {
                return Err("복원할 원래 폴더를 찾을 수 없습니다.".into());
            }
        }
        if let ItemSource::Recycle(r) = &item.source {
            if !valid_pidl(&r.pidl) {
                return Err("휴지통 항목 식별자가 유효하지 않습니다.".into());
            }
        }
    }
    Ok(())
}
pub fn valid_pidl(bytes: &[u8]) -> bool {
    if bytes.len() < 2 || bytes.len() > 65_536 {
        return false;
    }
    let mut offset = 0;
    while offset + 2 <= bytes.len() {
        let size = u16::from_le_bytes([bytes[offset], bytes[offset + 1]]) as usize;
        if size == 0 {
            return offset + 2 == bytes.len();
        }
        if size < 2 || offset + size > bytes.len() {
            return false;
        }
        offset += size;
    }
    false
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn changed_folder_contents_and_occupied_destinations_block_undo() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("copy");
        fs::create_dir(&p).unwrap();
        fs::write(p.join("a"), b"a").unwrap();
        let path = NativePath::new(&p);
        let r = Record {
            items: vec![HistoryItem {
                source: ItemSource::File(path.clone()),
                target: ItemTarget::Path(NativePath::new(d.path().join("original"))),
                expected: fingerprint(&path).unwrap(),
            }],
        };
        validate(&r).unwrap();
        fs::write(p.join("new"), b"new").unwrap();
        assert!(validate(&r).is_err());
        let mut r = r;
        r.items[0].expected = fingerprint(&path).unwrap();
        fs::write(d.path().join("original"), b"other").unwrap();
        assert!(validate(&r).is_err());
        assert_eq!(fs::read(p.join("new")).unwrap(), b"new");
    }
    #[test]
    fn pidl_bounds_and_terminator_are_validated() {
        assert!(valid_pidl(&[4, 0, 1, 2, 0, 0]));
        assert!(!valid_pidl(&[255, 255, 0, 0]));
        assert!(!valid_pidl(&[1, 0, 0, 0]));
        assert!(!valid_pidl(&[0, 0, 2]));
    }
}
