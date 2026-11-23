//! Native nonrecursive folder notifications, hosted in an owned cancellable worker.
use crate::model::NativePath;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Rename {
    pub from: NativePath,
    pub to: NativePath,
}
/// A buffer can split rename pairs; retain the old name until its matching new record.
pub fn parse(bytes: &[u8], root: &NativePath, old: &mut Option<NativePath>) -> Vec<Rename> {
    let mut offset = 0;
    let mut result = vec![];
    let mut units = 0;
    while offset + 12 <= bytes.len() {
        let read =
            |n| u32::from_le_bytes(bytes[offset + n..offset + n + 4].try_into().unwrap()) as usize;
        let next = read(0);
        let action = read(4);
        let len = read(8);
        if len % 2 != 0 || offset + 12 + len > bytes.len() {
            break;
        }
        let name = bytes[offset + 12..offset + 12 + len]
            .chunks_exact(2)
            .map(|p| u16::from_le_bytes([p[0], p[1]]))
            .collect();
        let name = NativePath(name);
        let path = NativePath::new(root.to_path().join(name.to_path()));
        match action {
            4 => *old = Some(path),
            5 => {
                if let Some(from) = old.take() {
                    units += from.0.len() + path.0.len();
                    if units <= 64000 {
                        result.push(Rename { from, to: path });
                    }
                }
            }
            _ => *old = None,
        }
        if next == 0 {
            break;
        }
        if next < 12 || offset + next > bytes.len() {
            break;
        }
        offset += next;
    }
    result
}
pub fn run(
    root: &NativePath,
    mut ready: impl FnMut(),
    mut changed: impl FnMut(Vec<Rename>),
) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::{
            fs::OpenOptions,
            os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
        };
        use windows::Win32::{
            Foundation::{CloseHandle, HANDLE},
            Storage::FileSystem::{
                FILE_NOTIFY_CHANGE_ATTRIBUTES, FILE_NOTIFY_CHANGE_DIR_NAME,
                FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE,
                FILE_NOTIFY_CHANGE_SIZE, ReadDirectoryChangesW,
            },
            System::{
                IO::{GetOverlappedResult, OVERLAPPED},
                Threading::{CreateEventW, INFINITE, ResetEvent, WaitForSingleObject},
            },
        };
        let file = OpenOptions::new()
            .access_mode(1)
            .share_mode(7)
            .custom_flags(0x02000000 | 0x40000000)
            .open(root.to_path())
            .map_err(|e| e.to_string())?;
        struct EventHandle(HANDLE);
        impl Drop for EventHandle {
            fn drop(&mut self) {
                unsafe {
                    let _ = CloseHandle(self.0);
                }
            }
        }
        let event = EventHandle(
            unsafe { CreateEventW(None, true, false, None) }.map_err(|e| e.to_string())?,
        );
        let handle = HANDLE(file.as_raw_handle());
        let mut buffer = vec![0u32; 16384];
        let mut first = true;
        let mut old = None;
        loop {
            let mut overlapped = OVERLAPPED {
                hEvent: event.0,
                ..Default::default()
            };
            unsafe {
                ResetEvent(event.0).map_err(|e| e.to_string())?;
                ReadDirectoryChangesW(
                    handle,
                    buffer.as_mut_ptr().cast(),
                    65536,
                    false,
                    FILE_NOTIFY_CHANGE_FILE_NAME
                        | FILE_NOTIFY_CHANGE_DIR_NAME
                        | FILE_NOTIFY_CHANGE_ATTRIBUTES
                        | FILE_NOTIFY_CHANGE_SIZE
                        | FILE_NOTIFY_CHANGE_LAST_WRITE,
                    None,
                    Some(&mut overlapped),
                    None,
                )
                .map_err(|e| e.to_string())?;
                if first {
                    ready();
                    first = false;
                }
                let _ = WaitForSingleObject(event.0, INFINITE);
                let mut count = 0;
                GetOverlappedResult(handle, &overlapped, &mut count, false)
                    .map_err(|e| e.to_string())?;
                let bytes = std::slice::from_raw_parts(
                    buffer.as_ptr().cast::<u8>(),
                    (count as usize).min(65536),
                );
                if count == 0 {
                    old = None;
                }
                let renames = parse(bytes, root, &mut old);
                changed(renames);
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        use notify::{
            EventKind, RecommendedWatcher, RecursiveMode, Watcher,
            event::{ModifyKind, RenameMode},
        };
        let (send, receive) = std::sync::mpsc::channel();
        let mut watcher = RecommendedWatcher::new(send, notify::Config::default())
            .map_err(|error| error.to_string())?;
        watcher
            .watch(&root.to_path(), RecursiveMode::NonRecursive)
            .map_err(|error| error.to_string())?;
        ready();
        for result in receive {
            let event = result.map_err(|error| error.to_string())?;
            let renames = match (&event.kind, event.paths.as_slice()) {
                (EventKind::Modify(ModifyKind::Name(RenameMode::Both)), [from, to]) => {
                    vec![Rename {
                        from: NativePath::new(from),
                        to: NativePath::new(to),
                    }]
                }
                _ => Vec::new(),
            };
            changed(renames);
        }
        Ok(())
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = (root, &mut ready, &mut changed);
        Err("폴더 변경 감시는 Windows 전용입니다.".into())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn record(action: u32, name: &str) -> Vec<u8> {
        let name: Vec<_> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let mut b = vec![];
        b.extend(0u32.to_le_bytes());
        b.extend(action.to_le_bytes());
        b.extend((name.len() as u32).to_le_bytes());
        b.extend(name);
        b
    }
    #[test]
    fn split_rename_and_bad_record_are_bounded() {
        let root = NativePath::new("root");
        let mut old = None;
        assert!(parse(&record(4, "old"), &root, &mut old).is_empty());
        let r = parse(&record(5, "new"), &root, &mut old);
        assert_eq!(r[0].from, NativePath::new(root.to_path().join("old")));
        assert_eq!(r[0].to, NativePath::new(root.to_path().join("new")));
        assert!(parse(&[0; 7], &root, &mut old).is_empty());
    }
}
