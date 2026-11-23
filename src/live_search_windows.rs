use super::*;
#[path = "live_search_windows/resume.rs"]
mod resume;
use resume::ResumeStore;
use std::sync::Arc;
use crate::{
    index_policy::RelativePathRoot,
    search::{IndexStatus, query_index_with_exclusion},
};
use ::windows::{
    Win32::{
        Foundation::*,
        Storage::FileSystem::*,
        System::{
            Com::{COINIT_APARTMENTTHREADED, CoCreateGuid, CoInitializeEx, CoUninitialize},
            IO::DeviceIoControl,
            Ioctl::{IOCTL_STORAGE_GET_DEVICE_NUMBER, STORAGE_DEVICE_NUMBER},
            Pipes::*,
            SystemInformation::GetSystemWindowsDirectoryW,
            Threading::*,
        },
        UI::{Shell::*, WindowsAndMessaging::SW_HIDE},
    },
    core::{HRESULT, PCWSTR, PWSTR},
};
use goz_core::{
    index::{FrnMap, LinkTarget, NTFS_ROOT_FRN, VolumeIndex, WtfName},
    query::{Budget, Leaf, parse_query},
    usn::walk_journal_buffer,
};
use goz_winfs::{JournalInfo, JournalQuery, VolumeHandle};
use std::{
    collections::BTreeSet,
    fs::File,
    io::{BufRead, BufReader, Write},
    os::windows::{
        fs::OpenOptionsExt,
        io::{AsRawHandle, FromRawHandle},
    },
    time::{Duration, Instant},
};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn guid_text(guid: ::windows::core::GUID) -> String {
    format!(
        "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        guid.data1,
        guid.data2,
        guid.data3,
        guid.data4[0],
        guid.data4[1],
        guid.data4[2],
        guid.data4[3],
        guid.data4[4],
        guid.data4[5],
        guid.data4[6],
        guid.data4[7]
    )
}
const DRIVE_UNKNOWN: u32 = 0;
const DRIVE_NO_ROOT_DIR: u32 = 1;
const DRIVE_REMOVABLE: u32 = 2;
const DRIVE_FIXED: u32 = 3;
const DRIVE_REMOTE: u32 = 4;
const DRIVE_CDROM: u32 = 5;
const DRIVE_RAMDISK: u32 = 6;
const MAX_VOLUME_NAME_UNITS: usize = 260;
const INITIAL_MOUNT_UNITS: usize = 256;
const MAX_MOUNT_UNITS: usize = 32 * 1024;

pub fn mounted_inventory() -> Result<super::MountedInventory, String> {
    let names = enumerate_volume_names()?;
    let mut volumes = names
        .into_iter()
        .map(|(guid, path)| inspect_volume(guid, &path))
        .collect::<Vec<_>>();
    volumes.sort_by(|left, right| left.guid.cmp(&right.guid));
    let network_drives = discover_network_drives(&volumes);
    Ok(super::MountedInventory {
        volumes,
        network_drives,
    })
}

fn windows_components_scope() -> Option<(crate::index_policy::VolumeGuid, RelativePathRoot)> {
    let mut directory = vec![0u16; MAX_MOUNT_UNITS];
    let length = unsafe { GetSystemWindowsDirectoryW(Some(&mut directory)) } as usize;
    if length == 0 || length >= directory.len() {
        return None;
    }
    let components = NativePath(directory[..length].to_vec())
        .to_path()
        .join("WinSxS");
    let mut absolute = NativePath::new(&components).0;
    absolute.push(0);
    let mut mount = vec![0u16; MAX_MOUNT_UNITS];
    unsafe { GetVolumePathNameW(PCWSTR(absolute.as_ptr()), &mut mount) }.ok()?;
    let mount_path = NativePath(nul_terminated(&mount).ok()?.to_vec()).to_path();
    let relative = components.strip_prefix(&mount_path).ok()?.to_str()?;
    let root = RelativePathRoot::parse(relative).ok()?;
    let mut name = vec![0u16; MAX_VOLUME_NAME_UNITS];
    unsafe { GetVolumeNameForVolumeMountPointW(PCWSTR(mount.as_ptr()), &mut name) }.ok()?;
    let name = String::from_utf16(nul_terminated(&name).ok()?).ok()?;
    let guid = crate::index_policy::VolumeGuid::parse(&name).ok()?;
    Some((guid, root))
}

pub fn result_mount_matches(guid: &crate::index_policy::VolumeGuid, mount: &NativePath) -> bool {
    if !valid_mount(&mount.0) {
        return false;
    }
    let mut mount_path = mount.0.clone();
    mount_path.push(0);
    let mut volume_name = vec![0u16; MAX_VOLUME_NAME_UNITS];
    if unsafe { GetVolumeNameForVolumeMountPointW(PCWSTR(mount_path.as_ptr()), &mut volume_name) }
        .is_err()
    {
        return false;
    }
    let Ok(name) = nul_terminated(&volume_name) else {
        return false;
    };
    let Ok(name) = String::from_utf16(name) else {
        return false;
    };
    crate::index_policy::VolumeGuid::parse(&name).is_ok_and(|actual| &actual == guid)
}

fn enumerate_volume_names() -> Result<Vec<(crate::index_policy::VolumeGuid, Vec<u16>)>, String> {
    let mut buffer = vec![0u16; MAX_VOLUME_NAME_UNITS];
    let handle = unsafe { FindFirstVolumeW(&mut buffer) }
        .map_err(|error| format!("FindFirstVolumeW: {error}"))?;
    let enumeration = (|| {
        let mut volumes = Vec::new();
        let mut seen = BTreeSet::new();
        loop {
            let end = buffer
                .iter()
                .position(|unit| *unit == 0)
                .ok_or_else(|| "FindVolumeW returned an unterminated volume name.".to_string())?;
            let text = String::from_utf16(&buffer[..end])
                .map_err(|error| format!("FindVolumeW returned invalid UTF-16: {error}"))?;
            let guid = crate::index_policy::VolumeGuid::parse(&text)
                .map_err(|error| format!("FindVolumeW returned an invalid GUID: {error}"))?;
            if !seen.insert(guid.clone()) {
                return Err("FindVolumeW returned a duplicate volume GUID.".into());
            }
            volumes.push((guid, buffer[..=end].to_vec()));
            match unsafe { FindNextVolumeW(handle, &mut buffer) } {
                Ok(()) => {}
                Err(error) if is_win32_error(&error, ERROR_NO_MORE_FILES) => break,
                Err(error) => return Err(format!("FindNextVolumeW: {error}")),
            }
        }
        Ok(volumes)
    })();
    let close =
        unsafe { FindVolumeClose(handle) }.map_err(|error| format!("FindVolumeClose: {error}"));
    match (enumeration, close) {
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Ok(volumes), Ok(())) => Ok(volumes),
    }
}

fn is_win32_error(error: &::windows::core::Error, expected: WIN32_ERROR) -> bool {
    error.code() == HRESULT::from_win32(expected.0)
}

fn inspect_volume(
    guid: crate::index_policy::VolumeGuid,
    volume_path: &[u16],
) -> super::InventoryVolume {
    let mut serial = 0u32;
    let mut filesystem_units = vec![0u16; 64];
    let information = unsafe {
        GetVolumeInformationW(
            PCWSTR(volume_path.as_ptr()),
            None,
            Some(&mut serial as *mut u32),
            None,
            None,
            Some(&mut filesystem_units),
        )
    };
    let (serial, filesystem, mut failure) = match information {
        Ok(()) => match nul_terminated(&filesystem_units) {
            Ok(units) => match String::from_utf16(units) {
                Ok(filesystem) if !filesystem.is_empty() => (Some(serial), Some(filesystem), None),
                Ok(_) => (
                    Some(serial),
                    None,
                    Some("파일시스템 이름이 비어 있습니다.".into()),
                ),
                Err(error) => (
                    Some(serial),
                    None,
                    Some(format!(
                        "파일시스템 이름의 UTF-16이 올바르지 않습니다: {error}"
                    )),
                ),
            },
            Err(error) => (Some(serial), None, Some(error)),
        },
        Err(error) => (None, None, Some(format!("GetVolumeInformationW: {error}"))),
    };

    let raw_drive_type = unsafe { GetDriveTypeW(PCWSTR(volume_path.as_ptr())) };
    let drive_type = match raw_drive_type {
        DRIVE_REMOVABLE => Some(crate::index_policy::DriveType::Removable),
        DRIVE_FIXED => Some(crate::index_policy::DriveType::Fixed),
        DRIVE_REMOTE | DRIVE_CDROM | DRIVE_RAMDISK => Some(crate::index_policy::DriveType::Other),
        DRIVE_UNKNOWN | DRIVE_NO_ROOT_DIR => {
            failure.get_or_insert_with(|| "볼륨 종류를 확인할 수 없습니다.".into());
            None
        }
        _ => {
            failure.get_or_insert_with(|| "알 수 없는 볼륨 종류입니다.".into());
            None
        }
    };

    let mounts = match query_mounts(volume_path) {
        Ok(mounts) if !mounts.is_empty() => mounts,
        Ok(_) => {
            failure.get_or_insert_with(|| "볼륨 마운트가 없습니다.".into());
            Vec::new()
        }
        Err(error) => {
            failure.get_or_insert_with(|| error);
            Vec::new()
        }
    };

    let classification = match (failure.is_none(), filesystem.as_deref(), drive_type) {
        (true, Some(filesystem), Some(drive_type)) => {
            match crate::index_policy::classify_ntfs_volume(filesystem, drive_type) {
                crate::index_policy::NtfsVolumeClass::Fixed => super::InventoryClass::Fixed,
                crate::index_policy::NtfsVolumeClass::Removable => super::InventoryClass::Removable,
                crate::index_policy::NtfsVolumeClass::Unsupported => {
                    super::InventoryClass::Unsupported
                }
            }
        }
        _ => super::InventoryClass::Unavailable,
    };
    super::InventoryVolume {
        guid,
        serial,
        mounts,
        classification,
        message: failure,
    }
}

fn query_mounts(volume_path: &[u16]) -> Result<Vec<NativePath>, String> {
    let mut capacity = INITIAL_MOUNT_UNITS;
    loop {
        let mut buffer = vec![0u16; capacity];
        let mut required = 0u32;
        match unsafe {
            GetVolumePathNamesForVolumeNameW(
                PCWSTR(volume_path.as_ptr()),
                Some(&mut buffer),
                &mut required,
            )
        } {
            Ok(()) => {
                let used = usize::try_from(required)
                    .map_err(|error| format!("invalid mount length: {error}"))?;
                if used == 0 || used > buffer.len() {
                    return Err(
                        "GetVolumePathNamesForVolumeNameW returned an invalid length.".into(),
                    );
                }
                let raw_mounts = split_mount_multi_sz(&buffer[..used])?;
                if raw_mounts.iter().any(|mount| !valid_mount(mount)) {
                    return Err(
                        "GetVolumePathNamesForVolumeNameW returned an invalid mount path.".into(),
                    );
                }
                let mut mounts = raw_mounts.into_iter().map(NativePath).collect::<Vec<_>>();
                mounts.sort_by(|left, right| {
                    is_drive_letter_mount(&right.0)
                        .cmp(&is_drive_letter_mount(&left.0))
                        .then_with(|| left.0.cmp(&right.0))
                });
                mounts.dedup_by(|left, right| left.0 == right.0);
                return Ok(mounts);
            }
            Err(error) if is_win32_error(&error, ERROR_MORE_DATA) => {
                let needed = usize::try_from(required)
                    .map_err(|conversion| format!("invalid required mount length: {conversion}"))?;
                if needed <= capacity || needed > MAX_MOUNT_UNITS {
                    return Err("mount lookup exceeded its bounded buffer.".into());
                }
                capacity = needed;
            }
            Err(error) => return Err(format!("GetVolumePathNamesForVolumeNameW: {error}")),
        }
    }
}

fn split_mount_multi_sz(buffer: &[u16]) -> Result<Vec<Vec<u16>>, String> {
    if buffer.len() < 2 || buffer[buffer.len() - 2..] != [0, 0] {
        return Err("mount lookup returned a malformed MULTI_SZ.".into());
    }
    let mut mounts = Vec::new();
    let mut start = 0usize;
    loop {
        let end = buffer[start..]
            .iter()
            .position(|unit| *unit == 0)
            .map(|offset| start + offset)
            .ok_or_else(|| "mount lookup returned an unterminated path.".to_string())?;
        if end == start {
            if buffer[start..].iter().any(|unit| *unit != 0) {
                return Err("mount lookup returned data after its terminator.".into());
            }
            break;
        }
        mounts.push(buffer[start..end].to_vec());
        start = end + 1;
        if start >= buffer.len() {
            return Err("mount lookup omitted its MULTI_SZ terminator.".into());
        }
    }
    Ok(mounts)
}

fn nul_terminated(buffer: &[u16]) -> Result<&[u16], String> {
    let end = buffer
        .iter()
        .position(|unit| *unit == 0)
        .ok_or_else(|| "volume information returned an unterminated name.".to_string())?;
    Ok(&buffer[..end])
}

fn valid_mount(mount: &[u16]) -> bool {
    mount.len() >= 3
        && ((65..=90).contains(&mount[0]) || (97..=122).contains(&mount[0]))
        && mount[1] == 58
        && mount[2] == 92
        && mount[mount.len() - 1] == 92
        && !mount.contains(&0)
}

fn is_drive_letter_mount(mount: &[u16]) -> bool {
    mount.len() == 3 && valid_mount(mount)
}

fn same_drive_letter_mount(left: &[u16], right: &[u16]) -> bool {
    is_drive_letter_mount(left)
        && is_drive_letter_mount(right)
        && (left[0] | 32) == (right[0] | 32)
        && left[1..] == right[1..]
}

fn discover_network_drives(volumes: &[super::InventoryVolume]) -> Vec<NativePath> {
    let mut network = Vec::new();
    for mount in crate::platform::navigation_drives() {
        if !is_drive_letter_mount(&mount.0) {
            continue;
        }
        let already_mounted = volumes
            .iter()
            .flat_map(|volume| volume.mounts.iter())
            .any(|existing| same_drive_letter_mount(&existing.0, &mount.0));
        if already_mounted
            || network
                .iter()
                .any(|existing: &NativePath| same_drive_letter_mount(&existing.0, &mount.0))
        {
            continue;
        }
        let mut terminated = mount.0.clone();
        terminated.push(0);
        if unsafe { GetDriveTypeW(PCWSTR(terminated.as_ptr())) } == DRIVE_REMOTE {
            network.push(mount);
        }
    }
    network
}

fn send(out: &mut File, e: &Event) -> Result<(), String> {
    serde_json::to_writer(&mut *out, e).map_err(|e| e.to_string())?;
    out.write_all(b"\n")
        .and_then(|_| out.flush())
        .map_err(|e| e.to_string())
}

fn send_cache_frame(out: &mut File, frame: &CacheFrame) -> Result<(), String> {
    let mut bytes = serde_json::to_vec(frame).map_err(|error| error.to_string())?;
    if bytes.len() > 64 * 1024 {
        return Err("캐시 프레임이 허용 크기를 넘었습니다.".into());
    }
    bytes.push(b'\n');
    out.write_all(&bytes)
        .and_then(|_| out.flush())
        .map_err(|error| error.to_string())
}
struct Process(HANDLE);
impl Drop for Process {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

// Keep index I/O below interactive browsing, as Everything's background monitor
// setting does. Failure to request this optional policy must not disable search.
struct BackgroundIo;
impl BackgroundIo {
    fn enter() -> Option<Self> {
        unsafe { SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN) }
            .ok()
            .map(|_| Self)
    }
}
impl Drop for BackgroundIo {
    fn drop(&mut self) {
        unsafe {
            let _ = SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_END);
        }
    }
}

const HOST_STOP_TIMEOUT: Duration = Duration::from_secs(5);
const HOST_STOP_MESSAGE: &str = "검색 호스트가 중지 중입니다.";
type SharedIoThread = std::sync::Arc<std::sync::Mutex<Option<goz_winfs::ThreadIoHandle>>>;
static SERVICE_STOP_REQUESTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
static SERVICE_IO_THREADS: std::sync::OnceLock<std::sync::Mutex<Vec<SharedIoThread>>> =
    std::sync::OnceLock::new();

fn service_io_threads() -> &'static std::sync::Mutex<Vec<SharedIoThread>> {
    SERVICE_IO_THREADS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

fn register_service_io_thread(handle: SharedIoThread) {
    {
        let mut handles = service_io_threads().lock().unwrap();
        if !handles
            .iter()
            .any(|current| std::sync::Arc::ptr_eq(current, &handle))
        {
            handles.push(handle.clone());
        }
    }
    if SERVICE_STOP_REQUESTED.load(std::sync::atomic::Ordering::Acquire) {
        if let Some(handle) = handle.lock().unwrap().as_ref() {
            let _ = goz_winfs::cancel_synchronous_io(handle);
        }
    }
}

pub(super) fn request_service_stop() {
    SERVICE_STOP_REQUESTED.store(true, std::sync::atomic::Ordering::Release);
    cancel_service_io();
}

fn cancel_service_io() {
    let handles = service_io_threads().lock().unwrap().clone();
    for shared in handles {
        if let Some(handle) = shared.lock().unwrap().as_ref() {
            let _ = goz_winfs::cancel_synchronous_io(handle);
        }
    }
}

pub(super) fn service_stop_requested() -> bool {
    SERVICE_STOP_REQUESTED.load(std::sync::atomic::Ordering::Acquire)
}

pub(super) fn reset_service_stop() {
    SERVICE_STOP_REQUESTED.store(false, std::sync::atomic::Ordering::Release);
    service_io_threads().lock().unwrap().clear();
}

struct HostStop {
    requested: std::sync::atomic::AtomicBool,
    worker_returned: std::sync::atomic::AtomicBool,
    normal_returned: std::sync::atomic::AtomicBool,
    io_thread: SharedIoThread,
    cancel_requested: std::sync::atomic::AtomicBool,
    native_returned: std::sync::atomic::AtomicBool,
    native_aborted: std::sync::atomic::AtomicBool,
    service_mode: bool,
}

impl Default for HostStop {
    fn default() -> Self {
        Self {
            requested: std::sync::atomic::AtomicBool::new(false),
            worker_returned: std::sync::atomic::AtomicBool::new(false),
            normal_returned: std::sync::atomic::AtomicBool::new(false),
            io_thread: std::sync::Arc::new(std::sync::Mutex::new(None)),
            cancel_requested: std::sync::atomic::AtomicBool::new(false),
            native_returned: std::sync::atomic::AtomicBool::new(false),
            native_aborted: std::sync::atomic::AtomicBool::new(false),
            service_mode: false,
        }
    }
}

impl HostStop {
    fn new(service_mode: bool) -> std::sync::Arc<Self> {
        let stop = std::sync::Arc::new(Self {
            service_mode,
            ..Self::default()
        });
        if service_mode {
            register_service_io_thread(stop.io_thread.clone());
        }
        stop
    }

    fn attach_current_thread(&self) -> Result<(), String> {
        let handle = goz_winfs::current_thread_io_handle().map_err(|error| error.to_string())?;
        *self.io_thread.lock().unwrap() = Some(handle);
        if self.service_mode {
            if SERVICE_STOP_REQUESTED.load(std::sync::atomic::Ordering::Acquire) {
                if let Some(handle) = self.io_thread.lock().unwrap().as_ref() {
                    let _ = goz_winfs::cancel_synchronous_io(handle);
                }
            }
        }
        Ok(())
    }

    fn is_requested(&self) -> bool {
        self.requested.load(std::sync::atomic::Ordering::Acquire)
            || (self.service_mode
                && SERVICE_STOP_REQUESTED.load(std::sync::atomic::Ordering::Acquire))
    }

    fn before_native(&self) -> Result<(), String> {
        if self.is_requested() {
            Err(HOST_STOP_MESSAGE.into())
        } else {
            Ok(())
        }
    }

