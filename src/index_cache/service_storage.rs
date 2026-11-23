use super::*;
use std::{
    ffi::c_void,
    io::{BufReader, BufWriter},
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle},
    },
    path::Prefix,
    ptr,
};
use windows::{
    Win32::{
        Foundation::{
            ERROR_SUCCESS, GENERIC_ALL, GENERIC_READ, GENERIC_WRITE, HANDLE, HLOCAL, LocalFree,
        },
        Security::{
            ACCESS_ALLOWED_ACE, ACE_HEADER, ACL,
            Authorization::{
                ConvertStringSecurityDescriptorToSecurityDescriptorW, ConvertStringSidToSidW,
                GetSecurityInfo, SE_FILE_OBJECT,
            },
            DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetSecurityDescriptorControl,
            INHERIT_ONLY_ACE, IsWellKnownSid, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
            PSID, SE_DACL_PROTECTED, SECURITY_ATTRIBUTES, WinBuiltinAdministratorsSid,
            WinLocalSystemSid,
        },
        Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, CREATE_NEW, CreateDirectoryW, CreateFileW, DELETE,
            FILE_ALL_ACCESS, FILE_APPEND_DATA, FILE_ATTRIBUTE_DIRECTORY,
            FILE_ATTRIBUTE_REPARSE_POINT, FILE_CREATION_DISPOSITION, FILE_DELETE_CHILD,
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_WRITE_ATTRIBUTES, FILE_WRITE_DATA,
            FILE_WRITE_EA, GetDriveTypeW, GetFileInformationByHandle, MOVEFILE_REPLACE_EXISTING,
            MOVEFILE_WRITE_THROUGH, MoveFileExW, OPEN_EXISTING, READ_CONTROL, WRITE_DAC,
            WRITE_OWNER,
        },
        System::SystemServices::{ACCESS_ALLOWED_ACE_TYPE, ACCESS_DENIED_ACE_TYPE},
    },
    core::{PCWSTR, w},
};

const BUFFER_BYTES: usize = 64 * 1024;
const DRIVE_FIXED: u32 = 3;

// Keep the verified directory chain open without FILE_SHARE_DELETE while the store lives.
pub(super) struct ServiceStorage {
    _directories: Vec<File>,
}

