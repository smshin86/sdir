use crate::index_policy::VolumeGuid;
use goz_core::{types::Frn, wtf8};
use std::{
    mem::size_of,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
};
use windows::{
    Wdk::{Foundation::OBJECT_ATTRIBUTES, Storage::FileSystem::*},
    Win32::{
        Foundation::{HANDLE, OBJ_CASE_INSENSITIVE, OBJ_DONT_REPARSE, UNICODE_STRING},
        Security::{DuplicateToken, RevertToSelf, SecurityImpersonation, TOKEN_DUPLICATE},
        Storage::FileSystem::*,
        System::{
            IO::IO_STATUS_BLOCK,
            Threading::{OpenProcessToken, SetThreadToken},
        },
    },
    core::{PCWSTR, PWSTR},
};

pub(super) struct RequesterAccess(OwnedHandle);

impl RequesterAccess {
    pub(super) fn from_process(process: HANDLE) -> Result<Self, String> {
        let mut token = HANDLE::default();
        unsafe { OpenProcessToken(process, TOKEN_DUPLICATE, &mut token) }
            .map_err(|e| format!("검색 요청 계정의 권한을 확인하지 못했습니다: {e}"))?;
        let token = unsafe { OwnedHandle::from_raw_handle(token.0) };
        Self::duplicate(&token)
    }

    fn duplicate(token: &OwnedHandle) -> Result<Self, String> {
        let mut duplicate = HANDLE::default();
        unsafe {
            DuplicateToken(
                HANDLE(token.as_raw_handle()),
                SecurityImpersonation,
                &mut duplicate,
            )
        }
        .map_err(|e| format!("검색 권한 검사에 필요한 토큰을 만들지 못했습니다: {e}"))?;
        Ok(Self(unsafe { OwnedHandle::from_raw_handle(duplicate.0) }))
    }

    pub(super) fn with_permissions<T>(&self, check: impl FnOnce() -> T) -> Result<T, String> {
        unsafe { SetThreadToken(None, Some(HANDLE(self.0.as_raw_handle()))) }
            .map_err(|e| format!("검색 요청 계정의 권한을 적용하지 못했습니다: {e}"))?;
        let _impersonating = Impersonation;
        Ok(check())
    }
}

struct Impersonation;

impl Drop for Impersonation {
    fn drop(&mut self) {
        // Continuing the host requires a known security context, including during unwinding.
        if unsafe { RevertToSelf() }.is_err() {
            std::process::abort();
        }
    }
}

pub(super) struct VolumeAccess {
    root: OwnedHandle,
    directories: Vec<VerifiedDirectory>,
}

struct VerifiedDirectory {
    name: Vec<u16>,
    handle: OwnedHandle,
}

impl VolumeAccess {
    pub(super) fn open(guid: &VolumeGuid, serial: Option<u32>) -> Option<Self> {
        let path = super::wide(&format!("\\\\?\\Volume{{{}}}\\", guid.as_str()));
        let root = Self::open_root(&path)?;
        let info = file_information(&root.root)?;
        (Some(info.dwVolumeSerialNumber) == serial).then_some(root)
    }

    fn open_root(path: &[u16]) -> Option<Self> {
        let root = unsafe {
            CreateFileW(
                PCWSTR(path.as_ptr()),
                (FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES).0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS
                    | FILE_FLAG_OPEN_REPARSE_POINT
                    | FILE_FLAG_OPEN_NO_RECALL,
                None,
            )
        }
        .ok()?;
        Some(Self {
            root: unsafe { OwnedHandle::from_raw_handle(root.0) },
            directories: Vec::new(),
        })
    }

    pub(super) fn allows(&mut self, relative: &[u8], frn: Frn, is_dir: bool) -> bool {
        let path = wtf8::to_utf16(relative);
        let mut components = path.split(|&unit| unit == u16::from(b'\\')).peekable();
        let mut depth = 0;
        while let Some(name) = components.next() {
            let is_last = components.peek().is_none();
            if !is_last
                && self
                    .directories
                    .get(depth)
                    .is_some_and(|entry| entry.name == name)
            {
                depth += 1;
                continue;
            }
            self.directories.truncate(depth);
            let parent = self
                .directories
                .last()
                .map_or(&self.root, |entry| &entry.handle);
            let Some(child) = open_child(parent, name, !is_last || is_dir) else {
                return false;
            };
            let Some(info) = file_information(&child) else {
                return false;
            };
            if is_last {
                let file_id =
                    (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
                return file_id == frn.0;
            }
            if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
                return false;
            }
            // A verified directory handle is reused only during this query. Each leaf
            // is still opened under the requester's token and its identity is checked.
            self.directories.push(VerifiedDirectory {
                name: name.to_vec(),
                handle: child,
            });
            depth += 1;
        }
        false
    }
}

fn open_child(parent: &OwnedHandle, name: &[u16], directory: bool) -> Option<OwnedHandle> {
    if name.is_empty()
        || name == [u16::from(b'.')]
        || name == [u16::from(b'.'); 2]
        || name.iter().any(|&unit| matches!(unit, 0 | 47 | 58))
    {
        return None;
    }
    let length = u16::try_from(std::mem::size_of_val(name)).ok()?;
    let name = UNICODE_STRING {
        Length: length,
        MaximumLength: length,
        Buffer: PWSTR(name.as_ptr().cast_mut()),
    };
    let object = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: HANDLE(parent.as_raw_handle()),
        ObjectName: &name,
        Attributes: OBJ_CASE_INSENSITIVE | OBJ_DONT_REPARSE,
        ..Default::default()
    };
    let (access, kind) = if directory {
        (FILE_LIST_DIRECTORY, FILE_DIRECTORY_FILE)
    } else {
        (FILE_READ_DATA, FILE_NON_DIRECTORY_FILE)
    };
    let mut child = HANDLE::default();
    let mut status = IO_STATUS_BLOCK::default();
    // Handle-relative opens keep each checked directory fixed for the next component.
    let result = unsafe {
        NtCreateFile(
            &mut child,
            access | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            &object,
            &mut status,
            None,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            FILE_OPEN,
            kind | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
            None,
            0,
        )
    };
    if !result.is_ok() || child.is_invalid() {
        return None;
    }
    Some(unsafe { OwnedHandle::from_raw_handle(child.0) })
}

