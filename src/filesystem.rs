use crate::model::{FileAction, FileEntry, NativePath};
use std::{fs, path::Path, time::UNIX_EPOCH};

pub fn entry(path: &Path) -> Result<FileEntry, String> {
    #[cfg(target_os = "macos")]
    if path.to_str().is_none() {
        return Err("이 파일 이름을 UTF-8로 읽을 수 없습니다.".into());
    }
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    Ok(from_metadata(path, metadata))
}
/// Windows ReadDir already carries this metadata: do not reopen every cloud item.
pub fn directory_entry(item: &fs::DirEntry) -> Result<FileEntry, String> {
    #[cfg(target_os = "macos")]
    if item.path().to_str().is_none() {
        return Err("이 파일 이름을 UTF-8로 읽을 수 없습니다.".into());
    }
    let metadata = item.metadata().map_err(|e| e.to_string())?;
    Ok(from_metadata(&item.path(), metadata))
}
fn from_metadata(path: &Path, metadata: fs::Metadata) -> FileEntry {
    let name = path
        .file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned();
    #[cfg(windows)]
    let (hidden, is_link) = {
        use std::os::windows::fs::MetadataExt;
        let a = metadata.file_attributes();
        (a & 2 != 0, a & 0x400 != 0)
    };
    #[cfg(not(windows))]
    let (hidden, is_link) = (name.starts_with('.'), metadata.file_type().is_symlink());
    FileEntry {
        path: NativePath::new(path),
        name,
        is_dir: metadata.is_dir(),
        is_link,
        bytes: if metadata.is_file() {
            Some(metadata.len())
        } else {
            None
        },
        modified: metadata
            .modified()
            .ok()
            .and_then(|x| x.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs()),
        hidden,
        cloud: {
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                crate::model::CloudState::from_attributes(metadata.file_attributes())
            }
            #[cfg(not(windows))]
            {
                crate::model::CloudState::None
            }
        },
    }
}

pub fn read_error(e: std::io::Error) -> String {
    let hint = match e.raw_os_error() {
        Some(362 | 4350 | 426) => {
            "클라우드 공급자와 네트워크 연결을 확인하고, 필요하면 공급자 앱에서 로컬 보관으로 변경한 뒤 다시 시도하세요."
        }
        Some(53 | 64 | 67 | 121 | 1231) => "네트워크 드라이브 연결을 확인한 뒤 다시 시도하세요.",
        Some(5) => "폴더 접근 권한을 확인하세요.",
        _ => "경로·접근 권한과 드라이브 연결을 확인하세요.",
    };
    format!("{e} · {hint} F5로 다시 시도할 수 있습니다.")
}
pub fn list_folder(
    path: &NativePath,
    show_hidden: bool,
    mut emit: impl FnMut(Vec<FileEntry>) -> bool,
) -> Result<usize, String> {
    let mut batch = Vec::with_capacity(256);
    let mut limit = 256;
    let mut last_flush = std::time::Instant::now();
    let mut skipped = 0;
    let mut wire_bytes = 0;
    for item in fs::read_dir(path.to_path()).map_err(read_error)? {
        let item = match item {
            Ok(v) => v,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        match directory_entry(&item) {
            Ok(e) if show_hidden || !e.hidden => {
                // Worker messages carry only the filename; the parent is already in the request.
                wire_bytes += e.name.len() * 6 + 128;
                batch.push(e);
            }
            Ok(_) => {}
            Err(_) => skipped += 1,
        }
        if batch.len() >= limit
            || wire_bytes >= 500_000
            || (!batch.is_empty() && last_flush.elapsed() >= std::time::Duration::from_millis(30))
        {
            if !emit(std::mem::replace(&mut batch, Vec::with_capacity(1024))) {
                return Ok(skipped);
            }
            limit = 1024;
            last_flush = std::time::Instant::now();
            wire_bytes = 0;
        }
    }
    if !batch.is_empty() {
        emit(batch);
    }
    Ok(skipped)
}

#[cfg(target_os = "macos")]
pub fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', ':', '\0']) {
        return Err("파일 이름에 사용할 수 없는 문자가 있습니다.".into());
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.encode_utf16().count() > 255
        || name == "."
        || name == ".."
        || name.ends_with([' ', '.'])
        || name.chars().any(|c| c < ' ' || "<>:\"/\\|?*".contains(c))
    {
        return Err("파일 이름에 사용할 수 없는 문자가 있습니다.".into());
    }
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    if ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
    {
        return Err("Windows 예약 이름은 사용할 수 없습니다.".into());
    }
    Ok(())
}