    fn after_native<T>(&self, result: Result<T, String>) -> Result<T, String> {
        if self.is_requested() {
            self.native_returned
                .store(true, std::sync::atomic::Ordering::Release);
            Err(HOST_STOP_MESSAGE.into())
        } else {
            result
        }
    }

    fn after_winfs<T>(&self, result: Result<T, goz_winfs::WinError>) -> Result<T, String> {
        if self.is_requested() {
            self.native_returned
                .store(true, std::sync::atomic::Ordering::Release);
            if result
                .as_ref()
                .is_err_and(|error| error.code == ERROR_OPERATION_ABORTED.0)
            {
                self.native_aborted
                    .store(true, std::sync::atomic::Ordering::Release);
            }
            Err(HOST_STOP_MESSAGE.into())
        } else {
            result.map_err(|error| error.to_string())
        }
    }

    fn after_windows<T>(&self, result: ::windows::core::Result<T>) -> Result<T, String> {
        if self.is_requested() {
            self.native_returned
                .store(true, std::sync::atomic::Ordering::Release);
            if result
                .as_ref()
                .is_err_and(|error| error.code() == HRESULT::from_win32(ERROR_OPERATION_ABORTED.0))
            {
                self.native_aborted
                    .store(true, std::sync::atomic::Ordering::Release);
            }
            Err(HOST_STOP_MESSAGE.into())
        } else {
            result.map_err(|error| error.to_string())
        }
    }

    fn supervise_eof(&self) {
        self.requested
            .store(true, std::sync::atomic::Ordering::Release);
        let deadline = Instant::now() + HOST_STOP_TIMEOUT;
        while !self
            .worker_returned
            .load(std::sync::atomic::Ordering::Acquire)
        {
            if let Some(handle) = self.io_thread.lock().unwrap().as_ref() {
                match goz_winfs::cancel_synchronous_io(handle) {
                    Ok(true) => self
                        .cancel_requested
                        .store(true, std::sync::atomic::Ordering::Release),
                    Ok(false) | Err(_) => {}
                }
            }
            if Instant::now() >= deadline {
                if !self.service_mode {
                    unsafe {
                        let _ = TerminateProcess(GetCurrentProcess(), 0xE000_0001);
                    }
                }
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

fn host_winfs<T>(
    stop: &HostStop,
    call: impl FnOnce() -> Result<T, goz_winfs::WinError>,
) -> Result<T, String> {
    stop.before_native()?;
    stop.after_winfs(call())
}

fn host_windows<T>(
    stop: &HostStop,
    call: impl FnOnce() -> ::windows::core::Result<T>,
) -> Result<T, String> {
    stop.before_native()?;
    stop.after_windows(call())
}

struct HostRunReturned {
    stop: std::sync::Arc<HostStop>,
    normal: bool,
}

impl Drop for HostRunReturned {
    fn drop(&mut self) {
        self.stop
            .worker_returned
            .store(true, std::sync::atomic::Ordering::Release);
        if self.normal {
            self.stop
                .normal_returned
                .store(true, std::sync::atomic::Ordering::Release);
        }
    }
}

// Private pipe DACL: current user, Administrators and SYSTEM only. No world-readable metadata.
struct PipeSecurity(::windows::Win32::Security::PSECURITY_DESCRIPTOR);
impl Drop for PipeSecurity {
    fn drop(&mut self) {
        unsafe {
            let _ = LocalFree(Some(HLOCAL(self.0.0)));
        }
    }
}
impl PipeSecurity {
    unsafe fn current_user() -> Result<Self, String> {
        use ::windows::Win32::Security::{Authorization::*, *};
        let mut raw = HANDLE::default();
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) }
            .map_err(|e| e.to_string())?;
        let token = Process(raw);
        let mut size = 0;
        let _ = unsafe { GetTokenInformation(token.0, TokenUser, None, 0, &mut size) };
        let mut buffer = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
        unsafe {
            GetTokenInformation(
                token.0,
                TokenUser,
                Some(buffer.as_mut_ptr().cast()),
                size,
                &mut size,
            )
        }
        .map_err(|e| e.to_string())?;
        let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
        let mut sid = PWSTR::null();
        unsafe { ConvertSidToStringSidW(user.User.Sid, &mut sid) }.map_err(|e| e.to_string())?;
        let sid_text = unsafe { sid.to_string() };
        unsafe {
            let _ = LocalFree(Some(HLOCAL(sid.0.cast())));
        }
        let sddl = wide(&format!(
            "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;{})",
            sid_text.map_err(|e| e.to_string())?
        ));
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(sddl.as_ptr()),
                1,
                &mut descriptor,
                None,
            )
        }
        .map_err(|e| e.to_string())?;
        Ok(Self(descriptor))
    }

    unsafe fn service_owner(owner_sid: &str) -> Result<Self, String> {
        use ::windows::Win32::Security::{Authorization::*, *};
        let sddl = wide(&format!("D:P(A;;GA;;;SY)(A;;GA;;;{owner_sid})"));
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(sddl.as_ptr()),
                1,
                &mut descriptor,
                None,
            )
        }
        .map_err(|error| format!("서비스 파이프 보안 설명자를 만들지 못했습니다: {error}"))?;
        Ok(Self(descriptor))
    }
}

const HOST_WRITER_STOP_TIMEOUT: Duration = Duration::from_secs(2);
const HOST_STOP_GRACE: Duration = Duration::from_secs(6);
const HOST_FORCE_EXIT_CODE: u32 = 0xE000_0002;

type HostWriteResult = (HostCommand, Result<(), String>);