fn file_information(file: &OwnedHandle) -> Option<BY_HANDLE_FILE_INFORMATION> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) }.ok()?;
    Some(info)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NativePath;
    use std::{
        fs,
        path::{Path, PathBuf},
        process::Command,
    };
    use windows::Win32::System::Threading::GetCurrentProcess;

    fn change_read_deny(path: &Path, deny: bool) {
        let icacls =
            PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32/icacls.exe");
        let args = if deny {
            ["/deny", "*S-1-1-0:(RD)"]
        } else {
            ["/remove:d", "*S-1-1-0"]
        };
        let output = Command::new(icacls).arg(path).args(args).output().unwrap();
        assert!(
            output.status.success(),
            "icacls failed with {:?}",
            output.status.code()
        );
    }

    #[test]
    fn neighboring_files_permission_check_batch() {
        let fixture = tempfile::tempdir().unwrap();
        let parent = fixture.path().join(r"first\second\third\fourth");
        fs::create_dir_all(&parent).unwrap();
        let mut candidates = vec![];
        for i in 0..200 {
            let file = parent.join(format!("benchmark-{i:03}.txt"));
            fs::write(&file, b"sample").unwrap();
            let handle = fs::File::open(&file).unwrap();
            let mut info = BY_HANDLE_FILE_INFORMATION::default();
            unsafe {
                GetFileInformationByHandle(HANDLE(handle.as_raw_handle()), &mut info).unwrap()
            };
            let id = Frn((u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow));
            let relative = file
                .strip_prefix(fixture.path())
                .unwrap()
                .to_str()
                .unwrap()
                .as_bytes()
                .to_vec();
            candidates.push((relative, id));
        }
        let mut root = NativePath::new(fixture.path()).0;
        root.push(0);
        let requester = RequesterAccess::from_process(unsafe { GetCurrentProcess() }).unwrap();
        let mut times = vec![];
        for _ in 0..5 {
            requester
                .with_permissions(|| {
                    let mut access = VolumeAccess::open_root(&root).unwrap();
                    let start = std::time::Instant::now();
                    for (name, id) in &candidates {
                        assert!(access.allows(name, *id, false));
                    }
                    times.push(start.elapsed().as_secs_f64() * 1000.);
                })
                .unwrap();
        }
        times.sort_by(f64::total_cmp);
        println!("200 native access checks median {:.2} ms", times[2]);
        fixture.close().unwrap();
    }

    #[test]
    fn windows_requester_follows_current_acl_and_indexed_file_identity() {
        let fixture = tempfile::tempdir().unwrap();
        let parent = fixture.path().join("테스트");
        fs::create_dir(&parent).unwrap();
        let path = parent.join("문서.txt");
        fs::write(&path, b"owned fixture").unwrap();
        let native = NativePath::new(&path);
        let mut absolute = native.0;
        absolute.push(0);
        let mut mount = vec![0u16; 32_768];
        let mut guid = [0u16; 64];
        unsafe {
            GetVolumePathNameW(PCWSTR(absolute.as_ptr()), &mut mount).unwrap();
            GetVolumeNameForVolumeMountPointW(PCWSTR(mount.as_ptr()), &mut guid).unwrap();
        }
        let title = |path: &[u16]| {
            String::from_utf16(&path[..path.iter().position(|&s| s == 0).unwrap()]).unwrap()
        };
        let guid = VolumeGuid::parse(&title(&guid)).unwrap();
        let mount = PathBuf::from(title(&mount));
        let relative = path
            .strip_prefix(&mount)
            .unwrap()
            .to_str()
            .unwrap()
            .as_bytes();
        let file = fs::File::open(&path).unwrap();
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info).unwrap() };
        drop(file);
        let id = Frn((u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow));
        let requester = RequesterAccess::from_process(unsafe { GetCurrentProcess() }).unwrap();
        let allowed = |expected_id| {
            requester
                .with_permissions(|| {
                    VolumeAccess::open(&guid, Some(info.dwVolumeSerialNumber))
                        .expect("the requester can list the fixture volume")
                        .allows(relative, expected_id, false)
                })
                .unwrap()
        };
        let original = allowed(id);
        let stale = allowed(Frn(id.0 ^ (1 << 48)));
        change_read_deny(&path, true);
        let file_denied = !allowed(id);
        change_read_deny(&path, false);
        change_read_deny(&parent, true);
        let parent_denied = !allowed(id);
        change_read_deny(&parent, false);
        assert!(
            original,
            "the correctly indexed accessible file should appear"
        );
        assert!(
            !stale,
            "a replaced file must not authorize a different indexed identity"
        );
        assert!(
            file_denied,
            "a file with read access revoked must not appear"
        );
        assert!(
            parent_denied,
            "a file inside a directory the requester cannot list must not appear"
        );
        assert!(allowed(id), "new queries must see restored permissions");
        fixture.close().unwrap();
    }
}