pub fn validate_action(action: &FileAction) -> Result<(), String> {
    match action {
        FileAction::BatchRename { pairs } => crate::batch::validate(pairs)?,
        FileAction::History { record } => crate::history::validate(record)?,
        FileAction::Rename { source, name } => {
            validate_name(name)?;
            if source.to_path().parent().is_none() {
                return Err("드라이브 루트의 이름은 변경할 수 없습니다.".into());
            }
        }
        FileAction::NewFolder { name, .. } => validate_name(name)?,
        FileAction::Copy {
            sources,
            destination,
        }
        | FileAction::Move {
            sources,
            destination,
        } => {
            if sources.is_empty() {
                return Err("선택된 항목이 없습니다.".into());
            }
            let dest = fs::canonicalize(destination.to_path()).map_err(|e| e.to_string())?;
            for source in sources {
                let original = fs::canonicalize(source.to_path()).map_err(|e| e.to_string())?;
                if dest == original || (original.is_dir() && dest.starts_with(&original)) {
                    return Err("폴더를 자기 자신이나 하위 폴더에 넣을 수 없습니다.".into());
                }
                if matches!(action, FileAction::Move { .. })
                    && original.parent() == Some(dest.as_path())
                {
                    return Err("원본과 대상 폴더가 같습니다.".into());
                }
            }
        }
        FileAction::Recycle { sources } | FileAction::Delete { sources } => {
            if sources.is_empty() || sources.iter().any(|p| p.to_path().parent().is_none()) {
                return Err("드라이브 루트는 삭제할 수 없습니다.".into());
            }
        }
    }
    Ok(())
}

pub fn readable_size(bytes: Option<u64>) -> String {
    let Some(bytes) = bytes else {
        return "—".into();
    };
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024. && unit < 4 {
        value /= 1024.;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", units[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    fn directory_metadata_is_cached_without_reopening_item() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("cloud-like.txt");
        fs::write(&path, b"cached attributes").unwrap();
        let item = fs::read_dir(d.path()).unwrap().next().unwrap().unwrap();
        fs::remove_file(&path).unwrap();
        let cached =
            directory_entry(&item).expect("enumeration metadata does not reopen removed file");
        assert_eq!(cached.bytes, Some(17));
        assert_eq!(cached.path.to_path(), path);
        assert!(entry(&path).is_err());
    }
    #[test]
    fn listing_streams_and_keeps_unicode() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("보고서.txt"), b"hello").unwrap();
        fs::create_dir(d.path().join("folder")).unwrap();
        let mut rows = vec![];
        list_folder(&NativePath::new(d.path()), true, |b| {
            rows.extend(b);
            true
        })
        .unwrap();
        assert_eq!(rows.len(), 2);
        assert!(
            rows.iter()
                .any(|e| e.name == "보고서.txt" && e.bytes == Some(5))
        );
    }
    #[test]
    fn invalid_names_are_rejected() {
        for name in ["", ".", "..", "../escape", "a\0b"] {
            assert!(validate_name(name).is_err(), "{name}");
        }
        let platform_names = ["a\\b", "CON.txt", "LPT1", "trailing."];
        for name in platform_names {
            assert_eq!(
                validate_name(name).is_ok(),
                cfg!(target_os = "macos"),
                "{name}"
            );
        }
        assert!(validate_name("보고서 2026.txt").is_ok());
    }
    #[test]
    fn recursive_destination_is_rejected() {
        let d = tempfile::tempdir().unwrap();
        let parent = d.path().join("from");
        let child = parent.join("child");
        fs::create_dir_all(&child).unwrap();
        assert!(
            validate_action(&FileAction::Copy {
                sources: vec![NativePath::new(&parent)],
                destination: NativePath::new(&child)
            })
            .is_err()
        );
    }
}