struct ProxyCommandWriter {
    sender: Option<crossbeam_channel::Sender<HostCommand>>,
    results: crossbeam_channel::Receiver<HostWriteResult>,
    done: crossbeam_channel::Receiver<()>,
    io_thread: std::sync::Arc<std::sync::Mutex<Option<goz_winfs::ThreadIoHandle>>>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl ProxyCommandWriter {
    fn start(pipe: File) -> Result<Self, String> {
        let (sender, receiver) = crossbeam_channel::bounded::<HostCommand>(16);
        let (result_tx, results) = crossbeam_channel::unbounded::<HostWriteResult>();
        let (ready_tx, ready_rx) = crossbeam_channel::bounded::<Result<(), String>>(1);
        let (done_tx, done) = crossbeam_channel::bounded::<()>(1);
        let io_thread = std::sync::Arc::new(std::sync::Mutex::new(None));
        let writer_io = io_thread.clone();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let writer_stop = stop.clone();
        std::thread::spawn(move || {
            let handle = match goz_winfs::current_thread_io_handle() {
                Ok(handle) => handle,
                Err(error) => {
                    let _ = ready_tx.send(Err(error.to_string()));
                    return;
                }
            };
            *writer_io.lock().unwrap() = Some(handle);
            if ready_tx.send(Ok(())).is_err() {
                return;
            }
            let mut pipe = pipe;
            while let Ok(command) = receiver.recv() {
                if writer_stop.load(std::sync::atomic::Ordering::Acquire) {
                    break;
                }
                let result = write_command(&mut pipe, &command);
                let failed = result.is_err();
                let _ = result_tx.send((command, result));
                if failed {
                    break;
                }
            }
            drop(pipe);
            writer_io.lock().unwrap().take();
            let _ = done_tx.send(());
        });
        match ready_rx.recv_timeout(Duration::from_secs(2)) {
            Ok(Ok(())) => Ok(Self {
                sender: Some(sender),
                results,
                done,
                io_thread,
                stop,
            }),
            Ok(Err(error)) => Err(format!(
                "검색 명령 작성 스레드를 시작하지 못했습니다: {error}"
            )),
            Err(error) => Err(format!("검색 명령 작성 스레드 응답이 없습니다: {error}")),
        }
    }

    fn try_send(&self, command: HostCommand) -> Result<(), HostCommand> {
        let Some(sender) = &self.sender else {
            return Err(command);
        };
        match sender.try_send(command) {
            Ok(()) => Ok(()),
            Err(crossbeam_channel::TrySendError::Full(command))
            | Err(crossbeam_channel::TrySendError::Disconnected(command)) => Err(command),
        }
    }

    fn command_sender(&self) -> Result<crossbeam_channel::Sender<HostCommand>, String> {
        self.sender
            .as_ref()
            .cloned()
            .ok_or_else(|| "검색 명령 작성기가 종료됐습니다.".into())
    }

    fn stop(&mut self, timeout: Duration) -> Result<bool, String> {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        self.sender.take();
        let deadline = Instant::now() + timeout;
        let mut cancel_error = None;
        loop {
            match self.done.try_recv() {
                Ok(()) | Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    return match cancel_error {
                        Some(error) => Err(error),
                        None => Ok(true),
                    };
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {}
            }
            if let Some(handle) = self.io_thread.lock().unwrap().as_ref() {
                if let Err(error) = goz_winfs::cancel_synchronous_io(handle) {
                    cancel_error = Some(format!("검색 명령 I/O 취소에 실패했습니다: {error}"));
                }
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

struct ProxyHostOwner {
    child: Option<Process>,
    pid: u32,
    terminate_on_timeout: bool,
    session: u64,
    events: crossbeam_channel::Sender<Event>,
    commands: Option<File>,
    writer: Option<ProxyCommandWriter>,
}

impl ProxyHostOwner {
    fn child(&self) -> &Process {
        self.child.as_ref().expect("owned host process is present")
    }

    fn shutdown(&mut self) -> Result<(u32, bool), String> {
        if !self.terminate_on_timeout {
            return self.shutdown_service();
        }
        let mut writer_done = true;
        let mut writer_error = None;
        if let Some(writer) = self.writer.as_mut() {
            match writer.stop(HOST_WRITER_STOP_TIMEOUT) {
                Ok(done) => writer_done = done,
                Err(error) => {
                    writer_error = Some(error);
                    writer_done = false;
                }
            }
        }
        self.commands.take();
        let mut forced = false;
        let mut exit_code = None;
        let mut wait_error = None;
        if !writer_done {
            let (code, did_force) = terminate_owned_host(self.child())?;
            exit_code = Some(code);
            forced = did_force;
            if let Some(writer) = self.writer.as_mut() {
                match writer.stop(HOST_WRITER_STOP_TIMEOUT) {
                    Ok(true) => writer_done = true,
                    Ok(false) => {}
                    Err(error) => writer_error = Some(error),
                }
            }
        }
        if exit_code.is_none() {
            match wait_owned_host(self.child(), HOST_STOP_GRACE) {
                Ok(Some(code)) => exit_code = Some(code),
                Ok(None) => {
                    let (code, did_force) = terminate_owned_host(self.child())?;
                    exit_code = Some(code);
                    forced = did_force;
                }
                Err(error) => {
                    wait_error = Some(error.clone());
                    match terminate_owned_host(self.child()) {
                        Ok((code, did_force)) => {
                            exit_code = Some(code);
                            forced = did_force;
                        }
                        Err(terminate_error) => {
                            return Err(format!("{error}; {terminate_error}"));
                        }
                    }
                }
            }
        }
        let exit_code = exit_code.expect("host cleanup must record a native exit code");
        if !writer_done {
            return Err(writer_error
                .unwrap_or_else(|| "검색 명령 작성 스레드가 종료되지 않았습니다.".into()));
        }
        self.child.take();
        if let Some(error) = wait_error {
            return Err(format!(
                "검색 호스트 종료 대기가 실패했습니다: {error} (exit code {exit_code})"
            ));
        }
        if let Some(error) = writer_error {
            return Err(error);
        }
        Ok((exit_code, forced))
    }

    fn shutdown_service(&mut self) -> Result<(u32, bool), String> {
        let mut writer_done = true;
        if let Some(writer) = self.writer.as_mut() {
            writer_done = writer
                .stop(HOST_WRITER_STOP_TIMEOUT)
                .map_err(|error| format!("서비스 검색 명령 정리가 실패했습니다: {error}"))?;
        }
        self.commands.take();
        if !writer_done {
            return Err(format!(
                "SDIR 인덱스 서비스 PID {}의 명령 작성 스레드가 종료되지 않았습니다.",
                self.pid
            ));
        }
        let exit_code = wait_owned_host(self.child(), HOST_STOP_GRACE)?.ok_or_else(|| {
            format!(
                "SDIR 인덱스 서비스 PID {}가 파이프 EOF 뒤 6초 안에 종료되지 않았습니다.",
                self.pid
            )
        })?;
        self.child.take();
        Ok((exit_code, false))
    }

    fn wait_until_exit(&mut self) -> Result<u32, String> {
        if self.child().0.is_invalid() {
            return Err("검색 호스트를 감시할 유효한 프로세스 핸들이 없습니다.".into());
        }
        let mut wait_error_reported = false;
        let mut writer_error_reported = false;
        loop {
            let exit_code = match wait_owned_host(self.child(), Duration::from_secs(1)) {
                Ok(code) => code,
                Err(error) => {
                    if !wait_error_reported {
                        wait_error_reported = true;
                        let _ = self.events.send(Event::Failure {
                            lane: 4,
                            id: self.session,
                            message: format!("검색 호스트 종료 감시가 실패했습니다: {error}"),
                        });
                    }
                    match query_owned_host_exit(self.child()) {
                        Ok(code) => code,
                        Err(query_error) => {
                            if !writer_error_reported {
                                writer_error_reported = true;
                                let _ = self.events.send(Event::Failure {
                                    lane: 4,
                                    id: self.session,
                                    message: format!(
                                        "검색 호스트 종료 코드를 조회하지 못했습니다: {query_error}"
                                    ),
                                });
                            }
                            None
                        }
                    }
                }
            };
            if let Some(code) = exit_code {
                let writer_done = match self.writer.as_mut() {
                    Some(writer) => match writer.stop(HOST_WRITER_STOP_TIMEOUT) {
                        Ok(done) => done,
                        Err(error) => {
                            if !writer_error_reported {
                                writer_error_reported = true;
                                let _ = self.events.send(Event::Failure {
                                    lane: 4,
                                    id: self.session,
                                    message: format!(
                                        "검색 명령 작성 스레드 정리가 실패했습니다: {error}"
                                    ),
                                });
                            }
                            false
                        }
                    },
                    None => true,
                };
                if writer_done {
                    self.child.take();
                    return Ok(code);
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

fn validate_owned_host(child: &Process) -> Result<u32, String> {
    if child.0.is_invalid() {
        return Err("검색 호스트 프로세스 핸들이 유효하지 않습니다.".into());
    }
    let pid = unsafe { GetProcessId(child.0) };
    if pid == 0 {
        return Err(format!(
            "검색 호스트 PID를 확인하지 못했습니다: {}",
            std::io::Error::last_os_error()
        ));
    }
    let mut exit_code = 0u32;
    unsafe { GetExitCodeProcess(child.0, &mut exit_code) }
        .map_err(|error| format!("검색 호스트 프로세스 권한을 확인하지 못했습니다: {error}"))?;
    Ok(pid)
}

fn query_owned_host_exit(child: &Process) -> Result<Option<u32>, String> {
    let mut exit_code = 0u32;
    unsafe { GetExitCodeProcess(child.0, &mut exit_code) }
        .map_err(|error| format!("검색 호스트 종료 코드 조회가 실패했습니다: {error}"))?;
    if exit_code == STILL_ACTIVE.0 as u32 {
        Ok(None)
    } else {
        Ok(Some(exit_code))
    }
}

fn wait_owned_host(child: &Process, timeout: Duration) -> Result<Option<u32>, String> {
    let millis = timeout.as_millis().min(u128::from(u32::MAX)) as u32;
    let result = unsafe { WaitForSingleObject(child.0, millis) };
    if result == WAIT_OBJECT_0 {
        let mut exit_code = 0u32;
        unsafe { GetExitCodeProcess(child.0, &mut exit_code) }
            .map_err(|error| format!("검색 호스트 종료 코드를 읽지 못했습니다: {error}"))?;
        if exit_code == STILL_ACTIVE.0 as u32 {
            return Err("검색 호스트 핸들이 신호 상태지만 종료 코드는 STILL_ACTIVE입니다.".into());
        }
        Ok(Some(exit_code))
    } else if result == WAIT_TIMEOUT {
        Ok(None)
    } else if result == WAIT_FAILED {
        Err(format!(
            "검색 호스트 종료 대기가 실패했습니다: {}",
            std::io::Error::last_os_error()
        ))
    } else {
        Err(format!(
            "검색 호스트 종료 대기에서 예상하지 않은 결과를 받았습니다: {result:?}"
        ))
    }
}

struct ParentIdentity {
    process: Process,
    pid: u32,
    created_filetime: u64,
    owner_sid: Option<String>,
}

impl ParentIdentity {
    fn validate(&self) -> Result<(), String> {
        if unsafe { GetProcessId(self.process.0) } != self.pid {
            return Err("검색 요청 부모 프로세스의 열린 PID가 바뀌었습니다.".into());
        }
        match unsafe { WaitForSingleObject(self.process.0, 0) } {
            WAIT_TIMEOUT => {}
            WAIT_OBJECT_0 => return Err("검색 요청 부모 프로세스가 이미 종료됐습니다.".into()),
            WAIT_FAILED => {
                return Err(format!(
                    "검색 요청 부모 프로세스 상태를 확인하지 못했습니다: {}",
                    std::io::Error::last_os_error()
                ));
            }
            result => return Err(format!("검색 요청 부모 상태가 잘못됐습니다: {result:?}")),
        }
        let revalidated = crate::live_search::service::validate_parent_process(
            self.pid,
            self.created_filetime,
            self.owner_sid.as_deref(),
        )?;
        unsafe {
            let _ = CloseHandle(revalidated);
        }
        Ok(())
    }
}

fn send_authenticated(
    pipe: &mut File,
    parent: &ParentIdentity,
    event: &Event,
) -> Result<(), String> {
    parent.validate()?;
    send(pipe, event)
}

fn terminate_owned_host(child: &Process) -> Result<(u32, bool), String> {
    if let Some(code) = wait_owned_host(child, Duration::ZERO)? {
        return Ok((code, false));
    }
    if let Err(error) = unsafe { TerminateProcess(child.0, HOST_FORCE_EXIT_CODE) } {
        if let Some(code) = wait_owned_host(child, Duration::ZERO)? {
            return Ok((code, false));
        }
        return Err(format!("검색 호스트 제한 종료가 실패했습니다: {error}"));
    }
    let code = wait_owned_host(child, Duration::from_secs(2))?.ok_or_else(|| {
        "검색 호스트 강제 종료 후에도 프로세스 핸들이 신호 상태가 되지 않았습니다.".to_string()
    })?;
    Ok((code, true))
}

fn proxy_event_output() -> crossbeam_channel::Sender<Event> {
    let (sender, receiver) = crossbeam_channel::unbounded::<Event>();
    std::thread::spawn(move || {
        while let Ok(event) = receiver.recv() {
            if emit(&event).is_err() {
                break;
            }
        }
    });
    sender
}

fn supervise_proxy_requests(
    queued: Vec<Request>,
    requests: crossbeam_channel::Receiver<Request>,
    forwarded: crossbeam_channel::Sender<Request>,
    input_closed: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    for request in queued.into_iter().chain(requests) {
        if matches!(&request, Request::ProxyInputClosed) {
            break;
        }
        if forwarded.send(request).is_err() {
            break;
        }
    }
    input_closed.store(true, std::sync::atomic::Ordering::Release);
}

fn check_host_exit(exit_code: u32) -> Result<(), String> {
    if exit_code == 0 {
        Ok(())
    } else {
        Err(format!(
            "검색 호스트가 종료됐습니다 (exit code {exit_code})."
        ))
    }
}

enum ProxyAckPoll {
    Ack(bool, Option<String>),
    Stopped,
    Pending,
}

fn validate_host_event_pid(event: &Event, expected: u32) -> Result<(), String> {
    let actual = match event {
        Event::VolumeIndex { host_pid, .. } => Some(Some(*host_pid)),
        Event::VolumeSearch { host_pid, .. }
        | Event::ConfigAck { host_pid, .. }
        | Event::RebuildAck { host_pid, .. } => Some(*host_pid),
        _ => None,
    };
    if actual.is_some_and(|actual| actual != Some(expected)) {
        return Err(format!(
            "검색 서비스 이벤트 PID가 확인된 호스트 PID {expected}와 다릅니다."
        ));
    }
    Ok(())
}

fn poll_proxy_ack(
    ack: &crossbeam_channel::Receiver<(bool, Option<String>)>,
    input_closed: &std::sync::atomic::AtomicBool,
    events_done: &std::sync::atomic::AtomicBool,
    child: &Process,
    started: Instant,
    event_error: &std::sync::Mutex<Option<String>>,
) -> Result<ProxyAckPoll, String> {
    if let Some(error) = event_error.lock().unwrap().clone() {
        return Err(error);
    }
    if input_closed.load(std::sync::atomic::Ordering::Acquire) {
        return Ok(ProxyAckPoll::Stopped);
    }
    if let Some(exit_code) = wait_owned_host(child, Duration::ZERO)? {
        return Err(format!(
            "검색 호스트가 초기 설정 응답 전에 종료됐습니다 (exit code {exit_code})."
        ));
    }
    if events_done.load(std::sync::atomic::Ordering::Acquire) {
        return Err("검색 호스트 설정 응답 파이프가 초기 응답 전에 닫혔습니다.".into());
    }
    match ack.recv_timeout(Duration::from_millis(50)) {
        Ok((accepted, message)) => Ok(ProxyAckPoll::Ack(accepted, message)),
        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
            Err("검색 호스트 설정 응답을 읽지 못했습니다.".into())
        }
        Err(crossbeam_channel::RecvTimeoutError::Timeout)
            if started.elapsed() > Duration::from_secs(20) =>
        {
            Err("검색 호스트 초기 설정 응답 시간이 초과됐습니다.".into())
        }
        Err(crossbeam_channel::RecvTimeoutError::Timeout) => Ok(ProxyAckPoll::Pending),
    }
}

pub(super) fn proxy(
    id: u64,
    memory_mib: u16,
    service_only: bool,
    settings: crate::index_policy::NtfsIndexingSettings,
    queued: Vec<Request>,
    requests: crossbeam_channel::Receiver<Request>,
) -> Result<(), String> {
    let input_closed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (request_tx, request_rx) = crossbeam_channel::unbounded::<Request>();
    let request_stop = input_closed.clone();
    std::thread::spawn(move || {
        supervise_proxy_requests(queued, requests, request_tx, request_stop);
    });
    if input_closed.load(std::sync::atomic::Ordering::Acquire) {
        return Ok(());
    }
    if !settings.is_valid() {
        emit(&Event::ConfigAck {
            session: id,
            generation: 0,
            host_pid: None,
            accepted: false,
            message: Some("검색 설정이 올바르지 않습니다.".into()),
        })?;
        return Ok(());
    }

    let memory_mib = memory_mib.clamp(128, 2048);
    let service = match crate::live_search::service::open_for_client() {
        Ok(Some(service)) => Some(service),
        Ok(None) if !service_only => None,
        Err(error) if !service_only => return Err(error),
        outcome => {
            let message = match outcome {
                Err(error) => format!("색인 서비스를 사용할 수 없어 자동 복원을 대기합니다. {error}"),
                Ok(_) => "색인 서비스가 설치되지 않아 자동 복원을 대기합니다.".into(),
            };
            emit(&Event::ConfigAck {
                session: id,
                generation: 0,
                host_pid: None,
                accepted: false,
                message: Some(message),
            })?;
            return Ok(());
        }
    };
    let output_events = proxy_event_output();
    let service_mode = service.is_some();
    let parent_pid = std::process::id();
    let parent_created_filetime = crate::live_search::service::current_process_created_filetime()?;
    let pipe_id = guid_text(unsafe { CoCreateGuid() }.map_err(|e| e.to_string())?);
    let name = if service_mode {
        format!(r"\\.\pipe\SDIR-index-v1-{pipe_id}")
    } else {
        format!(r"\\.\pipe\SDIR-search-{{{pipe_id}}}")
    };
    let security = match service.as_ref() {
        Some(service) => unsafe { PipeSecurity::service_owner(service.owner_sid())? },
        None => unsafe { PipeSecurity::current_user()? },
    };
    let attributes = ::windows::Win32::Security::SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<::windows::Win32::Security::SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: security.0.0,
        bInheritHandle: false.into(),
    };
    let events = create_server_pipe(&name, &attributes)?;
    let commands = create_server_pipe(&format!("{name}-queries"), &attributes)?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let file = wide(&exe.to_string_lossy());
    if input_closed.load(std::sync::atomic::Ordering::Acquire) {
        return Ok(());
    }
    let (child, host_pid, terminate_on_timeout) = if let Some(service) = service.as_ref() {
        let started = service.start(
            &pipe_id,
            parent_pid,
            parent_created_filetime,
            id,
            memory_mib,
        )?;
        (Process(started.handle), started.pid, false)
    } else {
        let verb = wide("runas");
        let args = wide(&format!(
            "--ntfs-host --pipe \"{name}\" --parent {parent_pid} --parent-created-filetime {parent_created_filetime} --index-id {id} --memory-mib {memory_mib}"
        ));
        struct Com;
        impl Drop for Com {
            fn drop(&mut self) {
                unsafe { CoUninitialize() };
            }
        }
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
            .ok()
            .map_err(|e| e.to_string())?;
        let _com = Com;
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
            lpVerb: PCWSTR(verb.as_ptr()),
            lpFile: PCWSTR(file.as_ptr()),
            lpParameters: PCWSTR(args.as_ptr()),
            nShow: SW_HIDE.0,
            ..Default::default()
        };
        if let Err(error) = unsafe { ShellExecuteExW(&mut info) } {
            if !info.hProcess.is_invalid() {
                unsafe {
                    let _ = CloseHandle(info.hProcess);
                }
            }
            return Err(if error.code() == HRESULT::from_win32(ERROR_CANCELLED.0) {
                "전체 드라이브 검색 권한 요청을 취소했습니다. 폴더 검색은 그대로 사용할 수 있습니다.".into()
            } else {
                format!("검색 프로세스를 시작하지 못했습니다: {error}")
            });
        }
        let process = Process(info.hProcess);
        let pid = validate_owned_host(&process)?;
        (process, pid, true)
    };
    let mut owner = ProxyHostOwner {
        child: Some(child),
        pid: host_pid,
        terminate_on_timeout,
        session: id,
        events: output_events.clone(),
        commands: Some(commands),
        writer: None,
    };
    let mut event_pipe = Some(events);
    let run_result = (|| -> Result<(), String> {
        let expected = validate_owned_host(owner.child())?;
        if expected != owner.pid {
            return Err("실행 컨트롤러 PID와 검색 호스트 PID가 다릅니다.".into());
        }
        let child_handle = owner.child().0;
        if let Err(error) = connect_server_pipe(
            event_pipe.as_ref().unwrap(),
            expected,
            child_handle,
            &input_closed,
        ) {
            if input_closed.load(std::sync::atomic::Ordering::Acquire) {
                return Ok(());
            }
            return Err(error);
        }
        if input_closed.load(std::sync::atomic::Ordering::Acquire) {
            return Ok(());
        }
        if let Err(error) = connect_server_pipe(
            owner.commands.as_ref().unwrap(),
            expected,
            child_handle,
            &input_closed,
        ) {
            if input_closed.load(std::sync::atomic::Ordering::Acquire) {
                return Ok(());
            }
            return Err(error);
        }
        if input_closed.load(std::sync::atomic::Ordering::Acquire) {
            return Ok(());
        }

        let events_done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let reader_done = events_done.clone();
        let event_output = output_events.clone();
        let event_error = std::sync::Arc::new(std::sync::Mutex::new(None));
        let reader_error = event_error.clone();
        let expected_event_pid = owner.pid;
        let (ack_tx, ack_rx) = crossbeam_channel::bounded::<(bool, Option<String>)>(1);
        std::thread::spawn(move || {
            for line in BufReader::new(event_pipe.take().unwrap()).lines() {
                let Ok(line) = line else { break };
                if line.len() > 2_000_000 {
                    break;
                }
                if let Ok(event) = serde_json::from_str::<Event>(&line) {
                    if let Err(error) = validate_host_event_pid(&event, expected_event_pid) {
                        *reader_error.lock().unwrap() = Some(error.clone());
                        let _ = ack_tx.try_send((false, Some(error)));
                        break;
                    }
                    if let Event::ConfigAck {
                        session,
                        generation,
                        accepted,
                        message,
                        ..
                    } = &event
                    {
                        if *session == id && *generation == 0 {
                            let _ = ack_tx.try_send((*accepted, message.clone()));
                        }
                    }
                    if event_output.send(event).is_err() {
                        break;
                    }
                }
            }
            reader_done.store(true, std::sync::atomic::Ordering::Release);
        });

        let command_pipe = owner.commands.take().unwrap();
        owner.writer = Some(ProxyCommandWriter::start(command_pipe)?);
        let inbox = CommandInbox::default();
        let initial = HostCommand::Configure {
            session: id,
            generation: 0,
            settings: Box::new(settings),
        };
        owner
            .writer
            .as_ref()
            .unwrap()
            .try_send(initial)
            .map_err(|_| "검색 초기 설정 명령 대기열이 가득 찼습니다.".to_string())?;
        let started = Instant::now();
        let accepted = loop {
            if let Some(error) = event_error.lock().unwrap().clone() {
                inbox.close();
                return Err(error);
            }
            if input_closed.load(std::sync::atomic::Ordering::Acquire) {
                inbox.close();
                return Ok(());
            }
            for request in request_rx.try_iter() {
                if input_closed.load(std::sync::atomic::Ordering::Acquire) {
                    break;
                }
                enqueue_proxy_request(&inbox, request, &output_events);
            }
            while let Ok((command, result)) = owner.writer.as_ref().unwrap().results.try_recv() {
                if let Err(message) = result {
                    let _ = output_events.send(rejected_command(command, None, message.clone()));
                    return Err(format!(
                        "검색 초기 Configure 명령을 전송하지 못했습니다: {message}"
                    ));
                }
            }
            match poll_proxy_ack(
                &ack_rx,
                &input_closed,
                &events_done,
                owner.child(),
                started,
                &event_error,
            )? {
                ProxyAckPoll::Ack(accepted, message) => break (accepted, message),
                ProxyAckPoll::Stopped => {
                    inbox.close();
                    return Ok(());
                }
                ProxyAckPoll::Pending => {}
            }
        };
        if !accepted.0 {
            inbox.close();
            return Ok(());
        }

        let mut queued = std::collections::VecDeque::new();
        while !input_closed.load(std::sync::atomic::Ordering::Acquire) {
            if let Some(error) = event_error.lock().unwrap().clone() {
                return Err(error);
            }
            let mut progressed = false;
            for request in request_rx.try_iter() {
                if input_closed.load(std::sync::atomic::Ordering::Acquire) {
                    break;
                }
                queued.push_back(request);
            }
            if input_closed.load(std::sync::atomic::Ordering::Acquire) {
                break;
            }
            for _ in 0..4 {
                let Some(request) = queued.pop_front() else {
                    break;
                };
                progressed = true;
                enqueue_proxy_request(&inbox, request, &output_events);
            }
            let writer = owner.writer.as_ref().unwrap();
            for command in inbox.pop_batch(4) {
                progressed = true;
                if let Err(command) = writer.try_send(command) {
                    let _ = output_events.send(rejected_command(
                        command,
                        None,
                        "검색 호스트 명령 작성기가 가득 찼습니다.".into(),
                    ));
                }
            }
            while let Ok((command, result)) = writer.results.try_recv() {
                if let Err(message) = result {
                    let _ = output_events.send(rejected_command(command, None, message));
                }
            }
            if let Some(exit_code) = wait_owned_host(owner.child(), Duration::ZERO)? {
                return Err(format!(
                    "전체 드라이브 검색 프로세스가 종료됐습니다 (exit code {exit_code})."
                ));
            }
            if events_done.load(std::sync::atomic::Ordering::Acquire) {
                return Err("검색 호스트 이벤트 파이프가 닫혔습니다.".into());
            }
            if !progressed {
                match request_rx.recv_timeout(Duration::from_millis(25)) {
                    Ok(request) => queued.push_back(request),
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                }
            }
        }
        inbox.close();
        Ok(())
    })();
    let cleanup_result = owner.shutdown();
    if owner.child.is_some() {
        let failure = match (&run_result, &cleanup_result) {
            (Err(run_error), Err(cleanup_error)) => format!("{run_error}; {cleanup_error}"),
            (Err(run_error), Ok((exit_code, forced))) => {
                format!("{run_error} (exit code {exit_code}, forced={forced})")
            }
            (Ok(()), Err(cleanup_error)) => cleanup_error.clone(),
            (Ok(()), Ok(_)) => "검색 호스트 프로세스가 아직 소유자에게 남아 있습니다.".into(),
        };
        let _ = output_events.send(Event::Failure {
            lane: 4,
            id,
            message: failure.clone(),
        });
        if !owner.terminate_on_timeout {
            return match query_owned_host_exit(owner.child()) {
                Ok(Some(exit_code)) => {
                    owner.child.take();
                    Err(format!(
                        "{failure} (SDIR 인덱스 서비스 PID {}의 native exit code {exit_code})",
                        owner.pid
                    ))
                }
                Ok(None) => Err(format!(
                    "{failure}; SDIR 인덱스 서비스 PID {}가 아직 실행 중이며 클라이언트는 강제 종료 권한을 갖지 않습니다.",
                    owner.pid
                )),
                Err(error) => Err(format!(
                    "{failure}; SDIR 인덱스 서비스 PID {}의 종료 상태를 확인하지 못했습니다: {error}",
                    owner.pid
                )),
            };
        }
        return match owner.wait_until_exit() {
            Ok(exit_code) => Err(format!(
                "{failure} (검색 호스트 native exit code {exit_code})"
            )),
            Err(wait_error) => Err(format!("{failure}; {wait_error}")),
        };
    }
    match (run_result, cleanup_result) {
        (Ok(()), Ok((exit_code, false))) => check_host_exit(exit_code),
        (Ok(()), Ok((exit_code, true))) => Err(format!(
            "검색 호스트가 제한 시간 뒤 native fallback으로 종료됐습니다 (exit code {exit_code})."
        )),
        (Err(run_error), Ok((exit_code, forced))) => Err(format!(
            "{run_error} (검색 호스트 exit code {exit_code}, forced={forced})"
        )),
        (Ok(()), Err(cleanup_error)) => Err(cleanup_error),
        (Err(run_error), Err(cleanup_error)) => Err(format!("{run_error}; {cleanup_error}")),
    }
}

fn enqueue_proxy_request(
    inbox: &CommandInbox,
    request: Request,
    events: &crossbeam_channel::Sender<Event>,
) {
    let command = match request {
        Request::Configure {
            session,
            generation,
            settings,
        } => HostCommand::Configure {
            session,
            generation,
            settings,
        },
        Request::Rebuild {
            session,
            generation,
            request_id,
            guid,
        } => HostCommand::Rebuild {
            session,
            generation,
            request_id,
            guid,
        },
        Request::VolumeQuery {
            session,
            generation,
            query_id,
            text,
        } => HostCommand::Query {
            session,
            generation,
            query_id,
            text,
        },
        Request::ProxyInputClosed => return,
        request => {
            let _ = events.send(reject_proxy_request(request));
            return;
        }
    };
    match inbox.enqueue(command) {
        Ok(Some(replaced)) => {
            let _ = events.send(rejected_command(
                replaced,
                None,
                "새 검색어로 대체되었습니다.".into(),
            ));
        }
        Ok(None) => {}
        Err(rejected) => {
            let _ = events.send(rejected_command(
                rejected,
                None,
                "검색 제어 대기열이 가득 찼습니다. 요청을 다시 시도하세요.".into(),
            ));
        }
    }
}

fn reject_proxy_request(request: Request) -> Event {
    let message = "전체 드라이브 검색 세션에서 허용되지 않은 요청입니다.".to_string();
    match request {
        Request::Query { id, .. } => Event::Search {
            id,
            result: Err(message),
        },
        Request::Index { id, .. } | Request::IndexVolumes { id, .. } => Event::Failure {
            lane: 4,
            id,
            message,
        },
        Request::Configure {
            session,
            generation,
            ..
        } => Event::ConfigAck {
            session,
            generation,
            host_pid: None,
            accepted: false,
            message: Some(message),
        },
        Request::Rebuild {
            session,
            generation,
            request_id,
            guid,
        } => Event::RebuildAck {
            session,
            generation,
            request_id,
            guid,
            host_pid: None,
            accepted: false,
            message: Some(message),
        },
        Request::VolumeQuery {
            session,
            generation,
            query_id,
            ..
        } => Event::VolumeSearch {
            session,
            generation,
            query_id,
            host_pid: None,
            result: Err(message),
        },
        Request::List { id, .. }
        | Request::Tree { id, .. }
        | Request::Preview { id, .. }
        | Request::Watch { id, .. } => Event::Failure {
            lane: 4,
            id,
            message,
        },
        _ => Event::Notice { message },
    }
}

fn create_server_pipe(
    name: &str,
    attributes: &::windows::Win32::Security::SECURITY_ATTRIBUTES,
) -> Result<File, String> {
    let name = wide(name);
    unsafe {
        // Duplex access permits changing the server wait mode. Each instance
        // still carries only one direction, with no concurrent read/write.
        let handle = CreateNamedPipeW(
            PCWSTR(name.as_ptr()),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_NOWAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            65536,
            65536,
            0,
            Some(attributes),
        );
        if handle == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(File::from_raw_handle(handle.0))
    }
}

fn connect_server_pipe(
    pipe: &File,
    expected: u32,
    child: HANDLE,
    input_closed: &std::sync::atomic::AtomicBool,
) -> Result<(), String> {
    let handle = HANDLE(pipe.as_raw_handle());
    let start = Instant::now();
    unsafe {
        loop {
            if input_closed.load(std::sync::atomic::Ordering::Acquire) {
                return Err("검색 worker Stop이 호스트 연결을 중단했습니다.".into());
            }
            match ConnectNamedPipe(handle, None) {
                Ok(()) => break,
                Err(error) if error.code() == HRESULT::from_win32(ERROR_PIPE_CONNECTED.0) => break,
                Err(error)
                    if error.code() == HRESULT::from_win32(ERROR_PIPE_LISTENING.0)
                        || error.code() == HRESULT::from_win32(ERROR_NO_DATA.0) => {}
                Err(error) => return Err(error.to_string()),
            }
            if start.elapsed() > Duration::from_secs(20) {
                return Err("검색 프로세스 연결 시간이 초과됐습니다.".into());
            }
            match WaitForSingleObject(child, 0) {
                WAIT_OBJECT_0 => return Err("검색 프로세스가 연결 전에 종료됐습니다.".into()),
                WAIT_TIMEOUT => {}
                WAIT_FAILED => {
                    return Err(format!(
                        "검색 프로세스 연결 대기가 실패했습니다: {}",
                        std::io::Error::last_os_error()
                    ));
                }
                result => {
                    return Err(format!(
                        "검색 프로세스 연결 대기 결과가 올바르지 않습니다: {result:?}"
                    ));
                }
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        let mut actual = 0;
        GetNamedPipeClientProcessId(handle, &mut actual).map_err(|error| error.to_string())?;
        if actual != expected {
            return Err("예상하지 않은 검색 프로세스 연결을 거부했습니다.".into());
        }
        SetNamedPipeHandleState(handle, Some(&PIPE_WAIT), None, None)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

// The lower-privilege server must never acquire the elevated client's token.
fn connect_host_pipe(
    name: &str,
    parent: &ParentIdentity,
    write_only: bool,
) -> Result<File, String> {
    let pipe = std::fs::OpenOptions::new()
        .read(!write_only)
        .write(write_only)
        .security_qos_flags((SECURITY_SQOS_PRESENT | SECURITY_ANONYMOUS).0)
        .open(name)
        .map_err(|e| e.to_string())?;
    unsafe {
        let mut server = 0;
        GetNamedPipeServerProcessId(HANDLE(pipe.as_raw_handle()), &mut server)
            .map_err(|e| e.to_string())?;
        if server != parent.pid {
            return Err("검색 부모 프로세스가 일치하지 않습니다.".into());
        }
    }
    parent.validate()?;
    Ok(pipe)
}

fn read_first_command(
    reader: BufReader<File>,
    service_mode: bool,
) -> Result<
    (
        BufReader<File>,
        Result<Option<HostCommand>, HostCommandReadError>,
    ),
    String,
> {
    let (send_result, receive_result) = crossbeam_channel::bounded(1);
    let io_thread: SharedIoThread = std::sync::Arc::new(std::sync::Mutex::new(None));
    let reader_io_thread = io_thread.clone();
    std::thread::spawn(move || {
        let mut reader = reader;
        let result = match goz_winfs::current_thread_io_handle() {
            Ok(handle) => {
                *reader_io_thread.lock().unwrap() = Some(handle);
                if service_mode {
                    register_service_io_thread(reader_io_thread.clone());
                }
                if service_mode && service_stop_requested() {
                    Ok(None)
                } else {
                    super::read_query(&mut reader)
                }
            }
            Err(error) => Err(HostCommandReadError {
                command: None,
                message: format!("초기 명령 읽기 스레드를 준비하지 못했습니다: {error}"),
            }),
        };
        let _ = send_result.send((reader, result));
        reader_io_thread.lock().unwrap().take();
    });
    if service_mode {
        let configure_deadline = Instant::now() + Duration::from_secs(20);
        let mut cancellation_deadline = None;
        loop {
            let now = Instant::now();
            if service_stop_requested() || now >= configure_deadline {
                let deadline = cancellation_deadline.get_or_insert(now + HOST_STOP_TIMEOUT);
                if now >= *deadline {
                    return Err("서비스 초기 명령 읽기를 종료하지 못했습니다.".into());
                }
                cancel_service_io();
            }
            match receive_result.recv_timeout(Duration::from_millis(25)) {
                Ok(result) if cancellation_deadline.is_none() || service_stop_requested() => {
                    return Ok(result);
                }
                Ok(_) => return Err("초기 Configure 명령의 대기 시간이 초과됐습니다.".into()),
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    return Err("서비스 초기 명령 읽기 스레드가 응답하지 않았습니다.".into());
                }
            }
        }
    }
    match receive_result.recv_timeout(Duration::from_secs(20)) {
        Ok(result) => Ok(result),
        Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
            if let Some(handle) = io_thread.lock().unwrap().as_ref() {
                let _ = goz_winfs::cancel_synchronous_io(handle);
            }
            let _ = receive_result.recv_timeout(Duration::from_secs(2));
            Err("초기 Configure 명령이 20초 안에 도착하지 않았습니다.".into())
        }
        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
            Err("초기 Configure 읽기 스레드가 응답하지 않았습니다.".into())
        }
    }
}

pub(super) fn host(args: &[String]) -> Result<(), String> {
    if !goz_winfs::current_process_is_elevated_admin().map_err(|e| e.to_string())? {
        return Err("검색 전용 프로세스에 관리자 권한이 필요합니다.".into());
    }
    if args.len() != 12
        || args.get(1).map(String::as_str) != Some("--ntfs-host")
        || args.get(2).map(String::as_str) != Some("--pipe")
        || args.get(4).map(String::as_str) != Some("--parent")
        || args.get(6).map(String::as_str) != Some("--parent-created-filetime")
        || args.get(8).map(String::as_str) != Some("--index-id")
        || args.get(10).map(String::as_str) != Some("--memory-mib")
    {
        return Err("검색 프로세스 인수 형식이 올바르지 않습니다.".into());
    }
    let name = args[3].as_str();
    if !name.starts_with(r"\\.\pipe\SDIR-search-") || name.len() > 100 {
        return Err("잘못된 검색 연결".into());
    }
    let parent: u32 = args[5].parse().map_err(|_| "잘못된 부모")?;
    let parent_created_filetime: u64 = args[7].parse().map_err(|_| "잘못된 부모 생성 시각")?;
    let id: u64 = args[9].parse().map_err(|_| "잘못된 색인 ID")?;
    let memory_mib: u16 = args[11].parse().map_err(|_| "잘못된 메모리 상한")?;
    run_host(name, parent, parent_created_filetime, id, memory_mib, None)
}

pub(super) fn service_host(
    pipe_id: &str,
    parent_pid: u32,
    parent_created_filetime: u64,
    index_id: u64,
    memory_mib: u16,
) -> Result<(), String> {
    crate::live_search::service::validate_service_identity()?;
    if pipe_id.len() != 36
        || !pipe_id
            .bytes()
            .enumerate()
            .all(|(index, byte)| match index {
                8 | 13 | 18 | 23 => byte == b'-',
                _ => byte.is_ascii_hexdigit(),
            })
    {
        return Err("서비스 pipe GUID가 올바르지 않습니다.".into());
    }
    let name = format!(r"\\.\pipe\SDIR-index-v1-{pipe_id}");
    run_host(
        &name,
        parent_pid,
        parent_created_filetime,
        index_id,
        memory_mib,
        Some(crate::live_search::service::read_owner_sid()?),
    )
}

fn run_host(
    name: &str,
    parent_pid: u32,
    parent_created_filetime: u64,
    id: u64,
    memory_mib: u16,
    owner_sid: Option<String>,
) -> Result<(), String> {
    if parent_pid == 0 || parent_created_filetime == 0 || id == 0 {
        return Err("검색 요청 식별자가 올바르지 않습니다.".into());
    }
    if !(128..=2048).contains(&memory_mib) {
        return Err("잘못된 메모리 상한".into());
    }
    let parent_handle = crate::live_search::service::validate_parent_process(
        parent_pid,
        parent_created_filetime,
        owner_sid.as_deref(),
    )?;
    let parent = ParentIdentity {
        process: Process(parent_handle),
        pid: parent_pid,
        created_filetime: parent_created_filetime,
        owner_sid,
    };
    let service_mode = parent.owner_sid.is_some();
    let mut pipe = connect_host_pipe(name, &parent, true)?;
    let input = connect_host_pipe(&format!("{name}-queries"), &parent, false)?;
    let mut reader = BufReader::new(input);
    let host_pid = std::process::id();

    let (reader_after_first, first_result) = read_first_command(reader, service_mode)?;
    if service_mode && service_stop_requested() {
        return Ok(());
    }
    reader = reader_after_first;
    let first = match first_result {
        Ok(Some(command)) => command,
        Ok(None) => {
            send_authenticated(
                &mut pipe,
                &parent,
                &Event::ConfigAck {
                    session: id,
                    generation: 0,
                    host_pid: Some(host_pid),
                    accepted: false,
                    message: Some("초기 설정 없이 검색 호스트가 종료됐습니다.".into()),
                },
            )?;
            return Ok(());
        }
        Err(error) => {
            if let Some(command) = error.command {
                send_authenticated(
                    &mut pipe,
                    &parent,
                    &rejected_command(command, Some(host_pid), error.message),
                )?;
            } else {
                send_authenticated(
                    &mut pipe,
                    &parent,
                    &Event::Failure {
                        lane: 4,
                        id,
                        message: error.message.clone(),
                    },
                )?;
                send_authenticated(
                    &mut pipe,
                    &parent,
                    &Event::ConfigAck {
                        session: id,
                        generation: 0,
                        host_pid: Some(host_pid),
                        accepted: false,
                        message: Some(error.message),
                    },
                )?;
            }
            return Ok(());
        }
    };

    let (session, generation, mut settings) = match first {
        HostCommand::Configure {
            session,
            generation,
            settings,
        } if session == id && generation == 0 => (session, generation, *settings),
        HostCommand::Configure {
            session,
            generation,
            settings: _,
        } => {
            let message = "초기 설정의 검색 세션 또는 세대가 일치하지 않습니다.".to_string();
            send_authenticated(
                &mut pipe,
                &parent,
                &Event::ConfigAck {
                    session,
                    generation,
                    host_pid: Some(host_pid),
                    accepted: false,
                    message: Some(message.clone()),
                },
            )?;
            send_authenticated(
                &mut pipe,
                &parent,
                &Event::ConfigAck {
                    session: id,
                    generation: 0,
                    host_pid: Some(host_pid),
                    accepted: false,
                    message: Some(message),
                },
            )?;
            return Ok(());
        }
        command => {
            send_authenticated(
                &mut pipe,
                &parent,
                &rejected_command(
                    command,
                    Some(host_pid),
                    "검색 호스트의 첫 명령은 Configure여야 합니다.".into(),
                ),
            )?;
            send_authenticated(
                &mut pipe,
                &parent,
                &Event::ConfigAck {
                    session: id,
                    generation: 0,
                    host_pid: Some(host_pid),
                    accepted: false,
                    message: Some("초기 Configure 명령이 없습니다.".into()),
                },
            )?;
            return Ok(());
        }
    };
    if service_mode {
        settings = service_safe_settings(settings);
    }

    let (inbox, notices) = (
        std::sync::Arc::new(CommandInbox::default()),
        crossbeam_channel::unbounded::<HostNotice>(),
    );
    let stop = HostStop::new(service_mode);
    let input_inbox = inbox.clone();
    let input_stop = stop.clone();
    let (notice_tx, notice_rx) = notices;
    let reader_for_commands = reader;
    send_authenticated(
        &mut pipe,
        &parent,
        &Event::ConfigAck {
            session,
            generation,
            host_pid: Some(host_pid),
            accepted: true,
            message: None,
        },
    )?;
    let input_thread = std::thread::spawn(move || {
        supervise_host_input(
            reader_for_commands,
            input_inbox,
            input_stop,
            notice_tx,
            service_mode,
        );
    });

    let _background_io = BackgroundIo::enter();
    let result = run(
        session,
        generation,
        u64::from(memory_mib) * 1024 * 1024,
        settings,
        inbox,
        notice_rx,
        stop.clone(),
        service_mode,
        &parent,
        |event| send_authenticated(&mut pipe, &parent, event),
    );
    if service_mode {
        stop.requested
            .store(true, std::sync::atomic::Ordering::Release);
        let deadline = Instant::now() + HOST_STOP_TIMEOUT;
        while !input_thread.is_finished() {
            if Instant::now() >= deadline {
                return Err("서비스 명령 읽기 스레드를 제한 시간 안에 종료하지 못했습니다.".into());
            }
            cancel_service_io();
            std::thread::sleep(Duration::from_millis(10));
        }
        input_thread
            .join()
            .map_err(|_| "서비스 명령 읽기 스레드가 비정상 종료했습니다.")?;
    } else if stop.is_requested() {
        let _ = input_thread.join();
    }
    result
}

fn supervise_host_input<R: BufRead>(
    mut reader: R,
    input_inbox: std::sync::Arc<CommandInbox>,
    input_stop: std::sync::Arc<HostStop>,
    notice_tx: crossbeam_channel::Sender<HostNotice>,
    service_mode: bool,
) {
    let io_thread: SharedIoThread = std::sync::Arc::new(std::sync::Mutex::new(None));
    if service_mode {
        match goz_winfs::current_thread_io_handle() {
            Ok(handle) => {
                *io_thread.lock().unwrap() = Some(handle);
                register_service_io_thread(io_thread.clone());
            }
            Err(error) => {
                let _ = notice_tx.send(HostNotice::Protocol(format!(
                    "서비스 입력 스레드를 준비하지 못했습니다: {error}"
                )));
                input_inbox.close();
                input_stop
                    .requested
                    .store(true, std::sync::atomic::Ordering::Release);
                return;
            }
        }
    }
    while !service_mode || !input_stop.is_requested() {
        match read_query(&mut reader) {
            // Persistence is disabled. Replies must not enter the unconsumed reply queue.
            Ok(Some(command)) if command.is_cache_reply() => {}
            Ok(Some(command)) => match input_inbox.enqueue(command) {
                Ok(Some(replaced)) => {
                    let _ = notice_tx.send(HostNotice::Rejected(
                        replaced,
                        "새 검색어로 대체되었습니다.".into(),
                    ));
                }
                Ok(None) => {}
                Err(rejected) => {
                    let _ = notice_tx.send(HostNotice::Rejected(
                        rejected,
                        "검색 제어 대기열이 가득 찼습니다. 요청을 다시 시도하세요.".into(),
                    ));
                }
            },
            Ok(None) => break,
            Err(error) => {
                if let Some(command) = error.command {
                    let _ = notice_tx.send(HostNotice::Rejected(command, error.message));
                } else {
                    let _ = notice_tx.send(HostNotice::Protocol(error.message));
                }
                break;
            }
        }
    }
    input_inbox.close();
    input_stop.supervise_eof();
    io_thread.lock().unwrap().take();
}

enum HostNotice {
    Rejected(HostCommand, String),
    Protocol(String),
}

fn volume_guid_path(guid: &crate::index_policy::VolumeGuid) -> String {
    let slash = char::from(92);
    format!(
        "{}{}?{}Volume{{{}}}{}",
        slash,
        slash,
        slash,
        guid.as_str(),
        slash
    )
}

const FSCTL_SCRATCH_BYTES: usize = 1024 * 1024;
const MAX_PAGE_RECORDS: usize = 256;
const MAX_VOLUME_ENTRIES: usize = 5_000_000;
const MAX_ACTIVE_GUIDS: usize = crate::index_policy::MAX_VOLUME_POLICIES;
const MAX_QUERY_HITS_PER_VOLUME: usize = 200;
const MAX_QUERY_PATH_CANDIDATES: usize = MAX_ACTIVE_GUIDS * MAX_QUERY_HITS_PER_VOLUME;

fn query_candidate_capacity(volume_count: usize) -> Result<usize, String> {
    let candidates = volume_count
        .checked_mul(MAX_QUERY_HITS_PER_VOLUME)
        .ok_or_else(|| "검색 결과 작업공간이 너무 큽니다. 범위를 좁혀 주세요.".to_string())?;
    if volume_count > MAX_ACTIVE_GUIDS || candidates > MAX_QUERY_PATH_CANDIDATES {
        return Err("검색할 드라이브 또는 결과 후보가 너무 많습니다. 범위를 좁혀 주세요.".into());
    }
    Ok(candidates)
}

fn rebuild_snapshot_mode(decision: Option<crate::index_policy::VolumeDecision>) -> Option<bool> {
    match decision {
        Some(crate::index_policy::VolumeDecision::Included { monitor }) => Some(!monitor),
        Some(crate::index_policy::VolumeDecision::Excluded)
        | Some(crate::index_policy::VolumeDecision::Unsupported)
        | None => None,
    }
}

struct Volume {
    stop: std::sync::Arc<HostStop>,
    guid: crate::index_policy::VolumeGuid,
    serial: Option<u32>,
    classification: super::InventoryClass,
    mount: Option<NativePath>,
    root: NativePath,
    state: Phase,
    message: Option<String>,
    handle: Option<VolumeHandle>,
    index: Option<Arc<VolumeIndex>>,
    restored: bool,
    saved_revision: Option<u64>,
    cache_save_after: Instant,
    journal: Option<JournalInfo>,
    cursor: i64,
    high_water: Option<i64>,
    layout_started: bool,
    snapshot_only: bool,
    revision: u64,
    checked: Instant,
    retry_at: Instant,
}
impl Volume {
    fn new(info: super::InventoryVolume, stop: std::sync::Arc<HostStop>) -> Self {
        let mount = info.mounts.first().cloned();
        let root = mount
            .clone()
            .unwrap_or_else(|| NativePath::new(volume_guid_path(&info.guid)));
        Self {
            stop,
            guid: info.guid,
            serial: info.serial,
            classification: info.classification,
            mount,
            root,
            state: Phase::Unavailable,
            message: info.message,
            handle: None,
            index: None,
            restored: false,
            saved_revision: None,
            cache_save_after: Instant::now(),
            journal: None,
            cursor: 0,
            high_water: None,
            layout_started: false,
            snapshot_only: false,
            revision: 0,
            checked: Instant::now(),
            retry_at: Instant::now(),
        }
    }
    fn clear_index(&mut self) {
        self.handle = None;
        self.index = None;
        self.restored = false;
        self.saved_revision = None;
        self.cache_save_after = Instant::now();
        self.journal = None;
        self.cursor = 0;
        self.high_water = None;
        self.layout_started = false;
    }
    fn fail(&mut self, message: String) {
        self.clear_index();
        self.state = Phase::Error;
        self.message = Some(message);
        self.retry_at = Instant::now() + Duration::from_secs(30);
        self.revision += 1;
    }
    fn retry_ready(&self, now: Instant) -> bool {
        self.state == Phase::Offline || (self.state == Phase::Error && now >= self.retry_at)
    }
    fn volume_decision(
        &self,
        settings: &crate::index_policy::NtfsIndexingSettings,
    ) -> Option<crate::index_policy::VolumeDecision> {
        let class = match self.classification {
            super::InventoryClass::Fixed => crate::index_policy::NtfsVolumeClass::Fixed,
            super::InventoryClass::Removable => crate::index_policy::NtfsVolumeClass::Removable,
            super::InventoryClass::Unsupported => crate::index_policy::NtfsVolumeClass::Unsupported,
            super::InventoryClass::Unavailable => return None,
        };
        Some(settings.decision_for(&self.guid, class))
    }
    fn apply_policy(
        &mut self,
        settings: &crate::index_policy::NtfsIndexingSettings,
        force_restart: bool,
        inventory_message: Option<String>,
        now: Instant,
    ) -> bool {
        let before_revision = self.revision;
        let before_state = self.state.clone();
        let before_message = self.message.clone();
        let had_data = self.handle.is_some() || self.index.is_some() || self.journal.is_some();

        if self.classification == super::InventoryClass::Unavailable
            || self.serial.is_none()
            || self.mount.is_none()
        {
            self.clear_index();
            self.snapshot_only = false;
            self.state = Phase::Unavailable;
            self.message = inventory_message.or_else(|| {
                Some("현재 볼륨 정보를 신뢰할 수 없어 결과를 게시하지 않습니다.".into())
            });
        } else {
            match self.volume_decision(settings) {
                Some(crate::index_policy::VolumeDecision::Unsupported) => {
                    self.clear_index();
                    self.snapshot_only = false;
                    self.state = Phase::Unsupported;
                    self.message = Some("NTFS 고정·이동식 볼륨만 지원합니다. 선택 0개.".into());
                }
                Some(crate::index_policy::VolumeDecision::Excluded) => {
                    self.clear_index();
                    self.snapshot_only = false;
                    self.state = Phase::Excluded;
                    self.message = Some("검색 설정에서 제외된 볼륨입니다. 선택 0개.".into());
                }
                Some(crate::index_policy::VolumeDecision::Included { monitor: false }) => {
                    if force_restart
                        || self.retry_ready(now)
                        || !self.snapshot_only
                        || matches!(
                            self.state,
                            Phase::Excluded | Phase::Unsupported | Phase::Unavailable
                        )
                        || (self.state == Phase::Snapshot && self.index.is_none())
                    {
                        self.begin_mode(true);
                    }
                }
                Some(crate::index_policy::VolumeDecision::Included { monitor: true }) => {
                    if force_restart
                        || self.retry_ready(now)
                        || self.snapshot_only
                        || matches!(
                            self.state,
                            Phase::Excluded
                                | Phase::Snapshot
                                | Phase::Unsupported
                                | Phase::Unavailable
                        )
                    {
                        self.begin_mode(false);
                    }
                }
                None => {
                    self.clear_index();
                    self.snapshot_only = false;
                    self.state = Phase::Unavailable;
                    self.message = inventory_message.or_else(|| {
                        Some("현재 볼륨 정보를 신뢰할 수 없어 결과를 게시하지 않습니다.".into())
                    });
                }
            }
        }

        if self.revision == before_revision
            && (self.state != before_state
                || self.message != before_message
                || (had_data && self.handle.is_none() && self.index.is_none()))
        {
            self.revision += 1;
        }
        self.revision != before_revision
    }
    fn apply_inventory(
        &mut self,
        info: super::InventoryVolume,
        settings: &crate::index_policy::NtfsIndexingSettings,
        now: Instant,
    ) -> bool {
        let before_revision = self.revision;
        let old_serial = self.serial;
        let old_classification = self.classification;
        let old_root = self.root.clone();
        let force_restart = old_serial != info.serial || old_classification != info.classification;
        let mount = info.mounts.into_iter().next();
        self.guid = info.guid;
        self.serial = info.serial;
        self.classification = info.classification;
        self.root = mount
            .clone()
            .unwrap_or_else(|| NativePath::new(volume_guid_path(&self.guid)));
        self.mount = mount;
        self.apply_policy(settings, force_restart, info.message, now);
        if self.revision == before_revision && self.root != old_root {
            self.revision += 1;
        }
        self.revision != before_revision
    }
    fn mark_offline(&mut self) -> bool {
        let before_revision = self.revision;
        let before_state = self.state.clone();
        let before_message = self.message.clone();
        let had_data = self.handle.is_some() || self.index.is_some() || self.journal.is_some();
        self.clear_index();
        self.state = Phase::Offline;
        self.message = Some("드라이브가 연결되지 않았습니다.".into());
        if before_state != self.state || before_message != self.message || had_data {
            self.revision += 1;
        }
        self.revision != before_revision
    }
    fn mark_inventory_failure(&mut self, message: String) -> bool {
        let before_revision = self.revision;
        let before_state = self.state.clone();
        let before_message = self.message.clone();
        let had_data = self.handle.is_some() || self.index.is_some() || self.journal.is_some();
        self.clear_index();
        self.state = Phase::Unavailable;
        self.message = Some(message);
        if before_state != self.state || before_message != self.message || had_data {
            self.revision += 1;
        }
        self.revision != before_revision
    }
    #[cfg(test)]
    fn begin(&mut self) {
        self.begin_mode(false);
    }
    fn begin_mode(&mut self, snapshot_only: bool) {
        self.clear_index();
        self.snapshot_only = snapshot_only;
        self.revision += 1;
        self.state = Phase::Indexing;
        self.message = None;
        self.checked = Instant::now();
        let result = (|| {
            let guid_path = volume_guid_path(&self.guid);
            let h = host_winfs(self.stop.as_ref(), || goz_winfs::open_volume(&guid_path))?;
            confirm_volume_identity(
                self.stop.as_ref(),
                &h,
                &self.guid,
                self.serial,
                self.mount.as_ref(),
            )?;
            let j = match host_winfs(self.stop.as_ref(), || goz_winfs::query_usn_journal(&h))? {
                JournalQuery::Active(j) => {
                    if !cursor_is_valid(j.journal_id, j.next_usn, j) {
                        return Err("USN 저널의 시작 커서가 유효하지 않습니다.".into());
                    }
                    Some(j)
                }
                JournalQuery::NotActive if layout_only_snapshot_allowed(snapshot_only) => None,
                JournalQuery::NotActive => {
                    return Err(
                        "USN 저널이 비활성입니다. 모니터링을 켜지 않고 다시 시도합니다.".into(),
                    );
                }
                JournalQuery::DeleteInProgress => {
                    return Err("USN 저널을 변경 중입니다. 30초 뒤 다시 시도합니다.".into());
                }
            };
            self.cursor = j.map_or(0, |journal| journal.next_usn);
            self.journal = j;
            self.high_water = None;
            self.layout_started = false;
            self.index = Some(Arc::new(VolumeIndex::new(NTFS_ROOT_FRN, FrnMap::sparse())));
            self.handle = Some(h);
            Ok(())
        })();
        if let Err(error) = result {
            if !self.stop.is_requested() {
                self.fail(error);
            }
        }
    }
    fn allocated_bytes(&self) -> u64 {
        self.index
            .as_ref()
            .map_or(0, |index| index.memory().total().allocated)
    }
    fn entry_count(&self) -> usize {
        self.index
            .as_ref()
            .map_or(0, |index| index.len().saturating_sub(2))
    }
    fn limit(&mut self, message: String) {
        self.clear_index();
        self.state = Phase::Limited;
        self.message = Some(message);
        self.revision += 1;
    }
    fn status(&self) -> VolumeStatus {
        VolumeStatus {
            root: self.root.clone(),
            phase: self.state.clone(),
            entries: self.entry_count(),
            allocated_bytes: self.allocated_bytes(),
            message: self.message.clone(),
            guid: Some(self.guid.clone()),
            serial: self.serial,
            mount: self.mount.clone(),
        }
    }
    fn explicit_rebuild_snapshot_only(
        &self,
        settings: &crate::index_policy::NtfsIndexingSettings,
    ) -> Result<bool, String> {
        if self.serial.is_none() || self.mount.is_none() {
            return Err("현재 드라이브를 신뢰할 수 없어 다시 색인할 수 없습니다.".into());
        }
        rebuild_snapshot_mode(self.volume_decision(settings))
            .ok_or_else(|| "현재 드라이브는 검색에서 제외되어 있습니다.".into())
    }
    fn query_scope_conflict(
        text: &str,
        settings: &crate::index_policy::NtfsIndexingSettings,
        volumes: &[Volume],
    ) -> Result<(), String> {
        if text.trim().is_empty() {
            return Ok(());
        }
        let has_included_mounted_scope = volumes.iter().any(|volume| {
            volume.mount.is_some()
                && matches!(
                    volume.volume_decision(settings),
                    Some(crate::index_policy::VolumeDecision::Included { .. })
                )
                && settings.volume_policy(&volume.guid).is_some_and(|policy| {
                    !policy.scope().include_roots().is_empty()
                        || !policy.scope().exclude_roots().is_empty()
                })
        });
        if !has_included_mounted_scope {
            return Ok(());
        }
        let parsed = parse_query(text).map_err(|error| error.to_string())?;
        let mut graph_dependent = false;
        if let Some(expression) = parsed.expr.as_ref() {
            expression.for_each_leaf(&mut |leaf| {
                if matches!(
                    leaf,
                    Leaf::Related { .. } | Leaf::Count { .. } | Leaf::TotalChildSize(_)
                ) {
                    graph_dependent = true;
                }
            });
        }
        if graph_dependent {
            Err("범위 검색에서는 Related, Count, TotalChildSize 조건을 지원하지 않습니다.".into())
        } else {
            Ok(())
        }
    }
    fn query(
        &self,
        text: &str,
        settings: &crate::index_policy::NtfsIndexingSettings,
        windows_components: Option<&(crate::index_policy::VolumeGuid, RelativePathRoot)>,
        budget: &Budget<'_>,
    ) -> Result<crate::search::SearchResult, String> {
        self.stop.before_native()?;
        if !can_publish_query(&self.state, self.index.is_some()) {
            return Err("현재 커밋된 색인만 검색할 수 있습니다.".into());
        }
        let index = self
            .index
            .as_ref()
            .ok_or_else(|| "이 볼륨에는 신뢰할 수 있는 색인이 없습니다.".to_string())?;
        let mount = self
            .mount
            .as_ref()
            .ok_or_else(|| "이 볼륨에 현재 마운트가 없습니다.".to_string())?;
        let scope = settings
            .volume_policy(&self.guid)
            .map(|policy| policy.scope());
        let excluded = windows_components
            .filter(|(guid, _)| guid == &self.guid && !settings.show_windows_components())
            .map(|(_, root)| root);
        let mut result = query_index_with_exclusion(
            index,
            &self.root,
            text,
            self.state == Phase::Live,
            scope,
            excluded,
            budget,
        )?;
        self.stop.before_native()?;
        for hit in &mut result.hits {
            hit.volume_guid = Some(self.guid.clone());
            hit.volume_mount = Some(mount.clone());
        }
        Ok(result)
    }
    fn step(
        &mut self,
        scratch: &mut [u8],
        other_bytes: u64,
        index_budget: u64,
    ) -> Result<bool, String> {
        self.stop.before_native()?;
        if scratch.len() != FSCTL_SCRATCH_BYTES {
            return Err("공용 FSCTL scratch 크기가 올바르지 않습니다.".into());
        }
        if self.handle.is_none() || self.index.is_none() {
            return Ok(false);
        }
        if volume_resources_exceeded(
            self.index.as_ref().expect("index checked above"),
            other_bytes,
            index_budget,
        ) {
            self.limit("검색 색인 메모리 또는 500만 항목 상한에 도달했습니다.".into());
            return Ok(true);
        }
        if self.checked.elapsed() > Duration::from_secs(2) {
            self.checked = Instant::now();
            let current = {
                let h = self.handle.as_ref().expect("handle checked above");
                confirm_volume_identity(
                    self.stop.as_ref(),
                    h,
                    &self.guid,
                    self.serial,
                    self.mount.as_ref(),
                )?;
                host_winfs(self.stop.as_ref(), || goz_winfs::query_usn_journal(h))?
            };
            match (self.journal, current) {
                (Some(expected), JournalQuery::Active(actual)) => {
                    if !cursor_is_valid(expected.journal_id, self.cursor, actual) {
                        return Err(
                            "주기 재검증에서 USN 커서가 현재 저널 창과 일치하지 않습니다.".into(),
                        );
                    }
                    self.journal = Some(actual);
                }
                (Some(_), JournalQuery::NotActive) => {
                    return Err("색인 중 USN 저널이 비활성으로 바뀌었습니다.".into());
                }
                (Some(_), JournalQuery::DeleteInProgress) => {
                    return Err("색인 중 USN 저널을 변경 중입니다.".into());
                }
                (None, JournalQuery::NotActive)
                    if layout_only_snapshot_allowed(self.snapshot_only) => {}
                (None, JournalQuery::Active(_))
                    if layout_only_snapshot_allowed(self.snapshot_only) =>
                {
                    self.begin_mode(true);
                    return Ok(true);
                }
                (None, JournalQuery::DeleteInProgress) if self.snapshot_only => {
                    return Err("USN 저널을 변경 중입니다.".into());
                }
                _ => return Err("현재 볼륨에 유효한 USN 저널이 없습니다.".into()),
            }
        }
        if self.state == Phase::Indexing {
            let layout_result = {
                let h = self.handle.as_ref().expect("handle checked above");
                host_winfs(self.stop.as_ref(), || {
                    goz_winfs::query_file_layout(
                        h,
                        goz_core::layout::RECOMMENDED_LAYOUT_FLAGS,
                        !self.layout_started,
                        scratch,
                    )
                })?
            };
            match layout_result {
                Some(n) => {
                    if n > scratch.len() {
                        return Err("MFT 페이지 크기가 공용 FSCTL scratch를 넘었습니다.".into());
                    }
                    let files = goz_core::layout::walk_layout_buffer(&scratch[..n])
                        .map_err(|e| format!("MFT 기록 해석 실패: {e:?}"))?;
                    for batch in bounded_batches(&files) {
                        self.stop.before_native()?;
                        let exceeded = {
                            let index = self.index.as_mut().and_then(Arc::get_mut).expect("scheduled index is unshared");
                            for file in batch {
                                index.insert_layout(file);
                            }
                            volume_resources_exceeded(index, other_bytes, index_budget)
                        };
                        if exceeded {
                            self.limit(
                                "MFT 처리 중 검색 색인 메모리 또는 500만 항목 상한에 도달했습니다."
                                    .into(),
                            );
                            return Ok(true);
                        }
                    }
                    self.layout_started = true;
                    self.revision += 1;
                }
                None if self.journal.is_none()
                    && layout_only_snapshot_allowed(self.snapshot_only) =>
                {
                    let current = {
                        let h = self.handle.as_ref().expect("handle checked above");
                        confirm_volume_identity(
                            self.stop.as_ref(),
                            h,
                            &self.guid,
                            self.serial,
                            self.mount.as_ref(),
                        )?;
                        host_winfs(self.stop.as_ref(), || goz_winfs::query_usn_journal(h))?
                    };
                    match current {
                        JournalQuery::Active(_) => {
                            self.begin_mode(true);
                            return Ok(true);
                        }
                        JournalQuery::DeleteInProgress => {
                            return Err("USN 저널을 변경 중입니다.".into());
                        }
                        JournalQuery::NotActive => {}
                    }
                    let exceeded = {
                        let index = self.index.as_mut().and_then(Arc::get_mut).expect("scheduled index is unshared");
                        let allocated = index.memory().total().allocated;
                        if has_optimize_headroom(allocated, other_bytes, index_budget) {
                            index.optimize_storage();
                        }
                        volume_resources_exceeded(index, other_bytes, index_budget)
                    };
                    if exceeded {
                        self.limit("스냅샷 최적화 중 검색 색인 메모리 또는 500만 항목 상한에 도달했습니다.".into());
                        return Ok(true);
                    }
                    self.state = Phase::Snapshot;
                    self.handle = None;
                    self.revision += 1;
                }
                None => {
                    let expected = self
                        .journal
                        .ok_or_else(|| "MFT EOF에서 USN 저널 커서가 없습니다.".to_string())?;
                    let eof_journal = {
                        let h = self.handle.as_ref().expect("handle checked above");
                        match host_winfs(self.stop.as_ref(), || goz_winfs::query_usn_journal(h))? {
                            JournalQuery::Active(journal) => journal,
                            JournalQuery::NotActive => {
                                return Err("USN 저널이 비활성입니다.".into());
                            }
                            JournalQuery::DeleteInProgress => {
                                return Err("USN 저널을 변경 중입니다.".into());
                            }
                        }
                    };
                    if !cursor_is_valid(expected.journal_id, self.cursor, eof_journal) {
                        return Err(
                            "MFT 완료 시 USN 커서가 현재 저널 창과 일치하지 않습니다.".into()
                        );
                    }
                    {
                        let h = self.handle.as_ref().expect("handle checked above");
                        confirm_volume_identity(
                            self.stop.as_ref(),
                            h,
                            &self.guid,
                            self.serial,
                            self.mount.as_ref(),
                        )?;
                    }
                    self.journal = Some(eof_journal);
                    self.high_water = Some(eof_journal.next_usn);
                    let exceeded = {
                        let index = self.index.as_mut().and_then(Arc::get_mut).expect("scheduled index is unshared");
                        let allocated = index.memory().total().allocated;
                        if has_optimize_headroom(allocated, other_bytes, index_budget) {
                            index.optimize_storage();
                        }
                        volume_resources_exceeded(index, other_bytes, index_budget)
                    };
                    if exceeded {
                        self.limit(
                            "MFT 완료 시 검색 색인 메모리 또는 500만 항목 상한에 도달했습니다."
                                .into(),
                        );
                        return Ok(true);
                    }
                    self.state = Phase::CatchingUp;
                    if cursor_reached_high_water(self.cursor, eof_journal.next_usn) {
                        self.publish_caught_up()?;
                    } else {
                        self.revision += 1;
                    }
                }
            }
            return Ok(true);
        }
        if !matches!(self.state, Phase::Live | Phase::CatchingUp) {
            return Ok(false);
        }
        if self.state == Phase::CatchingUp
            && self
                .high_water
                .is_some_and(|high_water| cursor_reached_high_water(self.cursor, high_water))
        {
            return self.publish_caught_up();
        }
        let journal_id = self
            .journal
            .ok_or_else(|| "USN journal cursor is missing while reading.".to_string())?
            .journal_id;
        let n = {
            let h = self.handle.as_ref().expect("active journal handle missing");
            self.stop.before_native()?;
            let result =
                goz_winfs::read_usn_journal(h, self.cursor, journal_id, u32::MAX, 0, scratch);
            if self.stop.is_requested() {
                self.stop.after_winfs(result)?;
                return Err(HOST_STOP_MESSAGE.into());
            }
            match result {
                Ok(n) => n,
                Err(error) if error.code == goz_winfs::ERROR_JOURNAL_ENTRY_DELETED => {
                    let snapshot_only = self.snapshot_only;
                    self.begin_mode(snapshot_only);
                    return Ok(true);
                }
                Err(error) => return Err(error.to_string()),
            }
        };
        if n > scratch.len() {
            return Err("USN 페이지 크기가 공용 FSCTL scratch를 넘었습니다.".into());
        }
        let page =
            walk_journal_buffer(&scratch[..n]).map_err(|e| format!("USN 기록 해석 실패: {e:?}"))?;
        let page_window = {
            let h = self.handle.as_ref().expect("active journal handle missing");
            match host_winfs(self.stop.as_ref(), || goz_winfs::query_usn_journal(h))? {
                JournalQuery::Active(journal) => journal,
                JournalQuery::NotActive => return Err("USN 저널이 비활성입니다.".into()),
                JournalQuery::DeleteInProgress => return Err("USN 저널을 변경 중입니다.".into()),
            }
        };
        let next_cursor = validate_journal_page(journal_id, self.cursor, page_window, &page)
            .map_err(str::to_string)?;
        if page.records.is_empty() && page.next_usn == self.cursor {
            if is_live_idle_page(&self.state, self.cursor, &page) {
                self.journal = Some(page_window);
                return Ok(false);
            }
            return Err("Catch-up 중 USN 페이지가 커서 전진 없이 비어 있습니다.".into());
        }
        for batch in bounded_batches(&page.records) {
            self.stop.before_native()?;
            let exceeded = {
                let index = self.index.as_mut().and_then(Arc::get_mut).expect("scheduled index is unshared");
                if volume_resources_exceeded(index, other_bytes, index_budget) {
                    true
                } else {
                    let outcome = index.apply_batch(batch);
                    if volume_resources_exceeded(index, other_bytes, index_budget) {
                        true
                    } else {
                        let handle = self.handle.as_ref().expect("active journal handle missing");
                        reconcile_link_batch(
                            self.stop.as_ref(),
                            handle,
                            &self.guid,
                            self.serial,
                            index,
                            &outcome.needs_link_reconcile,
                        )?;
                        volume_resources_exceeded(index, other_bytes, index_budget)
                    }
                }
            };
            if exceeded {
                self.limit(
                    "USN 페이지 처리 중 검색 색인 메모리 또는 500만 항목 상한에 도달했습니다."
                        .into(),
                );
                return Ok(true);
            }
        }
        let commit_window = {
            let h = self.handle.as_ref().expect("active journal handle missing");
            match host_winfs(self.stop.as_ref(), || goz_winfs::query_usn_journal(h))? {
                JournalQuery::Active(journal) => journal,
                JournalQuery::NotActive => {
                    return Err("페이지 적용 중 USN 저널이 비활성입니다.".into());
                }
                JournalQuery::DeleteInProgress => {
                    return Err("페이지 적용 중 USN 저널을 변경 중입니다.".into());
                }
            }
        };
        if !cursor_is_valid(journal_id, self.cursor, commit_window)
            || !cursor_is_valid(journal_id, next_cursor, commit_window)
        {
            return Err("페이지 적용 뒤 USN 경계가 현재 저널 창에서 벗어났습니다.".into());
        }
        {
            let h = self.handle.as_ref().expect("active journal handle missing");
            confirm_volume_identity(
                self.stop.as_ref(),
                h,
                &self.guid,
                self.serial,
                self.mount.as_ref(),
            )?;
        }
        self.stop.before_native()?;
        self.cursor = next_cursor;
        self.journal = Some(commit_window);
        self.revision += 1;
        if self.state == Phase::CatchingUp {
            let high_water = self
                .high_water
                .ok_or_else(|| "MFT EOF catch-up target is missing.".to_string())?;
            if cursor_reached_high_water(self.cursor, high_water) {
                self.publish_caught_up()?;
            }
        }
        Ok(true)
    }
    fn publish_caught_up(&mut self) -> Result<bool, String> {
        self.stop.before_native()?;
        let high_water = self
            .high_water
            .ok_or_else(|| "MFT EOF catch-up target is missing.".to_string())?;
        if !cursor_reached_high_water(self.cursor, high_water) {
            return Ok(false);
        }
        let expected_id = self
            .journal
            .ok_or_else(|| "USN journal cursor is missing before publication.".to_string())?
            .journal_id;
        let window = {
            let h = self
                .handle
                .as_ref()
                .ok_or_else(|| "USN journal handle is missing before publication.".to_string())?;
            match host_winfs(self.stop.as_ref(), || goz_winfs::query_usn_journal(h))? {
                JournalQuery::Active(journal) => journal,
                JournalQuery::NotActive => return Err("공개 직전 USN 저널이 비활성입니다.".into()),
                JournalQuery::DeleteInProgress => {
                    return Err("공개 직전 USN 저널을 변경 중입니다.".into());
                }
            }
        };
        if !cursor_is_valid(expected_id, self.cursor, window) {
            return Err(
                "Live/Snapshot 공개 직전 USN 커서가 현재 저널 창과 일치하지 않습니다.".into(),
            );
        }
        {
            let h = self.handle.as_ref().expect("handle checked above");
            confirm_volume_identity(
                self.stop.as_ref(),
                h,
                &self.guid,
                self.serial,
                self.mount.as_ref(),
            )?;
        }
        self.journal = Some(window);
        self.state = completed_phase(self.snapshot_only);
        if self.restored {
            self.message = Some("저장된 색인 복원 · NTFS 변경분 반영 완료".into());
        }
        if self.snapshot_only {
            self.handle = None;
        }
        self.revision += 1;
        Ok(true)
    }
}

fn layout_only_snapshot_allowed(snapshot_only: bool) -> bool {
    snapshot_only
}

fn completed_phase(snapshot_only: bool) -> Phase {
    if snapshot_only {
        Phase::Snapshot
    } else {
        Phase::Live
    }
}

fn bounded_batches<'a, T>(records: &'a [T]) -> impl Iterator<Item = &'a [T]> {
    records.chunks(MAX_PAGE_RECORDS)
}

fn resource_limits_exceeded(
    entries: usize,
    allocated_bytes: u64,
    other_bytes: u64,
    index_budget: u64,
) -> bool {
    entries > MAX_VOLUME_ENTRIES || other_bytes.saturating_add(allocated_bytes) >= index_budget
}

fn volume_resources_exceeded(index: &VolumeIndex, other_bytes: u64, index_budget: u64) -> bool {
    resource_limits_exceeded(
        index.len(),
        index.memory().total().allocated,
        other_bytes,
        index_budget,
    )
}

fn has_optimize_headroom(allocated_bytes: u64, other_bytes: u64, index_budget: u64) -> bool {
    let used = other_bytes.saturating_add(allocated_bytes);
    let headroom = index_budget.saturating_sub(used);
    headroom > 0 && headroom >= allocated_bytes
}

fn result_mount_matches_for_host(
    stop: &HostStop,
    guid: &crate::index_policy::VolumeGuid,
    mount: &NativePath,
) -> Result<bool, String> {
    if !valid_mount(&mount.0) {
        return Ok(false);
    }
    let mut mount_path = mount.0.clone();
    mount_path.push(0);
    let mut volume_name = vec![0u16; MAX_VOLUME_NAME_UNITS];
    host_windows(stop, || unsafe {
        GetVolumeNameForVolumeMountPointW(PCWSTR(mount_path.as_ptr()), &mut volume_name)
    })?;
    let Ok(name) = nul_terminated(&volume_name) else {
        return Ok(false);
    };
    let Ok(name) = String::from_utf16(name) else {
        return Ok(false);
    };
    Ok(crate::index_policy::VolumeGuid::parse(&name).is_ok_and(|actual| &actual == guid))
}

// This is a session-only device identity. The volume GUID and filesystem serial
// are rechecked against the current mount before the raw handle is used.
fn storage_device_number(
    stop: &HostStop,
    handle: &VolumeHandle,
) -> Result<(u32, u32, u32), String> {
    let mut number = STORAGE_DEVICE_NUMBER::default();
    let size = std::mem::size_of::<STORAGE_DEVICE_NUMBER>() as u32;
    let mut written = 0;
    host_windows(stop, || unsafe {
        DeviceIoControl(
            HANDLE(handle.raw() as _),
            IOCTL_STORAGE_GET_DEVICE_NUMBER,
            None,
            0,
            Some((&mut number as *mut STORAGE_DEVICE_NUMBER).cast()),
            size,
            Some(&mut written),
            None,
        )
    })
    .map_err(|error| format!("볼륨 장치 정보 조회 실패: {error}"))?;
    if written != size {
        return Err("볼륨 장치 정보 길이가 올바르지 않습니다.".into());
    }
    Ok((
        number.DeviceType,
        number.DeviceNumber,
        number.PartitionNumber,
    ))
}

fn confirm_volume_identity(
    stop: &HostStop,
    handle: &VolumeHandle,
    guid: &crate::index_policy::VolumeGuid,
    serial: Option<u32>,
    mount: Option<&NativePath>,
) -> Result<(), String> {
    let serial = serial.ok_or_else(|| "볼륨 serial을 확인할 수 없습니다.".to_string())?;
    let mount = mount.ok_or_else(|| "볼륨 GUID에 연결된 마운트가 없습니다.".to_string())?;
    if !result_mount_matches_for_host(stop, guid, mount)? {
        return Err("현재 마운트가 확인된 볼륨 GUID와 일치하지 않습니다.".into());
    }

    // File information APIs do not support the raw volume device handle.
    // Bind it to a fresh handle for the same GUID and read metadata from the root.
    let guid_path = volume_guid_path(guid);
    let reference = host_winfs(stop, || goz_winfs::open_volume(&guid_path))?;
    if storage_device_number(stop, handle)? != storage_device_number(stop, &reference)? {
        return Err("열린 장치가 현재 볼륨 GUID와 일치하지 않습니다.".into());
    }

    let volume_root = wide(&guid_path);
    let mut actual_serial = 0u32;
    let mut filesystem = [0u16; 16];
    host_windows(stop, || unsafe {
        GetVolumeInformationW(
            PCWSTR(volume_root.as_ptr()),
            None,
            Some(&mut actual_serial),
            None,
            None,
            Some(&mut filesystem),
        )
    })
    .map_err(|error| format!("볼륨 루트 정보 조회 실패: {error}"))?;
    let filesystem = String::from_utf16_lossy(nul_terminated(&filesystem)?);
    if actual_serial != serial || !filesystem.eq_ignore_ascii_case("NTFS") {
        return Err("열린 볼륨의 serial 또는 파일시스템이 현재 인벤토리와 다릅니다.".into());
    }
    if !result_mount_matches_for_host(stop, guid, mount)? {
        return Err("볼륨 확인 중 드라이브 연결이 바뀌었습니다.".into());
    }
    Ok(())
}

fn split_raw_link_path(raw: &[u16]) -> Result<(usize, &[u16]), String> {
    let separator = u16::from(b'\\');
    if raw.len() < 2
        || raw[0] != separator
        || raw[1] == separator
        || raw
            .iter()
            .any(|unit| *unit == 0 || *unit == u16::from(b'/'))
    {
        return Err("하드 링크 경로가 볼륨 상대 raw 경로가 아닙니다.".into());
    }
    let mut start = 1;
    for position in 1..=raw.len() {
        if position == raw.len() || raw[position] == separator {
            let component = &raw[start..position];
            if component.is_empty()
                || (component.len() == 1 && component[0] == u16::from(b'.'))
                || (component.len() == 2
                    && component[0] == u16::from(b'.')
                    && component[1] == u16::from(b'.'))
            {
                return Err("하드 링크 경로에 비정상 구성 요소가 있습니다.".into());
            }
            start = position + 1;
        }
    }
    let last_separator = raw
        .iter()
        .rposition(|unit| *unit == separator)
        .ok_or_else(|| "하드 링크 경로에 부모 구분자가 없습니다.".to_string())?;
    let name = raw
        .get(last_separator + 1..)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| "하드 링크 경로의 파일 이름이 비어 있습니다.".to_string())?;
    Ok((last_separator, name))
}

fn open_verified_parent(
    stop: &HostStop,
    guid: &crate::index_policy::VolumeGuid,
    serial: u32,
    raw_parent_path: &[u16],
    expected_frn: goz_core::types::Frn,
) -> Result<File, String> {
    let root = volume_guid_path(guid);
    let mut path: Vec<u16> = root.trim_end_matches('\\').encode_utf16().collect();
    path.extend_from_slice(raw_parent_path);
    path.push(0);
    let file = host_windows(stop, || unsafe {
        CreateFileW(
            PCWSTR(path.as_ptr()),
            FILE_READ_ATTRIBUTES.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            None,
        )
        .map(|opened| File::from_raw_handle(opened.0))
    })?;

    let mut information: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    host_windows(stop, || unsafe {
        GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut information)
    })?;
    if information.dwVolumeSerialNumber != serial {
        return Err("하드 링크 부모 serial이 확인된 볼륨과 다릅니다.".into());
    }
    if information.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 == 0 {
        return Err("하드 링크 부모가 디렉터리가 아닙니다.".into());
    }
    let actual_frn =
        (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow);
    if actual_frn != expected_frn.0 {
        return Err("하드 링크 raw 부모 경로의 전체 FRN이 색인과 다릅니다.".into());
    }
    Ok(file)
}

fn reconcile_link_batch(
    stop: &HostStop,
    handle: &VolumeHandle,
    guid: &crate::index_policy::VolumeGuid,
    serial: Option<u32>,
    index: &mut VolumeIndex,
    pending: &[goz_core::types::Frn],
) -> Result<(), String> {
    let serial =
        serial.ok_or_else(|| "하드 링크 보정에 필요한 볼륨 serial이 없습니다.".to_string())?;
    for &frn in pending {
        stop.before_native()?;
        let links = match host_winfs(stop, || goz_winfs::link_paths(handle, frn.0))? {
            Some(links) => links,
            None if index.head_of(frn).is_some() => {
                return Err("색인에 남은 파일의 하드 링크 경로를 확인할 수 없습니다.".into());
            }
            None => continue,
        };
        if links.is_empty() {
            return Err(
                "하드 링크 조회가 빈 집합을 반환해 전체 그래프를 확인할 수 없습니다.".into(),
            );
        }
        let mut targets = Vec::with_capacity(links.len());
        let mut verified_parents = Vec::with_capacity(links.len());
        let mut seen = BTreeSet::new();
        for raw_path in links {
            if !seen.insert(raw_path.clone()) {
                return Err("하드 링크 조회에서 중복 raw 경로를 확인했습니다.".into());
            }
            let (separator, name_units) = split_raw_link_path(&raw_path)?;
            let parent_relative = if separator == 0 {
                &raw_path[0..0]
            } else {
                &raw_path[1..separator]
            };
            let mut parent_bytes = Vec::new();
            goz_core::wtf8::from_utf16(parent_relative, &mut parent_bytes);
            let parent = goz_core::query::resolve_scope(index, &parent_bytes, true)
                .ok_or_else(|| "하드 링크 raw 부모 경로가 색인 범위에 없습니다.".to_string())?;
            let parent_entry = index.entry(parent);
            if !parent_entry.is_dir() {
                return Err("하드 링크 색인 부모가 디렉터리가 아닙니다.".into());
            }
            let expected_frn = parent_entry.frn();
            let verified =
                open_verified_parent(stop, guid, serial, &raw_path[..separator], expected_frn)?;
            let mut name_bytes = Vec::new();
            let lossy = goz_core::wtf8::from_utf16(name_units, &mut name_bytes);
            targets.push(LinkTarget {
                parent,
                name: WtfName::new(name_bytes, lossy),
            });
            verified_parents.push(verified);
        }
        if targets.len() != verified_parents.len() {
            return Err("하드 링크 전체 부모 확인이 완료되지 않았습니다.".into());
        }
        stop.before_native()?;
        index.reconcile_links(frn, &targets);
        drop(verified_parents);
    }
    Ok(())
}

fn cursor_is_valid(expected_journal_id: u64, cursor: i64, actual: JournalInfo) -> bool {
    let saved = goz_core::usn::SavedCursor {
        journal_id: expected_journal_id,
        next_usn: cursor,
    };
    let live = goz_core::usn::JournalInfo {
        journal_id: actual.journal_id,
        first_usn: actual.first_usn,
        next_usn: actual.next_usn,
        lowest_valid_usn: actual.lowest_valid_usn,
        max_usn: actual.max_usn,
    };
    matches!(
        goz_core::usn::validate_cursor(&saved, &live),
        goz_core::usn::Resync::InSync
    )
}

fn validate_journal_page(
    journal_id: u64,
    cursor: i64,
    journal: JournalInfo,
    page: &goz_core::usn::JournalWalk,
) -> Result<i64, &'static str> {
    if page.skipped.v4 > 0 || page.skipped.unknown_version > 0 {
        return Err("unsupported USN record version");
    }
    if page.next_usn < cursor {
        return Err("USN page cursor regressed");
    }
    if !cursor_is_valid(journal_id, cursor, journal)
        || !cursor_is_valid(journal_id, page.next_usn, journal)
    {
        return Err("USN page cursor is outside the current journal window");
    }
    if page
        .records
        .iter()
        .any(|record| record.usn < cursor || record.usn >= page.next_usn)
    {
        return Err("USN page record is outside its cursor interval");
    }
    Ok(page.next_usn)
}

fn cursor_reached_high_water(cursor: i64, high_water: i64) -> bool {
    cursor >= high_water
}

fn is_live_idle_page(state: &Phase, cursor: i64, page: &goz_core::usn::JournalWalk) -> bool {
    matches!(state, Phase::Live) && page.records.is_empty() && page.next_usn == cursor
}

fn can_publish_query(state: &Phase, has_index: bool) -> bool {
    has_index && matches!(state, Phase::Live | Phase::Snapshot)
}

fn bound_active_volumes(
    mut volumes: Vec<super::InventoryVolume>,
) -> (Vec<super::InventoryVolume>, usize) {
    volumes.sort_by(|left, right| left.guid.cmp(&right.guid));
    let overflow = volumes.len().saturating_sub(MAX_ACTIVE_GUIDS);
    volumes.truncate(MAX_ACTIVE_GUIDS);
    (volumes, overflow)
}

fn next_fair_volume(states: &[Phase], start: usize) -> Option<usize> {
    if states.is_empty() {
        return None;
    }
    let start = start % states.len();
    (0..states.len())
        .map(|offset| (start + offset) % states.len())
        .find(|&index| {
            matches!(
                states[index],
                Phase::Indexing | Phase::CatchingUp | Phase::Live
            )
        })
}

fn service_safe_settings(
    mut settings: crate::index_policy::NtfsIndexingSettings,
) -> crate::index_policy::NtfsIndexingSettings {
    let _ = settings.set_auto_removable(false);
    settings
}

fn run(
    session: u64,
    mut generation: u64,
    memory_limit: u64,
    mut settings: crate::index_policy::NtfsIndexingSettings,
    inbox: std::sync::Arc<CommandInbox>,
    notices: crossbeam_channel::Receiver<HostNotice>,
    stop: std::sync::Arc<HostStop>,
    service_mode: bool,
    parent: &ParentIdentity,
    mut output: impl FnMut(&Event) -> Result<(), String>,
) -> Result<(), String> {
    stop.attach_current_thread()?;
    if service_mode {
        settings = service_safe_settings(settings);
    }
    let mut returned = HostRunReturned {
        stop: stop.clone(),
        normal: false,
    };
    let windows_components = windows_components_scope();
    let mut volumes: Vec<Volume> = vec![];
    let mut scratch = vec![0; FSCTL_SCRATCH_BYTES];
    let index_budget = memory_limit.saturating_sub(FSCTL_SCRATCH_BYTES as u64);
    let mut overflow_count = 0usize;
    let mut next_volume = 0usize;
    let mut network_drives = Vec::new();
    let mut inventory_error: Option<String> = None;
    let mut discovery = Instant::now() - Duration::from_secs(10);
    let mut reported = Instant::now();
    let mut query: Option<(u64, u64, u64, String)> = None;
    let mut dirty = true;
    let mut queried_revision = 0;
    let mut query_clock = Instant::now() - Duration::from_secs(1);
    let mut rebuild_in_progress: Option<crate::index_policy::VolumeGuid> = None;
    let mut last_parent_check = Instant::now() - Duration::from_secs(2);
    let mut resume_store: Option<ResumeStore> = None;
    loop {
        if last_parent_check.elapsed() >= Duration::from_secs(1) {
            parent.validate()?;
            last_parent_check = Instant::now();
        }
        if stop.is_requested() {
            returned.normal = true;
            return Ok(());
        }
        for notice in notices.try_iter().take(8) {
            match notice {
                HostNotice::Rejected(command, message) => {
                    output(&rejected_command(
                        command,
                        Some(std::process::id()),
                        message,
                    ))?;
                }
                HostNotice::Protocol(message) => output(&Event::Failure {
                    lane: 4,
                    id: session,
                    message,
                })?,
            }
        }
        for command in inbox.pop_batch(4) {
            match command {
                HostCommand::Configure {
                    session: command_session,
                    generation: next_generation,
                    settings: next_settings,
                } => {
                    if command_session != session || next_generation <= generation {
                        output(&Event::ConfigAck {
                            session: command_session,
                            generation: next_generation,
                            host_pid: Some(std::process::id()),
                            accepted: false,
                            message: Some("이전 세대의 설정 명령을 거부했습니다.".into()),
                        })?;
                    } else if !next_settings.is_valid() {
                        output(&Event::ConfigAck {
                            session: command_session,
                            generation: next_generation,
                            host_pid: Some(std::process::id()),
                            accepted: false,
                            message: Some("검색 설정이 올바르지 않습니다.".into()),
                        })?;
                    } else {
                        let next_settings = *next_settings;
                        let next_settings = if service_mode {
                            service_safe_settings(next_settings)
                        } else {
                            next_settings
                        };
                        let now = Instant::now();
                        for volume in &mut volumes {
                            let old_scope = settings
                                .volume_policy(&volume.guid)
                                .map(|policy| policy.scope())
                                .filter(|scope| {
                                    !scope.include_roots().is_empty()
                                        || !scope.exclude_roots().is_empty()
                                });
                            let new_scope = next_settings
                                .volume_policy(&volume.guid)
                                .map(|policy| policy.scope())
                                .filter(|scope| {
                                    !scope.include_roots().is_empty()
                                        || !scope.exclude_roots().is_empty()
                                });
                            let restart_unfinished = old_scope != new_scope
                                && matches!(volume.state, Phase::Indexing | Phase::CatchingUp);
                            if restart_unfinished
                                || volume.volume_decision(&settings)
                                    != volume.volume_decision(&next_settings)
                            {
                                dirty |= volume.apply_policy(
                                    &next_settings,
                                    true,
                                    volume.message.clone(),
                                    now,
                                );
                            }
                        }
                        settings = next_settings;
                        generation = next_generation;
                        if let Some((query_session, query_generation, query_id, _)) = query.take() {
                            output(&Event::VolumeSearch {
                                session: query_session,
                                generation: query_generation,
                                query_id,
                                host_pid: Some(std::process::id()),
                                result: Err(
                                    "검색 설정 세대가 바뀌어 이전 검색이 취소됐습니다.".into()
                                ),
                            })?;
                        }
                        output(&Event::ConfigAck {
                            session: command_session,
                            generation,
                            host_pid: Some(std::process::id()),
                            accepted: true,
                            message: None,
                        })?;
                        dirty = true;
                    }
                }
                HostCommand::Rebuild {
                    session: command_session,
                    generation: command_generation,
                    request_id,
                    guid,
                } => {
                    let message = if !super::same_generation(
                        session,
                        generation,
                        command_session,
                        command_generation,
                    ) {
                        Some("이전 세대의 재구성 명령을 거부했습니다.".to_string())
                    } else if rebuild_in_progress.is_some() {
                        Some("다른 드라이브 재구성이 진행 중입니다.".to_string())
                    } else if let Some(volume) = volumes
                        .iter_mut()
                        .find(|volume| volume.guid.as_str() == guid.as_str())
                    {
                        if matches!(volume.state, Phase::Indexing | Phase::CatchingUp) {
                            Some("이미 색인 중인 드라이브입니다.".into())
                        } else {
                            match volume.explicit_rebuild_snapshot_only(&settings) {
                                Err(error) => Some(error),
                                Ok(snapshot_only) => {
                                    if let Some(resume) = &mut resume_store {
                                        resume.skip_restore(&guid);
                                    }
                                    volume.begin_mode(snapshot_only);
                                    if volume.state == Phase::Error {
                                        Some(volume.message.clone().unwrap_or_else(|| {
                                            "드라이브 색인을 다시 시작하지 못했습니다.".into()
                                        }))
                                    } else {
                                        rebuild_in_progress = Some(guid.clone());
                                        dirty = true;
                                        None
                                    }
                                }
                            }
                        }
                    } else {
                        Some("요청한 드라이브를 찾을 수 없습니다.".into())
                    };
                    if message.is_none() {
                        if let Some((query_session, query_generation, query_id, _)) = query.take() {
                            output(&Event::VolumeSearch {
                                session: query_session,
                                generation: query_generation,
                                query_id,
                                host_pid: Some(std::process::id()),
                                result: Err(
                                    "드라이브를 다시 색인해 이전 검색이 취소됐습니다.".into()
                                ),
                            })?;
                        }
                    }
                    output(&Event::RebuildAck {
                        session: command_session,
                        generation: command_generation,
                        request_id,
                        guid,
                        host_pid: Some(std::process::id()),
                        accepted: message.is_none(),
                        message,
                    })?;
                }
                HostCommand::Query {
                    session: query_session,
                    generation: query_generation,
                    query_id,
                    text,
                } => {
                    if !super::same_generation(session, generation, query_session, query_generation)
                    {
                        output(&Event::VolumeSearch {
                            session: query_session,
                            generation: query_generation,
                            query_id,
                            host_pid: Some(std::process::id()),
                            result: Err("이전 검색 세대의 요청을 거부했습니다.".into()),
                        })?;
                    } else if let Some((old_session, old_generation, old_id, _)) =
                        query.replace((query_session, query_generation, query_id, text))
                    {
                        output(&Event::VolumeSearch {
                            session: old_session,
                            generation: old_generation,
                            query_id: old_id,
                            host_pid: Some(std::process::id()),
                            result: Err("새 검색어로 대체되었습니다.".into()),
                        })?;
                        queried_revision = u64::MAX;
                    } else {
                        queried_revision = u64::MAX;
                    }
                }
                HostCommand::CacheLoadBegin { .. }
                | HostCommand::CacheLoadChunk { .. }
                | HostCommand::CacheLoadEnd { .. }
                | HostCommand::CacheLoadUnavailable { .. }
                | HostCommand::CacheExportReady { .. }
                | HostCommand::CacheExportChunkAck { .. }
                | HostCommand::CacheExportResult { .. } => {
                    // Cache transport is inactive. Leave the in-memory search state intact.
                }
            }
        }
        if stop.is_requested() || (inbox.closed_and_empty() && notices.is_empty()) {
            returned.normal = true;
            return Ok(());
        }
        if settings.persistent() && resume_store.is_none() {
            resume_store = parent.owner_sid.clone().map(ResumeStore::start);
        }
        if discovery.elapsed() >= Duration::from_secs(5) {
            discovery = Instant::now();
            queried_revision = u64::MAX;
            let inventory_result = super::mounted_inventory().map(|mut inventory| {
                if service_mode {
                    inventory
                        .volumes
                        .retain(|volume| volume.classification == super::InventoryClass::Fixed);
                    inventory.network_drives.clear();
                }
                inventory
            });
            match inventory_result {
                Ok(inventory) => {
                    if inventory_error.take().is_some() {
                        dirty = true;
                    }
                    if network_drives != inventory.network_drives {
                        network_drives = inventory.network_drives;
                        dirty = true;
                    }
                    let all_guids = inventory
                        .volumes
                        .iter()
                        .map(|item| item.guid.clone())
                        .collect::<BTreeSet<_>>();
                    let (selected, next_overflow_count) = bound_active_volumes(inventory.volumes);
                    if overflow_count != next_overflow_count {
                        overflow_count = next_overflow_count;
                        dirty = true;
                    }
                    let selected_guids = selected
                        .iter()
                        .map(|item| item.guid.clone())
                        .collect::<BTreeSet<_>>();
                    for volume in &mut volumes {
                        if !all_guids.contains(&volume.guid) {
                            if volume.mark_offline() {
                                dirty = true;
                                if let Some(resume) = &mut resume_store {
                                    resume.observe_offline(&volume.guid);
                                }
                            }
                        }
                    }
                    for info in selected {
                        if let Some(resume) = &mut resume_store {
                            resume.observe_online(&info.guid);
                        }
                        if let Some(volume) =
                            volumes.iter_mut().find(|volume| volume.guid == info.guid)
                        {
                            dirty |= volume.apply_inventory(info, &settings, Instant::now());
                        } else {
                            let mut volume = Volume::new(info.clone(), stop.clone());
                            dirty |= volume.apply_inventory(info, &settings, Instant::now());
                            volumes.push(volume);
                        }
                    }
                    let offline_slots = MAX_ACTIVE_GUIDS.saturating_sub(selected_guids.len());
                    let mut offline_kept = 0usize;
                    let previous_len = volumes.len();
                    volumes.retain(|volume| {
                        if selected_guids.contains(&volume.guid) {
                            true
                        } else if !all_guids.contains(&volume.guid)
                            && volume.state == Phase::Offline
                            && offline_kept < offline_slots
                        {
                            offline_kept += 1;
                            true
                        } else {
                            false
                        }
                    });
                    if volumes.len() != previous_len {
                        dirty = true;
                    }
                    next_volume %= volumes.len().max(1);
                }
                Err(error) => {
                    let message = format!(
                        "전체 볼륨 조회 실패입니다. 기존 결과를 제공하지 않습니다. {error}"
                    );
                    if inventory_error.as_ref() != Some(&message) {
                        inventory_error = Some(message.clone());
                        dirty = true;
                    }
                    if !network_drives.is_empty() {
                        network_drives.clear();
                        dirty = true;
                    }
                    if overflow_count != 0 {
                        overflow_count = 0;
                        dirty = true;
                    }
                    for volume in &mut volumes {
                        dirty |= volume.mark_inventory_failure(message.clone());
                    }
                }
            }
        }
        if let Some(resume) = &mut resume_store {
            dirty |= resume.poll(&mut volumes, generation);
            let bytes = volumes.iter().fold(resume.detached_bytes(&volumes), |sum, volume| {
                sum.saturating_add(volume.allocated_bytes())
            });
            dirty |= resume.schedule(&mut volumes, &settings, generation, index_budget.saturating_sub(bytes));
        }
        let detached = resume_store.as_ref().map_or(0, |io| io.detached_bytes(&volumes));
        let cache_loading = resume_store.as_ref().is_some_and(ResumeStore::loading);
        let mut bytes = volumes.iter().fold(detached, |sum, volume| {
            sum.saturating_add(volume.allocated_bytes())
        });
        let states = volumes
            .iter()
            .map(|volume| {
                if volume.index.as_ref().is_some_and(|index| Arc::strong_count(index) > 1) {
                    // An immutable snapshot is saving on the I/O worker. It still serves queries.
                    Phase::Snapshot
                } else {
                    volume.state.clone()
                }
            })
            .collect::<Vec<_>>();
        if let Some(index) = (!cache_loading).then(|| next_fair_volume(&states, next_volume)).flatten() {
            next_volume = (index + 1) % volumes.len();
            let own_bytes = volumes[index].allocated_bytes();
            let other_bytes = bytes.saturating_sub(own_bytes);
            let over_limit = volumes[index].index.as_ref().is_none_or(|volume_index| {
                volume_resources_exceeded(volume_index, other_bytes, index_budget)
            });
            if over_limit {
                volumes[index].limit(format!(
                    "검색 색인 메모리 또는 {}개 항목 상한에 도달했습니다. 상한을 높여 다시 시작하세요.",
                    MAX_VOLUME_ENTRIES
                ));
                dirty = true;
            } else {
                match volumes[index].step(&mut scratch, other_bytes, index_budget) {
                    Ok(changed) => dirty |= changed,
                    Err(_error) if stop.is_requested() => {
                        returned.normal = true;
                        return Ok(());
                    }
                    Err(error) => {
                        let volume = &mut volumes[index];
                        if volume.restored && volume.state == Phase::CatchingUp {
                            volume.begin_mode(false);
                            if volume.state == Phase::Indexing {
                                volume.message = Some("저장된 색인의 변경분을 적용할 수 없어 이 드라이브를 다시 색인합니다.".into());
                            }
                        } else {
                            volume.fail(error);
                        }
                        dirty = true;
                    }
                }
            }
            let detached = resume_store.as_ref().map_or(0, |io| io.detached_bytes(&volumes));
            bytes = volumes.iter().fold(detached, |sum, volume| {
                sum.saturating_add(volume.allocated_bytes())
            });
        }
        let working = volumes
            .iter()
            .any(|volume| matches!(volume.state, Phase::Indexing | Phase::CatchingUp));
        if let Some(guid) = &rebuild_in_progress {
            if volumes
                .iter()
                .find(|volume| &volume.guid == guid)
                .is_none_or(|volume| {
                    matches!(
                        volume.state,
                        Phase::Live
                            | Phase::Offline
                            | Phase::Excluded
                            | Phase::Error
                            | Phase::Unsupported
                            | Phase::Unavailable
                            | Phase::Snapshot
                            | Phase::Limited
                    )
                })
            {
                rebuild_in_progress = None;
            }
        }
        let revision = volumes.iter().map(|volume| volume.revision).sum::<u64>();
        let mut statuses: Vec<_> = volumes.iter().map(Volume::status).collect();
        if overflow_count > 0 {
            statuses.push(VolumeStatus {
                root: NativePath::new(format!("추가 볼륨 {}개", overflow_count)),
                phase: Phase::Limited,
                entries: 0,
                allocated_bytes: 0,
                message: Some(format!(
                    "활성/표시 GUID 상한 {}개를 넘어 {}개 볼륨을 생략했습니다.",
                    MAX_ACTIVE_GUIDS, overflow_count
                )),
                guid: None,
                serial: None,
                mount: None,
            });
        }
        let complete =
            !statuses.is_empty() && statuses.iter().all(|status| status.phase == Phase::Live);
        if dirty && reported.elapsed() >= Duration::from_millis(300) {
            let mut statuses = statuses;
            for root in &network_drives {
                if !statuses.iter().any(|volume| &volume.root == root) {
                    statuses.push(VolumeStatus {
                        root: root.clone(),
                        phase: Phase::Unsupported,
                        entries: 0,
                        allocated_bytes: 0,
                        message: Some(
                            "네트워크 드라이브는 전체 드라이브 색인을 지원하지 않습니다.".into(),
                        ),
                        guid: None,
                        serial: None,
                        mount: Some(root.clone()),
                    });
                }
            }
            output(&Event::VolumeIndex {
                session,
                generation,
                host_pid: std::process::id(),
                status: Box::new(IndexStatus {
                    root: NativePath::new("전체 로컬 NTFS 드라이브"),
                    entries: statuses.iter().map(|status| status.entries).sum(),
                    skipped: statuses
                        .iter()
                        .filter(|status| status.phase != Phase::Live)
                        .count(),
                    complete,
                    limited: statuses.iter().any(|status| status.phase == Phase::Limited),
                    allocated_bytes: bytes,
                    live: true,
                    host_pid: Some(std::process::id()),
                    volumes: statuses,
                }),
            })?;
            reported = Instant::now();
            dirty = false;
        }
        if let Some((query_session, query_generation, query_id, text)) = &query {
            if queried_revision != revision && query_clock.elapsed() >= Duration::from_millis(250) {
                let cancel = &stop.requested;
                let budget = Budget::until(Instant::now() + Duration::from_millis(250), cancel);
                let results = if let Some(error) = &inventory_error {
                    Err(format!("전체 볼륨 검색을 사용할 수 없습니다. {error}"))
                } else if let Err(error) = Volume::query_scope_conflict(text, &settings, &volumes) {
                    Err(error)
                } else {
                    let queryable = volumes
                        .iter()
                        .filter(|volume| {
                            volume.mount.is_some()
                                && matches!(
                                    volume.volume_decision(&settings),
                                    Some(crate::index_policy::VolumeDecision::Included { .. })
                                )
                                && can_publish_query(&volume.state, volume.index.is_some())
                        })
                        .count();
                    match query_candidate_capacity(queryable) {
                        Err(error) => Err(error),
                        Ok(_) => {
                            let mut parts = Vec::with_capacity(queryable);
                            let mut failure = None;
                            for volume in volumes.iter().filter(|volume| {
                                volume.mount.is_some()
                                    && matches!(
                                        volume.volume_decision(&settings),
                                        Some(crate::index_policy::VolumeDecision::Included { .. })
                                    )
                                    && can_publish_query(&volume.state, volume.index.is_some())
                            }) {
                                if budget.check() {
                                    failure = Some(
                                        "검색 시간이 초과됐습니다. 검색 범위를 줄여 주세요."
                                            .to_string(),
                                    );
                                    break;
                                }
                                match volume.query(
                                    text,
                                    &settings,
                                    windows_components.as_ref(),
                                    &budget,
                                ) {
                                    Ok(part) => parts.push(part),
                                    Err(error) => {
                                        failure = Some(error);
                                        break;
                                    }
                                }
                            }
                            if let Some(error) = failure {
                                Err(error)
                            } else {
                                super::merge_with_budget(parts, complete, &budget).map(Box::new)
                            }
                        }
                    }
                };
                output(&Event::VolumeSearch {
                    session: *query_session,
                    generation: *query_generation,
                    query_id: *query_id,
                    host_pid: Some(std::process::id()),
                    result: results,
                })?;
                queried_revision = revision;
                query_clock = Instant::now();
            }
        }
        if cache_loading {
            std::thread::sleep(Duration::from_millis(12));
        } else if !working {
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

#[cfg(test)]
mod replay_tests {
    use super::*;

    fn journal(
        journal_id: u64,
        first_usn: i64,
        next_usn: i64,
        lowest_valid_usn: i64,
    ) -> JournalInfo {
        JournalInfo {
            journal_id,
            first_usn,
            next_usn,
            lowest_valid_usn,
            max_usn: i64::MAX,
            maximum_size: 0,
        }
    }

    fn record(usn: i64) -> goz_core::usn::ParsedUsnRecord {
        goz_core::usn::ParsedUsnRecord {
            major_version: 2,
            frn: goz_core::types::Frn(1),
            parent_frn: goz_core::types::Frn(2),
            usn,
            timestamp_ft: 0,
            reason: 0,
            attributes: 0,
            name: b"entry".to_vec(),
            name_lossy: false,
        }
    }

    fn page(
        next_usn: i64,
        records: Vec<goz_core::usn::ParsedUsnRecord>,
    ) -> goz_core::usn::JournalWalk {
        goz_core::usn::JournalWalk {
            next_usn,
            records,
            skipped: Default::default(),
        }
    }

    #[test]
    fn cursor_validation_checks_id_first_lowest_and_next() {
        assert!(cursor_is_valid(7, 100, journal(7, 100, 200, 90)));
        assert!(!cursor_is_valid(8, 100, journal(7, 100, 200, 90)));
        assert!(!cursor_is_valid(7, 99, journal(7, 100, 200, 90)));
        assert!(!cursor_is_valid(7, 99, journal(7, 50, 200, 100)));
        assert!(!cursor_is_valid(7, 201, journal(7, 100, 200, 90)));
    }

    #[test]
    fn page_validation_checks_candidate_and_record_interval() {
        let live = journal(7, 100, 200, 90);
        assert_eq!(
            validate_journal_page(7, 100, live, &page(151, vec![record(100), record(150)])),
            Ok(151)
        );
        let mut cursor = 100;
        let rejected = validate_journal_page(7, cursor, live, &page(151, vec![record(151)]));
        if let Ok(next) = rejected {
            cursor = next;
        }
        assert_eq!(cursor, 100);
        assert!(validate_journal_page(7, 100, live, &page(151, vec![record(99)])).is_err());
        assert!(validate_journal_page(7, 100, live, &page(201, vec![])).is_err());
        assert!(validate_journal_page(8, 100, live, &page(151, vec![record(100)])).is_err());
    }

    #[test]
    fn unsupported_pages_are_rejected() {
        let live = journal(7, 100, 200, 90);
        let mut v4 = page(151, vec![record(100)]);
        v4.skipped.v4 = 1;
        assert!(validate_journal_page(7, 100, live, &v4).is_err());
        let mut unknown = page(151, vec![record(100)]);
        unknown.skipped.unknown_version = 1;
        assert!(validate_journal_page(7, 100, live, &unknown).is_err());
    }

    #[test]
    fn eof_target_and_empty_page_rules_are_explicit() {
        let empty = page(100, vec![]);
        assert!(cursor_reached_high_water(100, 100));
        assert!(cursor_reached_high_water(101, 100));
        assert!(!cursor_reached_high_water(99, 100));
        assert!(is_live_idle_page(&Phase::Live, 100, &empty));
        assert!(!is_live_idle_page(&Phase::CatchingUp, 100, &empty));
        assert!(!is_live_idle_page(&Phase::Snapshot, 100, &empty));
        assert!(!is_live_idle_page(&Phase::Live, 99, &empty));
        let actual_window = journal(7, 100, 200, 90);
        assert_eq!(
            validate_journal_page(7, 100, actual_window, &page(151, vec![])),
            Ok(151)
        );
        assert!(validate_journal_page(7, 100, actual_window, &page(201, vec![])).is_err());
        assert!(validate_journal_page(7, 100, actual_window, &empty).is_ok());
    }

    #[test]
    fn queries_require_a_committed_live_or_snapshot_index() {
        assert!(can_publish_query(&Phase::Live, true));
        assert!(can_publish_query(&Phase::Snapshot, true));
        assert!(!can_publish_query(&Phase::CatchingUp, true));
        assert!(!can_publish_query(&Phase::Indexing, true));
        assert!(!can_publish_query(&Phase::Live, false));
    }

    #[test]
    fn monitor_off_completion_is_snapshot_and_not_active_is_layout_only() {
        assert_eq!(completed_phase(false), Phase::Live);
        assert_eq!(completed_phase(true), Phase::Snapshot);
        assert!(!layout_only_snapshot_allowed(false));
        assert!(layout_only_snapshot_allowed(true));
        assert!(can_publish_query(&completed_phase(true), true));
    }

    #[test]
    fn page_batches_never_exceed_the_record_bound() {
        let records = vec![0u8; MAX_PAGE_RECORDS + 1];
        let sizes = bounded_batches(&records)
            .map(|batch| batch.len())
            .collect::<Vec<_>>();
        assert_eq!(sizes, vec![MAX_PAGE_RECORDS, 1]);
    }

    #[test]
    fn resource_limits_include_links_and_require_optimize_headroom() {
        assert!(!resource_limits_exceeded(MAX_VOLUME_ENTRIES, 40, 59, 100));
        assert!(resource_limits_exceeded(
            MAX_VOLUME_ENTRIES + 1,
            0,
            0,
            u64::MAX
        ));
        assert!(resource_limits_exceeded(1, 40, 60, 100));
        assert!(!resource_limits_exceeded(1, 40, 59, 100));
        assert!(has_optimize_headroom(40, 30, 110));
        assert!(!has_optimize_headroom(40, 40, 110));
    }

    #[test]
    fn guid_overflow_is_reported_and_scheduler_rotates_fairly() {
        let inventory = (0..MAX_ACTIVE_GUIDS + 3)
            .map(|number| {
                let guid_text = format!("00000000-0000-0000-0000-{number:012x}");
                super::InventoryVolume {
                    guid: crate::index_policy::VolumeGuid::parse(&guid_text).unwrap(),
                    serial: Some(number as u32),
                    mounts: Vec::new(),
                    classification: super::InventoryClass::Fixed,
                    message: None,
                }
            })
            .collect();
        let (bounded, overflow) = bound_active_volumes(inventory);
        assert_eq!(bounded.len(), MAX_ACTIVE_GUIDS);
        assert_eq!(overflow, 3);
        assert!(
            bounded
                .windows(2)
                .all(|pair| pair[0].guid.as_str() < pair[1].guid.as_str())
        );
        let phases = [
            Phase::Snapshot,
            Phase::Indexing,
            Phase::Live,
            Phase::Limited,
        ];
        assert_eq!(next_fair_volume(&phases, 0), Some(1));
        assert_eq!(next_fair_volume(&phases, 2), Some(2));
        assert_eq!(next_fair_volume(&phases, 3), Some(1));
    }

    #[test]
    fn raw_link_parent_paths_reject_ambiguous_components() {
        let path = "\\dir\\file".encode_utf16().collect::<Vec<_>>();
        let (separator, name) = split_raw_link_path(&path).unwrap();
        assert_eq!(separator, 4);
        assert_eq!(String::from_utf16(name).unwrap(), "file");
        let root_file = "\\root-file".encode_utf16().collect::<Vec<_>>();
        let (separator, name) = split_raw_link_path(&root_file).unwrap();
        assert_eq!(separator, 0);
        assert_eq!(String::from_utf16(name).unwrap(), "root-file");
        let traversal = "\\dir\\..\\file".encode_utf16().collect::<Vec<_>>();
        let doubled_root = "\\\\server\\file".encode_utf16().collect::<Vec<_>>();
        assert!(split_raw_link_path(&traversal).is_err());
        assert!(split_raw_link_path(&doubled_root).is_err());
    }
}

#[cfg(test)]
mod ipc_tests {
    fn current_test_parent() -> ParentIdentity {
        let pid = std::process::id();
        let created_filetime =
            crate::live_search::service::current_process_created_filetime().unwrap();
        let process =
            crate::live_search::service::validate_parent_process(pid, created_filetime, None)
                .unwrap();
        ParentIdentity {
            process: Process(process),
            pid,
            created_filetime,
            owner_sid: None,
        }
    }
    use super::*;
    use ::windows::Win32::Security::{RevertToSelf, TOKEN_QUERY};
    use std::io::Read;
    #[test]
    fn background_index_io_restores_thread_policy_and_failed_volumes_back_off() {
        unsafe {
            let before = GetThreadPriority(GetCurrentThread());
            let background =
                BackgroundIo::enter().expect("Windows supports background thread mode");
            assert_eq!(
                SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN)
                    .unwrap_err()
                    .code(),
                HRESULT::from_win32(ERROR_THREAD_MODE_ALREADY_BACKGROUND.0)
            );
            drop(background);
            assert_eq!(GetThreadPriority(GetCurrentThread()), before);
            drop(BackgroundIo::enter().expect("background mode must have been restored"));
        }
        let guid =
            crate::index_policy::VolumeGuid::parse("00000000-0000-0000-0000-000000000000").unwrap();
        let mut failed = Volume::new(
            super::InventoryVolume {
                guid,
                serial: Some(1),
                mounts: vec![],
                classification: super::InventoryClass::Fixed,
                message: None,
            },
            std::sync::Arc::new(super::HostStop::default()),
        );
        failed.begin();
        assert_eq!(failed.state, Phase::Error);
        assert!(!failed.retry_ready(failed.retry_at - Duration::from_secs(1)));
        assert!(failed.retry_ready(failed.retry_at));
        failed.state = Phase::Offline;
        assert!(failed.retry_ready(Instant::now()));
    }
    #[test]
    fn standard_user_inventory_reports_fixed_ntfs_test_drives_once() {
        let inventory = super::super::mounted_inventory()
            .expect("standard-user volume inventory should complete");
        let mut seen_guids = Vec::new();
        for volume in &inventory.volumes {
            assert!(
                crate::index_policy::VolumeGuid::parse(volume.guid.as_str())
                    .is_ok_and(|guid| guid == volume.guid),
                "inventory returned an invalid volume GUID"
            );
            assert!(
                !seen_guids.contains(&volume.guid),
                "inventory returned a duplicate volume GUID"
            );
            seen_guids.push(volume.guid.clone());
            for mount in &volume.mounts {
                assert!(valid_mount(&mount.0), "inventory returned an invalid mount");
                assert!(
                    result_mount_matches(&volume.guid, mount),
                    "mount does not resolve to its reported volume GUID"
                );
            }
        }
        if let Some(system_drive) = std::env::var("SystemDrive")
            .ok()
            .filter(|drive| drive.as_bytes().get(1) == Some(&b':'))
            .map(|drive| {
                drive
                    .encode_utf16()
                    .chain(std::iter::once(92u16))
                    .collect::<Vec<_>>()
            })
        {
            if let Some(volume) = inventory.volumes.iter().find(|volume| {
                volume
                    .mounts
                    .iter()
                    .any(|mount| same_drive_letter_mount(&mount.0, &system_drive))
            }) {
                if volume.classification == crate::live_search::InventoryClass::Fixed {
                    assert!(
                        volume.serial.is_some(),
                        "fixed NTFS system volume needs a serial"
                    );
                }
            }
        }
        for (index, network) in inventory.network_drives.iter().enumerate() {
            assert!(
                !inventory.network_drives[..index]
                    .iter()
                    .any(|other| same_drive_letter_mount(&network.0, &other.0))
            );
            assert!(
                !inventory
                    .volumes
                    .iter()
                    .flat_map(|volume| volume.mounts.iter())
                    .any(|mount| same_drive_letter_mount(&network.0, &mount.0))
            );
        }
    }

    #[test]
    fn search_pipe_transports_data_without_delegating_a_client_token() {
        unsafe {
            let name = format!(r"\\.\pipe\SDIR-search-{:?}", CoCreateGuid().unwrap());
            let wname = wide(&name);
            let h = CreateNamedPipeW(
                PCWSTR(wname.as_ptr()),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                4096,
                4096,
                0,
                None,
            );
            assert_ne!(h, INVALID_HANDLE_VALUE);
            let mut server = File::from_raw_handle(h.0);
            let (release, wait) = crossbeam_channel::bounded::<()>(1);
            let client = std::thread::spawn(move || {
                let mut p = connect_host_pipe(&name, &current_test_parent(), true).unwrap();
                p.write_all(b"hello").unwrap();
                wait.recv_timeout(Duration::from_secs(5)).unwrap();
            });
            if let Err(e) = ConnectNamedPipe(h, None) {
                assert_eq!(e.code(), HRESULT::from_win32(ERROR_PIPE_CONNECTED.0));
            }
            let mut data = [0; 5];
            server.read_exact(&mut data).unwrap();
            assert_eq!(&data, b"hello");
            // Anonymous SQOS either prevents impersonation or makes opening the token fail.
            if let Err(e) = ImpersonateNamedPipeClient(h) {
                assert_eq!(e.code(), HRESULT::from_win32(ERROR_CANNOT_IMPERSONATE.0));
            } else {
                struct Revert;
                impl Drop for Revert {
                    fn drop(&mut self) {
                        unsafe {
                            RevertToSelf().expect("must revert pipe impersonation");
                        }
                    }
                }
                let _revert = Revert;
                let mut token = HANDLE::default();
                let result = OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, true, &mut token);
                if result.is_ok() {
                    let _owned = Process(token);
                    panic!("anonymous pipe client token must not be available for delegation");
                }
                assert_eq!(
                    result.unwrap_err().code(),
                    HRESULT::from_win32(ERROR_CANT_OPEN_ANONYMOUS.0)
                );
            }
            release.send(()).unwrap();
            client.join().unwrap();
        }
    }

    #[test]
    fn search_status_flows_while_query_read_is_pending_and_query_pipe_closes() {
        unsafe {
            let name = format!(r"\\.\pipe\SDIR-search-{:?}", CoCreateGuid().unwrap());
            let security = PipeSecurity::current_user().unwrap();
            let attributes = ::windows::Win32::Security::SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<::windows::Win32::Security::SECURITY_ATTRIBUTES>()
                    as u32,
                lpSecurityDescriptor: security.0.0,
                bInheritHandle: false.into(),
            };
            let mut events = create_server_pipe(&name, &attributes).unwrap();
            let mut commands = create_server_pipe(&format!("{name}-queries"), &attributes).unwrap();
            let (started, waiting) = crossbeam_channel::bounded(1);
            let (finished, done) = crossbeam_channel::bounded(1);
            let host = std::thread::spawn(move || {
                let parent = current_test_parent();
                let mut output = connect_host_pipe(&name, &parent, true).unwrap();
                let mut input =
                    connect_host_pipe(&format!("{name}-queries"), &parent, false).unwrap();
                let reader = std::thread::spawn(move || {
                    started.send(()).unwrap();
                    let mut byte = [0];
                    input.read_exact(&mut byte).unwrap();
                    assert_eq!(byte, [7]);
                    assert!(input.read_exact(&mut byte).is_err());
                    finished.send(()).unwrap();
                });
                waiting.recv_timeout(Duration::from_secs(2)).unwrap();
                output.write_all(&[42]).unwrap();
                reader.join().unwrap();
            });
            let input_closed = std::sync::atomic::AtomicBool::new(false);
            connect_server_pipe(
                &events,
                std::process::id(),
                GetCurrentProcess(),
                &input_closed,
            )
            .unwrap();
            connect_server_pipe(
                &commands,
                std::process::id(),
                GetCurrentProcess(),
                &input_closed,
            )
            .unwrap();
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                let mut available = 0;
                PeekNamedPipe(
                    HANDLE(events.as_raw_handle()),
                    None,
                    0,
                    None,
                    Some(&mut available),
                    None,
                )
                .unwrap();
                if available > 0 {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "status blocked behind a pending query read"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            let mut byte = [0];
            events.read_exact(&mut byte).unwrap();
            assert_eq!(byte, [42]);
            commands.write_all(&[7]).unwrap();
            drop(commands);
            done.recv_timeout(Duration::from_secs(2)).unwrap();
            host.join().unwrap();
        }
    }

    fn synthetic_process(script: &str) -> (std::process::Child, Process) {
        use ::windows::Win32::Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle};
        use std::{os::windows::io::AsRawHandle, process::Stdio};
        let system_root = std::env::var_os("SystemRoot").expect("Windows SystemRoot");
        let powershell = std::path::PathBuf::from(system_root)
            .join("System32")
            .join("WindowsPowerShell")
            .join("v1.0")
            .join("powershell.exe");
        assert!(powershell.is_file());
        let child = std::process::Command::new(powershell)
            .args(["-NoProfile", "-Command", script])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let source = HANDLE(child.as_raw_handle());
        let mut owned = HANDLE::default();
        unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                source,
                GetCurrentProcess(),
                &mut owned,
                0,
                false,
                DUPLICATE_SAME_ACCESS,
            )
            .unwrap();
        }
        (child, Process(owned))
    }

    #[test]
    fn system_components_are_bound_to_the_actual_windows_volume() {
        let (guid, relative) = windows_components_scope().expect("Windows component store volume");
        assert_eq!(relative.as_str().rsplit('/').next(), Some("WinSxS"));
        assert!(
            mounted_inventory()
                .unwrap()
                .volumes
                .iter()
                .any(|volume| volume.guid == guid)
        );
    }

    #[test]
    fn stdin_eof_interrupts_initial_ack_wait_without_host_ack() {
        use std::sync::{Arc, atomic::AtomicBool};

        let (mut child, host) = synthetic_process("Start-Sleep -Seconds 30");
        let (request_tx, requests) = crossbeam_channel::unbounded();
        request_tx.send(Request::ProxyInputClosed).unwrap();
        drop(request_tx);
        let (forwarded, _forwarded_rx) = crossbeam_channel::unbounded();
        let input_closed = Arc::new(AtomicBool::new(false));
        let supervised = input_closed.clone();
        std::thread::spawn(move || {
            supervise_proxy_requests(vec![], requests, forwarded, supervised);
        });
        let deadline = Instant::now() + Duration::from_secs(1);
        while !input_closed.load(std::sync::atomic::Ordering::Acquire) {
            assert!(Instant::now() < deadline, "stdin EOF was not supervised");
            std::thread::sleep(Duration::from_millis(5));
        }
        let (ack_tx, ack_rx) = crossbeam_channel::bounded(1);
        let _ack_keepalive = ack_tx;
        let events_done = AtomicBool::new(false);
        assert!(matches!(
            poll_proxy_ack(
                &ack_rx,
                &input_closed,
                &events_done,
                &host,
                Instant::now(),
                &std::sync::Mutex::new(None)
            )
            .unwrap(),
            ProxyAckPoll::Stopped
        ));
        let (exit_code, forced) = terminate_owned_host(&host).unwrap();
        assert!(forced);
        assert_eq!(exit_code, HOST_FORCE_EXIT_CODE);
        assert_eq!(child.wait().unwrap().code(), Some(exit_code as i32));
    }

    #[test]
    fn owned_host_wait_reports_real_synthetic_windows_exit_code() {
        use std::time::Duration;

        let (mut child, host) = synthetic_process("exit 23");
        assert_eq!(child.wait().unwrap().code(), Some(23));
        assert_eq!(
            wait_owned_host(&host, Duration::from_secs(2)).unwrap(),
            Some(23)
        );
    }

    #[test]
    fn command_writer_stop_cancels_a_blocked_named_pipe_write() {
        use std::{os::windows::io::AsRawHandle, sync::atomic::AtomicBool};

        unsafe {
            let name = format!(r"\\.\pipe\SDIR-stop-test-{:?}", CoCreateGuid().unwrap());
            let security = PipeSecurity::current_user().unwrap();
            let attributes = ::windows::Win32::Security::SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<::windows::Win32::Security::SECURITY_ATTRIBUTES>()
                    as u32,
                lpSecurityDescriptor: security.0.0,
                bInheritHandle: false.into(),
            };
            let server = create_server_pipe(&name, &attributes).unwrap();
            let client = connect_host_pipe(&name, &current_test_parent(), false).unwrap();
            let input_closed = AtomicBool::new(false);
            connect_server_pipe(
                &server,
                std::process::id(),
                GetCurrentProcess(),
                &input_closed,
            )
            .unwrap();
            let client_handle = HANDLE(client.as_raw_handle());
            let mut writer = ProxyCommandWriter::start(server).unwrap();
            for query_id in 1..=16 {
                writer
                    .try_send(HostCommand::Query {
                        session: 7,
                        generation: 0,
                        query_id,
                        text: "x".repeat(4096),
                    })
                    .expect("bounded synthetic commands fit before the server pipe fills");
            }
            let mut available = 0;
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                PeekNamedPipe(client_handle, None, 0, None, Some(&mut available), None).unwrap();
                if available >= 60_000 || Instant::now() >= deadline {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            let stopped = writer.stop(HOST_WRITER_STOP_TIMEOUT);
            assert!(
                available >= 60_000,
                "synthetic host did not fill its pipe buffer"
            );
            assert!(
                stopped.unwrap(),
                "CancelSynchronousIo must release the writer"
            );
            drop(client);
        }
    }
    #[test]
    fn config_changes_restart_only_the_guid_whose_effective_decision_changed() {
        let first =
            crate::index_policy::VolumeGuid::parse("00000000-0000-0000-0000-000000000001").unwrap();
        let second =
            crate::index_policy::VolumeGuid::parse("00000000-0000-0000-0000-000000000002").unwrap();
        let before = crate::index_policy::NtfsIndexingSettings::default();
        let mut after = before.clone();
        after
            .set_volume_policy(crate::index_policy::VolumePolicy::new(
                first.clone(),
                crate::index_policy::VolumeOverride::Exclude,
                true,
                crate::index_policy::ScopePolicy::default(),
            ))
            .unwrap();
        assert_ne!(
            before.decision_for(&first, crate::index_policy::NtfsVolumeClass::Fixed),
            after.decision_for(&first, crate::index_policy::NtfsVolumeClass::Fixed)
        );
        assert_eq!(
            before.decision_for(&second, crate::index_policy::NtfsVolumeClass::Fixed),
            after.decision_for(&second, crate::index_policy::NtfsVolumeClass::Fixed)
        );

        let mut scoped = before.clone();
        let root = crate::index_policy::RelativePathRoot::parse("work").unwrap();
        let scope = crate::index_policy::ScopePolicy::new(vec![root], Vec::new()).unwrap();
        scoped
            .set_volume_policy(crate::index_policy::VolumePolicy::new(
                first.clone(),
                crate::index_policy::VolumeOverride::Auto,
                true,
                scope,
            ))
            .unwrap();
        assert_eq!(
            before.decision_for(&first, crate::index_policy::NtfsVolumeClass::Fixed),
            scoped.decision_for(&first, crate::index_policy::NtfsVolumeClass::Fixed)
        );
    }

    #[test]
    fn explicit_rebuild_mode_accepts_snapshot_and_live_volumes_only() {
        assert_eq!(
            rebuild_snapshot_mode(Some(crate::index_policy::VolumeDecision::Included {
                monitor: false
            })),
            Some(true)
        );
        assert_eq!(
            rebuild_snapshot_mode(Some(crate::index_policy::VolumeDecision::Included {
                monitor: true
            })),
            Some(false)
        );
        assert_eq!(
            rebuild_snapshot_mode(Some(crate::index_policy::VolumeDecision::Excluded)),
            None
        );
        assert_eq!(rebuild_snapshot_mode(None), None);
    }

    #[test]
    fn scope_graph_rejection_runs_before_publish_state_filter() {
        let guid =
            crate::index_policy::VolumeGuid::parse("00000000-0000-0000-0000-000000000001").unwrap();
        let scope = crate::index_policy::ScopePolicy::new(
            vec![crate::index_policy::RelativePathRoot::parse("work").unwrap()],
            Vec::new(),
        )
        .unwrap();
        let mut settings = crate::index_policy::NtfsIndexingSettings::default();
        settings
            .set_volume_policy(crate::index_policy::VolumePolicy::new(
                guid.clone(),
                crate::index_policy::VolumeOverride::Include,
                true,
                scope,
            ))
            .unwrap();
        let volume = Volume::new(
            super::InventoryVolume {
                guid,
                serial: Some(1),
                mounts: vec![NativePath::new("C:")],
                classification: super::InventoryClass::Fixed,
                message: None,
            },
            std::sync::Arc::new(HostStop::default()),
        );
        assert_eq!(volume.state, Phase::Unavailable);
        assert!(
            Volume::query_scope_conflict(
                "parent-count:1",
                &settings,
                std::slice::from_ref(&volume)
            )
            .is_err()
        );
        let unscoped = crate::index_policy::NtfsIndexingSettings::default();
        assert!(
            Volume::query_scope_conflict(
                "parent-count:1",
                &unscoped,
                std::slice::from_ref(&volume)
            )
            .is_ok()
        );
    }

    #[test]
    fn query_workspace_is_bounded_before_path_allocation() {
        assert_eq!(
            query_candidate_capacity(MAX_ACTIVE_GUIDS).unwrap(),
            MAX_ACTIVE_GUIDS * MAX_QUERY_HITS_PER_VOLUME
        );
        assert!(query_candidate_capacity(MAX_ACTIVE_GUIDS + 1).is_err());
    }
}