impl CacheStore {
    pub(crate) fn open_service_at(image: &Path, verified_owner_sid: &str) -> CacheResult<Self> {
        let service_dir = image.parent().ok_or(CacheError::UnsafePath)?;
        let mut components = service_dir.components();
        if !matches!(components.next(), Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_)))
            || components.next() != Some(Component::RootDir)
        {
            return Err(CacheError::UnsupportedLocation);
        }
        let mut parent = image.components().take(2).collect::<PathBuf>();
        if unsafe { GetDriveTypeW(PCWSTR(wide(&parent).as_ptr())) } != DRIVE_FIXED {
            return Err(CacheError::UnsupportedLocation);
        }
        let sid_path = Path::new(verified_owner_sid);
        if !matches!(sid_path.components().next(), Some(Component::Normal(part)) if part == sid_path.as_os_str())
        {
            return Err(CacheError::UnsafePath);
        }
        let owner = wide(sid_path);
        LocalAllocation::parse_sid(PCWSTR(owner.as_ptr()))?;
        let installer = LocalAllocation::parse_sid(w!(
            "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464"
        ))?;
        let mut directories = vec![open_protected(&parent, true, true, &installer)?];
        for component in components {
            let Component::Normal(part) = component else {
                return Err(CacheError::UnsafePath);
            };
            parent.push(part);
            directories.push(open_protected(&parent, true, false, &installer)?);
        }
        // Hold the executable through cache initialization along with the retained ancestors.
        let _image = open_protected(image, false, false, &installer)?;
        for name in [
            "ResumeCache",
            verified_owner_sid,
            CACHE_SUBDIR,
            CACHE_VERSION_DIR,
        ] {
            parent.push(name);
            directories.push(open_or_create_private_directory(&parent)?);
        }
        let cache_root = parent.parent().ok_or(CacheError::UnsafePath)?;
        let lock_path = cache_root.join(LOCK_FILE);
        let lock = match open_private(&lock_path, GENERIC_READ.0 | GENERIC_WRITE.0) {
            Ok(Some(lock)) => lock,
            Ok(None) => match create_private_file(&lock_path) {
                Err(CacheError::Io(error)) if error.kind() == io::ErrorKind::AlreadyExists => {
                    open_private(&lock_path, GENERIC_READ.0 | GENERIC_WRITE.0)?
                        .ok_or(CacheError::UnsafePath)?
                }
                result => result?,
            },
            Err(error) => return Err(error),
        };
        match lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => return Err(CacheError::Busy),
            Err(std::fs::TryLockError::Error(error)) => return Err(CacheError::Io(error)),
        }
        let store = Self {
            directory: parent,
            _lock_file: lock,
            limits: CacheLimits::default(),
            service: Some(ServiceStorage {
                _directories: directories,
            }),
        };
        for entry in fs::read_dir(&store.directory)? {
            let entry = entry?;
            if entry
                .file_name()
                .to_str()
                .and_then(classify_managed_name)
                .is_some()
                && !check_private_file(&entry.path())?
            {
                return Err(CacheError::UnsafePath);
            }
        }
        store.remove_stale_temps()?;
        Ok(store)
    }

    pub(crate) fn load_verified_service(
        &mut self,
        expected: &CacheIdentity,
        memory_budget: u64,
    ) -> CacheResult<Option<CachedVolume>> {
        if self.service.is_none() {
            return Err(CacheError::UnsafePath);
        }
        let Some(file) = open_private(&self.cache_path(&expected.volume_guid), GENERIC_READ.0)?
        else {
            return Ok(None);
        };
        let disk_bytes = file.metadata()?.len();
        let reader = BufReader::with_capacity(BUFFER_BYTES, file);
        load_bounded_cache(reader, &self.limits, expected, disk_bytes, memory_budget)
    }

    pub(crate) fn save_verified_service(
        &mut self,
        index: &VolumeIndex,
        metadata: &CacheMetadata,
    ) -> CacheResult<CacheStats> {
        if self.service.is_none() {
            return Err(CacheError::UnsafePath);
        }
        self.save_using(metadata, |file, limits| {
            let mut writer = BufWriter::with_capacity(BUFFER_BYTES, file);
            let stats = encode_cache(index, metadata, limits, &mut writer)?;
            writer.flush()?;
            Ok(stats)
        })
    }
}

pub(super) fn verify_encoded_file(
    file: &mut File,
    limits: &CacheLimits,
    metadata: &CacheMetadata,
    stats: &CacheStats,
) -> CacheResult<()> {
    let disk_bytes = file.metadata()?.len();
    let mut reader = BufReader::with_capacity(BUFFER_BYTES, file);
    let header = read_header(&mut reader, limits)?;
    if header.metadata != *metadata
        || header.row_count != stats.rows
        || header.payload_bytes != stats.payload_bytes
        || header.crc32 != stats.crc32
        || disk_bytes != stats.file_bytes
        || disk_bytes != header.file_bytes()?
    {
        return Err(CacheError::Corrupt(
            "temporary cache header or length changed",
        ));
    }
    let mut payload = PayloadReader::new(&mut reader, header.payload_bytes);
    let mut buffer = [0; BUFFER_BYTES];
    while payload.remaining != 0 {
        let len = payload.remaining.min(buffer.len() as u64) as usize;
        payload.read_exact(&mut buffer[..len])?;
    }
    verify_payload_end(payload, header.crc32)
}

pub(super) fn replace_file(source: &Path, target: &Path) -> CacheResult<()> {
    let source = wide(source);
    let target = wide(target);
    win(unsafe {
        MoveFileExW(
            PCWSTR(source.as_ptr()),
            PCWSTR(target.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    })
}

pub(super) fn check_private_file(path: &Path) -> CacheResult<bool> {
    Ok(open_private(path, 0)?.is_some())
}

fn open_private(path: &Path, access: u32) -> CacheResult<Option<File>> {
    match open_handle(path, false, access, OPEN_EXISTING, None) {
        Ok(file) => {
            verify_private(&file, false)?;
            Ok(Some(file))
        }
        Err(CacheError::Io(error)) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub(super) fn create_private_file(path: &Path) -> CacheResult<File> {
    let descriptor = LocalAllocation::private_descriptor()?;
    let attributes = descriptor.attributes();
    // CREATE_NEW plus an explicit descriptor prevents opening or chmodding an existing file.
    let file = open_handle(
        path,
        false,
        GENERIC_READ.0 | GENERIC_WRITE.0,
        CREATE_NEW,
        Some(&attributes),
    )?;
    if let Err(error) = verify_private(&file, false) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(error);
    }
    Ok(file)
}

fn open_or_create_private_directory(path: &Path) -> CacheResult<File> {
    let file = match open_handle(path, true, 0, OPEN_EXISTING, None) {
        Ok(file) => file,
        Err(CacheError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
            let descriptor = LocalAllocation::private_descriptor()?;
            let attributes = descriptor.attributes();
            match win(unsafe { CreateDirectoryW(PCWSTR(wide(path).as_ptr()), Some(&attributes)) }) {
                Err(CacheError::Io(error)) if error.kind() == io::ErrorKind::AlreadyExists => {}
                result => result?,
            }
            open_handle(path, true, 0, OPEN_EXISTING, None)?
        }
        Err(error) => return Err(error),
    };
    verify_private(&file, true)?;
    Ok(file)
}

fn open_handle(
    path: &Path,
    directory: bool,
    access: u32,
    disposition: FILE_CREATION_DISPOSITION,
    attributes: Option<&SECURITY_ATTRIBUTES>,
) -> CacheResult<File> {
    let mut flags = FILE_FLAG_OPEN_REPARSE_POINT;
    if directory {
        flags |= FILE_FLAG_BACKUP_SEMANTICS;
    }
    let handle = win(unsafe {
        CreateFileW(
            PCWSTR(wide(path).as_ptr()),
            access | READ_CONTROL.0 | FILE_READ_ATTRIBUTES.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            attributes.map(ptr::from_ref),
            disposition,
            flags,
            None,
        )
    })?;
    Ok(unsafe { File::from_raw_handle(handle.0) })
}

fn verify_kind(file: &File, directory: bool) -> CacheResult<()> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    win(unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) })?;
    if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
        || (info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0) != directory
        || (!directory && info.nNumberOfLinks != 1)
    {
        return Err(CacheError::UnsafePath);
    }
    Ok(())
}

fn open_protected(
    path: &Path,
    directory: bool,
    drive_root: bool,
    installer: &LocalAllocation,
) -> CacheResult<File> {
    let file = open_handle(path, directory, 0, OPEN_EXISTING, None)?;
    verify_kind(&file, directory)?;
    let security = FileSecurity::read(&file)?;
    let trusted =
        |sid| private_account(sid).is_some() || unsafe { EqualSid(sid, PSID(installer.0)).is_ok() };
    if !trusted(security.owner) {
        return Err(CacheError::UnsafePath);
    }
    let mut forbidden = GENERIC_ALL.0
        | GENERIC_WRITE.0
        | WRITE_DAC.0
        | WRITE_OWNER.0
        | DELETE.0
        | FILE_DELETE_CHILD.0
        | FILE_WRITE_EA.0
        | FILE_WRITE_ATTRIBUTES.0;
    if !drive_root {
        forbidden |= FILE_WRITE_DATA.0 | FILE_APPEND_DATA.0;
    }
    // Creating siblings in the drive root cannot replace the retained protected directory.
    security.for_each_ace(|header, mask, sid| {
        if header.AceFlags & INHERIT_ONLY_ACE.0 as u8 == 0
            && u32::from(header.AceType) == ACCESS_ALLOWED_ACE_TYPE
            && mask & forbidden != 0
            && !trusted(sid)
        {
            return Err(CacheError::UnsafePath);
        }
        Ok(())
    })?;
    Ok(file)
}

fn verify_private(file: &File, directory: bool) -> CacheResult<()> {
    verify_kind(file, directory)?;
    let security = FileSecurity::read(file)?;
    let mut control = 0;
    let mut revision = 0;
    win(unsafe {
        GetSecurityDescriptorControl(
            PSECURITY_DESCRIPTOR(security.descriptor.0),
            &mut control,
            &mut revision,
        )
    })?;
    if private_account(security.owner) != Some(0) || control & SE_DACL_PROTECTED.0 == 0 {
        return Err(CacheError::UnsafePath);
    }
    let mut full = [false; 2];
    security.for_each_ace(|header, mask, sid| {
        let account = private_account(sid).ok_or(CacheError::UnsafePath)?;
        if u32::from(header.AceType) != ACCESS_ALLOWED_ACE_TYPE {
            return Err(CacheError::UnsafePath);
        }
        if header.AceFlags & INHERIT_ONLY_ACE.0 as u8 == 0 && mask == FILE_ALL_ACCESS.0 {
            full[account] = true;
        }
        Ok(())
    })?;
    if full != [true; 2] {
        return Err(CacheError::UnsafePath);
    }
    Ok(())
}

fn private_account(sid: PSID) -> Option<usize> {
    if unsafe { IsWellKnownSid(sid, WinLocalSystemSid).as_bool() } {
        Some(0)
    } else if unsafe { IsWellKnownSid(sid, WinBuiltinAdministratorsSid).as_bool() } {
        Some(1)
    } else {
        None
    }
}

struct FileSecurity {
    descriptor: LocalAllocation,
    owner: PSID,
    dacl: *mut ACL,
}

impl FileSecurity {
    fn read(file: &File) -> CacheResult<Self> {
        let mut owner = PSID::default();
        let mut dacl = ptr::null_mut();
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        let status = unsafe {
            GetSecurityInfo(
                HANDLE(file.as_raw_handle()),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                Some(&mut owner),
                None,
                Some(&mut dacl),
                None,
                Some(&mut descriptor),
            )
        };
        if status != ERROR_SUCCESS {
            return Err(CacheError::Io(io::Error::from_raw_os_error(
                status.0 as i32,
            )));
        }
        let descriptor = LocalAllocation(descriptor.0);
        if owner.0.is_null() || dacl.is_null() {
            return Err(CacheError::UnsafePath);
        }
        Ok(Self {
            descriptor,
            owner,
            dacl,
        })
    }

    fn for_each_ace(
        &self,
        mut check: impl FnMut(ACE_HEADER, u32, PSID) -> CacheResult<()>,
    ) -> CacheResult<()> {
        for i in 0..u32::from(unsafe { (*self.dacl).AceCount }) {
            let mut ace = ptr::null_mut();
            win(unsafe { GetAce(self.dacl, i, &mut ace) })?;
            let header = unsafe { *ace.cast::<ACE_HEADER>() };
            if !matches!(
                u32::from(header.AceType),
                ACCESS_ALLOWED_ACE_TYPE | ACCESS_DENIED_ACE_TYPE
            ) || usize::from(header.AceSize) < size_of::<ACCESS_ALLOWED_ACE>()
            {
                return Err(CacheError::UnsafePath);
            }
            // Ordinary allow and deny ACEs share the Mask and variable-length SID layout.
            let ace = ace.cast::<ACCESS_ALLOWED_ACE>();
            let sid = PSID(unsafe { ptr::addr_of_mut!((*ace).SidStart).cast() });
            check(header, unsafe { (*ace).Mask }, sid)?;
        }
        Ok(())
    }
}

struct LocalAllocation(*mut c_void);

impl LocalAllocation {
    fn private_descriptor() -> CacheResult<Self> {
        let mut raw = PSECURITY_DESCRIPTOR::default();
        win(unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                w!("O:SYD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)"),
                1,
                &mut raw,
                None,
            )
        })?;
        Ok(Self(raw.0))
    }

    fn parse_sid(text: PCWSTR) -> CacheResult<Self> {
        let mut raw = PSID::default();
        win(unsafe { ConvertStringSidToSidW(text, &mut raw) })?;
        Ok(Self(raw.0))
    }

    fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.0,
            bInheritHandle: false.into(),
        }
    }
}

impl Drop for LocalAllocation {
    fn drop(&mut self) {
        unsafe {
            let _ = LocalFree(Some(HLOCAL(self.0)));
        }
    }
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

fn win<T>(result: windows::core::Result<T>) -> CacheResult<T> {
    result.map_err(|error| {
        // windows-result converts to a raw HRESULT; unwrap FACILITY_WIN32 for ErrorKind.
        let code = error.code().0 as u32;
        let io_error = if code & 0xffff_0000 == 0x8007_0000 {
            io::Error::from_raw_os_error((code & 0xffff) as i32)
        } else {
            io::Error::other(error)
        };
        CacheError::Io(io_error)
    })
}
