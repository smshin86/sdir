#[cfg(windows)]
mod windows_service {
    use std::{
        ffi::c_void,
        mem::size_of,
        os::windows::ffi::OsStringExt,
        path::{Path, PathBuf},
        ptr,
        sync::atomic::{AtomicPtr, Ordering},
        thread,
        time::{Duration, Instant},
    };

    use windows::Win32::{
        Foundation::{
            CloseHandle, ERROR_CALL_NOT_IMPLEMENTED, ERROR_SERVICE_ALREADY_RUNNING,
            ERROR_SERVICE_DOES_NOT_EXIST, ERROR_SERVICE_SPECIFIC_ERROR, ERROR_SUCCESS, FILETIME,
            HANDLE, HLOCAL,
        },
        Security::{
            Authorization::ConvertSidToStringSidW, GetTokenInformation, TOKEN_QUERY, TOKEN_USER,
            TokenSessionId, TokenUser,
        },
        System::{
            Registry::{
                HKEY, HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE, KEY_WOW64_64KEY, REG_DWORD,
                REG_EXPAND_SZ, REG_SZ, RegCloseKey, RegOpenKeyExW, RegQueryValueExW,
            },
            RemoteDesktop::WTSGetActiveConsoleSessionId,
            Services::{
                CloseServiceHandle, OpenSCManagerW, OpenServiceW, QueryServiceStatusEx,
                RegisterServiceCtrlHandlerExW, SC_HANDLE, SC_MANAGER_CONNECT,
                SC_STATUS_PROCESS_INFO, SERVICE_ACCEPT_STOP, SERVICE_CONTROL_INTERROGATE,
                SERVICE_CONTROL_STOP, SERVICE_DEMAND_START, SERVICE_QUERY_STATUS, SERVICE_RUNNING,
                SERVICE_START, SERVICE_START_PENDING, SERVICE_STATUS, SERVICE_STATUS_HANDLE,
                SERVICE_STATUS_PROCESS, SERVICE_STOP_PENDING, SERVICE_STOPPED,
                SERVICE_TABLE_ENTRYW, SERVICE_WIN32_OWN_PROCESS, SetServiceStatus,
                StartServiceCtrlDispatcherW, StartServiceW,
            },
            Threading::{
                GetCurrentProcess, GetProcessId, GetProcessTimes, OpenProcess, OpenProcessToken,
                PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, QueryFullProcessImageNameW,
            },
        },
        UI::Shell::{FOLDERID_ProgramFiles, KNOWN_FOLDER_FLAG, SHGetKnownFolderPath},
    };
    use windows::core::{HRESULT, PCWSTR, PWSTR};

    const SERVICE_NAME: &str = "SDIRIndex";
    const OWNER_KEY: &str = "SOFTWARE\\SDIR\\IndexService";
    const SERVICE_KEY: &str = "SYSTEM\\CurrentControlSet\\Services\\SDIRIndex";
    const SERVICE_FOLDER: &str = "SDIR Index Service";
    const SERVICE_EXE: &str = "SDIRIndexService.exe";
    const START_TIMEOUT: Duration = Duration::from_secs(20);
    const MAX_REGISTRY_VALUE_BYTES: u32 = 8192;
    const MAX_SERVICE_ARGUMENTS: usize = 13;

    static SERVICE_STATUS_HANDLE_PTR: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());

    struct ScHandle(SC_HANDLE);

    impl Drop for ScHandle {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseServiceHandle(self.0);
            }
        }
    }

    struct OwnedHandle(HANDLE);

    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    struct RegistryKey(HKEY);

    impl Drop for RegistryKey {
        fn drop(&mut self) {
            unsafe {
                let _ = RegCloseKey(self.0);
            }
        }
    }

    pub(crate) struct ClientService {
        _manager: ScHandle,
        service: ScHandle,
        owner_sid: String,
        expected_image: PathBuf,
    }

    pub(crate) struct StartedProcess {
        pub handle: HANDLE,
        pub pid: u32,
    }

    pub(crate) fn open_for_client() -> Result<Option<ClientService>, String> {
        let manager = unsafe { OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT) }
            .map_err(|error| format!("서비스 제어 관리자에 연결하지 못했습니다: {error}"))?;
        let manager = ScHandle(manager);
        let name = wide(SERVICE_NAME);
        let service = match unsafe {
            OpenServiceW(
                manager.0,
                PCWSTR(name.as_ptr()),
                SERVICE_START | SERVICE_QUERY_STATUS,
            )
        } {
            Ok(service) => ScHandle(service),
            Err(error) if error.code() == HRESULT::from_win32(ERROR_SERVICE_DOES_NOT_EXIST.0) => {
                return Ok(None);
            }
            Err(error) => {
                return Err(format!("SDIR 인덱스 서비스를 열지 못했습니다: {error}"));
            }
        };

        let owner_sid = read_owner_sid()?;
        let current_sid = current_process_user_sid()?;
        if !sid_equal(&owner_sid, &current_sid) {
            return Err("현재 Windows 사용자가 설치된 인덱스 서비스 승인 SID와 다릅니다.".into());
        }
        let active_console = active_console_session()?;
        let current_session = current_process_session_id()?;
        if current_session != active_console {
            return Err("인덱스 서비스는 활성 로컬 콘솔 세션에서만 사용할 수 있습니다.".into());
        }

        let expected_image = expected_service_image()?;
        let image_value = read_registry_string(SERVICE_KEY, "ImagePath")?;
        let expected_display = expected_image.to_string_lossy();
        let registered_path = expected_display
            .strip_prefix(r"\\?\")
            .unwrap_or(&expected_display);
        let expected_image_value = format!("\"{registered_path}\"");
        if !path_string_equal(&image_value, &expected_image_value) {
            return Err("SDIR 인덱스 서비스의 등록 실행 경로가 보호된 경로와 다릅니다.".into());
        }
        let service_type = read_registry_dword(SERVICE_KEY, "Type")?;
        let start_type = read_registry_dword(SERVICE_KEY, "Start")?;
        if service_type != 0x10 || start_type != u32::from(SERVICE_DEMAND_START.0) {
            return Err(
                "SDIR 인덱스 서비스 등록이 own-process 또는 demand-start 계약과 다릅니다.".into(),
            );
        }

        Ok(Some(ClientService {
            _manager: manager,
            service,
            owner_sid,
            expected_image,
        }))
    }

    impl ClientService {
        pub(crate) fn owner_sid(&self) -> &str {
            &self.owner_sid
        }

        pub(crate) fn start(
            &self,
            pipe_id: &str,
            parent_pid: u32,
            parent_created_filetime: u64,
            index_id: u64,
            memory_mib: u16,
        ) -> Result<StartedProcess, String> {
            if !valid_guid(pipe_id) || index_id == 0 || !(128..=2048).contains(&memory_mib) {
                return Err("인덱스 서비스 시작 인수가 유효하지 않습니다.".into());
            }

            let current = query_service_status(self.service.0)?;
            if current.dwCurrentState != SERVICE_STOPPED {
                return Err(format!(
                    "SDIR 인덱스 서비스가 이미 사용 중입니다 (상태 {}).",
                    current.dwCurrentState.0
                ));
            }

            let arguments = [
                "--protocol".into(),
                "1".into(),
                "--pipe-id".into(),
                pipe_id.into(),
                "--parent".into(),
                parent_pid.to_string(),
                "--parent-created-filetime".into(),
                parent_created_filetime.to_string(),
                "--index-id".into(),
                index_id.to_string(),
                "--memory-mib".into(),
                memory_mib.to_string(),
            ];
            let wide_arguments = arguments
                .iter()
                .map(|value| wide(value))
                .collect::<Vec<_>>();
            let argument_pointers = wide_arguments
                .iter()
                .map(|value| PCWSTR(value.as_ptr()))
                .collect::<Vec<_>>();
            match unsafe { StartServiceW(self.service.0, Some(&argument_pointers)) } {
                Ok(()) => {}
                Err(error)
                    if error.code() == HRESULT::from_win32(ERROR_SERVICE_ALREADY_RUNNING.0) =>
                {
                    return Err(
                        "SDIR 인덱스 서비스가 이미 실행 중인 세션을 소유하고 있습니다.".into(),
                    );
                }
                Err(error) => {
                    return Err(format!("SDIR 인덱스 서비스를 시작하지 못했습니다: {error}"));
                }
            }

            let started = Instant::now();
            let status = loop {
                let status = query_service_status(self.service.0)?;
                match status.dwCurrentState {
                    SERVICE_RUNNING => break status,
                    SERVICE_START_PENDING => {}
                    SERVICE_STOPPED => {
                        return Err(format!(
                            "SDIR 인덱스 서비스가 실행 상태에 도달하지 못했습니다 (Win32 {}, service {}).",
                            status.dwWin32ExitCode, status.dwServiceSpecificExitCode
                        ));
                    }
                    state => {
                        return Err(format!(
                            "SDIR 인덱스 서비스가 예상하지 않은 상태로 바뀌었습니다 ({}).",
                            state.0
                        ));
                    }
                }
                if started.elapsed() >= START_TIMEOUT {
                    return Err(
                        "SDIR 인덱스 서비스 시작 응답이 20초 안에 도착하지 않았습니다.".into(),
                    );
                }
                thread::sleep(Duration::from_millis(50));
            };
            if status.dwServiceType != SERVICE_WIN32_OWN_PROCESS || status.dwProcessId == 0 {
                return Err("실행 중인 서비스의 own-process PID를 확인하지 못했습니다.".into());
            }

            let handle = unsafe {
                OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                    false,
                    status.dwProcessId,
                )
            }
            .map_err(|error| {
                format!(
                    "서비스 PID {}에 query/synchronize 권한으로 연결하지 못했습니다: {error}",
                    status.dwProcessId
                )
            })?;
            let process = OwnedHandle(handle);
            let actual_pid = unsafe { GetProcessId(process.0) };
            if actual_pid != status.dwProcessId {
                return Err("SCM PID와 열린 서비스 프로세스의 PID가 다릅니다.".into());
            }
            let actual_image = process_image_path(process.0)?;
            if !canonical_path_equal(&actual_image, &self.expected_image)? {
                return Err(
                    "실행 중인 서비스 프로세스의 이미지 경로가 보호된 경로와 다릅니다.".into(),
                );
            }
            match unsafe { windows::Win32::System::Threading::WaitForSingleObject(process.0, 0) } {
                windows::Win32::Foundation::WAIT_TIMEOUT => {}
                windows::Win32::Foundation::WAIT_OBJECT_0 => {
                    return Err("서비스 프로세스가 파이프 요청 전에 종료됐습니다.".into());
                }
                windows::Win32::Foundation::WAIT_FAILED => {
                    return Err(format!(
                        "서비스 프로세스 상태를 확인하지 못했습니다: {}",
                        std::io::Error::last_os_error()
                    ));
                }
                result => {
                    return Err(format!(
                        "서비스 프로세스 상태 응답이 잘못됐습니다: {result:?}"
                    ));
                }
            }
            let handle = process.0;
            std::mem::forget(process);
            Ok(StartedProcess {
                handle,
                pid: status.dwProcessId,
            })
        }
    }

    pub(crate) fn read_owner_sid() -> Result<String, String> {
        let sid = read_registry_string(OWNER_KEY, "OwnerSid")?;
        if !valid_sid(&sid) {
            return Err(
                "HKLM\\SOFTWARE\\SDIR\\IndexService\\OwnerSid가 유효한 SID가 아닙니다.".into(),
            );
        }
        Ok(sid)
    }

    pub(crate) fn current_process_created_filetime() -> Result<u64, String> {
        let mut created = FILETIME::default();
        let mut exited = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        unsafe {
            GetProcessTimes(
                GetCurrentProcess(),
                &mut created,
                &mut exited,
                &mut kernel,
                &mut user,
            )
        }
        .map_err(|error| format!("현재 프로세스 생성 시각을 읽지 못했습니다: {error}"))?;
        Ok(filetime_value(created))
    }

    pub(crate) fn validate_parent_process(
        pid: u32,
        expected_created_filetime: u64,
        owner_sid: Option<&str>,
    ) -> Result<HANDLE, String> {
        let process = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                false,
                pid,
            )
        }
        .map_err(|error| format!("검색 요청 부모 프로세스를 열지 못했습니다: {error}"))?;
        let process = OwnedHandle(process);
        let actual_pid = unsafe { GetProcessId(process.0) };
        if actual_pid == 0 || actual_pid != pid {
            return Err("검색 요청 부모 프로세스의 PID가 바뀌었습니다.".into());
        }
        let mut created = FILETIME::default();
        let mut exited = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        unsafe { GetProcessTimes(process.0, &mut created, &mut exited, &mut kernel, &mut user) }
            .map_err(|error| format!("검색 요청 부모 생성 시각을 확인하지 못했습니다: {error}"))?;
        if filetime_value(created) != expected_created_filetime {
            return Err("검색 요청 부모의 PID와 생성 시각이 일치하지 않습니다.".into());
        }
        match unsafe { windows::Win32::System::Threading::WaitForSingleObject(process.0, 0) } {
            windows::Win32::Foundation::WAIT_TIMEOUT => {}
            windows::Win32::Foundation::WAIT_OBJECT_0 => {
                return Err("검색 요청 부모 프로세스가 이미 종료됐습니다.".into());
            }
            windows::Win32::Foundation::WAIT_FAILED => {
                return Err(format!(
                    "검색 요청 부모 프로세스 생존 상태를 확인하지 못했습니다: {}",
                    std::io::Error::last_os_error()
                ));
            }
            result => {
                return Err(format!(
                    "검색 요청 부모 생존 응답이 잘못됐습니다: {result:?}"
                ));
            }
        }
        if let Some(expected_sid) = owner_sid {
            let actual_sid = process_user_sid(process.0)?;
            if !sid_equal(expected_sid, &actual_sid) {
                return Err("검색 요청 부모의 TokenUser가 설치된 OwnerSid와 다릅니다.".into());
            }
            let session = process_session_id(process.0)?;
            let active_console = active_console_session()?;
            if session != active_console {
                return Err("검색 요청 부모가 활성 로컬 콘솔 세션에 있지 않습니다.".into());
            }
        }
        let handle = process.0;
        std::mem::forget(process);
        Ok(handle)
    }

    pub(crate) fn validate_service_identity() -> Result<(), String> {
        let expected = expected_service_image()?;
        let actual = std::env::current_exe().map_err(|error| error.to_string())?;
        if !canonical_path_equal(&actual, &expected)? {
            return Err("서비스 실행 이미지가 보호된 Program Files 경로와 다릅니다.".into());
        }
        if !sid_equal(
            &process_user_sid(unsafe { GetCurrentProcess() })?,
            "S-1-5-18",
        ) {
            return Err("SDIR 인덱스 서비스는 LocalSystem 토큰으로 실행해야 합니다.".into());
        }
        let _ = read_owner_sid()?;
        Ok(())
    }

    fn allow_owner_to_observe_service(owner_sid: &str) -> Result<(), String> {
        use windows::Win32::Security::{
            Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW,
            DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, SetKernelObjectSecurity,
        };
        // SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION permits waiting and
        // identity checks, without process termination or handle duplication.
        let sddl = wide(&format!(
            "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;0x101000;;;{owner_sid})"
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
        .map_err(|error| format!("서비스 관찰 ACL을 만들지 못했습니다: {error}"))?;
        let result = unsafe {
            SetKernelObjectSecurity(GetCurrentProcess(), DACL_SECURITY_INFORMATION, descriptor)
        };
        unsafe {
            let _ = windows::Win32::Foundation::LocalFree(Some(HLOCAL(descriptor.0)));
        }
        result.map_err(|error| format!("서비스 관찰 권한을 적용하지 못했습니다: {error}"))
    }

    pub(crate) fn request_service_stop() {
        crate::live_search::request_service_stop();
    }

    pub fn dispatch() -> Result<(), String> {
        let mut name = wide(SERVICE_NAME);
        let table = [
            SERVICE_TABLE_ENTRYW {
                lpServiceName: PWSTR(name.as_mut_ptr()),
                lpServiceProc: Some(service_main),
            },
            SERVICE_TABLE_ENTRYW::default(),
        ];
        unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) }
            .map_err(|error| format!("SCM 서비스 디스패처로 진입하지 못했습니다: {error}"))
    }

    unsafe extern "system" fn service_main(argc: u32, argv: *mut PWSTR) {
        crate::live_search::reset_service_stop();
        let name = wide(SERVICE_NAME);
        let status_handle = match unsafe {
            RegisterServiceCtrlHandlerExW(
                PCWSTR(name.as_ptr()),
                Some(service_control_handler),
                None,
            )
        } {
            Ok(handle) => handle,
            Err(_) => return,
        };
        SERVICE_STATUS_HANDLE_PTR.store(status_handle.0, Ordering::Release);

        if report_status(status_handle, SERVICE_START_PENDING, 0, 0, 1, 15_000).is_err() {
            finish_service(status_handle, 1);
            return;
        }
        let result = (|| {
            let arguments = parse_start_arguments(argc, argv)?;
            validate_service_identity()?;
            let owner_sid = read_owner_sid()?;
            let parent = validate_parent_process(
                arguments.parent_pid,
                arguments.parent_created_filetime,
                Some(&owner_sid),
            )?;
            unsafe {
                let _ = CloseHandle(parent);
            }
            allow_owner_to_observe_service(&owner_sid)?;
            report_status(status_handle, SERVICE_RUNNING, 0, 0, 0, 0)?;
            crate::live_search::service_host(
                &arguments.pipe_id,
                arguments.parent_pid,
                arguments.parent_created_filetime,
                arguments.index_id,
                arguments.memory_mib,
            )
        })();

        match result {
            Ok(()) => finish_service(status_handle, 0),
            Err(_) => finish_service(status_handle, 1),
        }
        SERVICE_STATUS_HANDLE_PTR.store(ptr::null_mut(), Ordering::Release);
    }

    unsafe extern "system" fn service_control_handler(
        control: u32,
        _event_type: u32,
        _event_data: *mut c_void,
        _context: *mut c_void,
    ) -> u32 {
        if control == SERVICE_CONTROL_STOP {
            request_service_stop();
            let raw = SERVICE_STATUS_HANDLE_PTR.load(Ordering::Acquire);
            if !raw.is_null() {
                let status_handle = SERVICE_STATUS_HANDLE(raw);
                let _ = report_status(status_handle, SERVICE_STOP_PENDING, 0, 0, 1, 10_000);
            }
            ERROR_SUCCESS.0
        } else if control == SERVICE_CONTROL_INTERROGATE {
            ERROR_SUCCESS.0
        } else {
            ERROR_CALL_NOT_IMPLEMENTED.0
        }
    }

    fn finish_service(status_handle: SERVICE_STATUS_HANDLE, service_exit: u32) {
        let (win32_exit, specific_exit) = if service_exit == 0 {
            (0, 0)
        } else {
            (ERROR_SERVICE_SPECIFIC_ERROR.0, service_exit)
        };
        let _ = report_status(
            status_handle,
            SERVICE_STOPPED,
            win32_exit,
            specific_exit,
            0,
            0,
        );
    }

    fn report_status(
        handle: SERVICE_STATUS_HANDLE,
        state: windows::Win32::System::Services::SERVICE_STATUS_CURRENT_STATE,
        win32_exit: u32,
        service_exit: u32,
        checkpoint: u32,
        wait_hint: u32,
    ) -> Result<(), String> {
        let status = SERVICE_STATUS {
            dwServiceType: SERVICE_WIN32_OWN_PROCESS,
            dwCurrentState: state,
            dwControlsAccepted: if state == SERVICE_RUNNING {
                SERVICE_ACCEPT_STOP
            } else {
                0
            },
            dwWin32ExitCode: win32_exit,
            dwServiceSpecificExitCode: service_exit,
            dwCheckPoint: checkpoint,
            dwWaitHint: wait_hint,
        };
        unsafe { SetServiceStatus(handle, &status) }
            .map_err(|error| format!("SCM 서비스 상태를 갱신하지 못했습니다: {error}"))
    }

    fn parse_start_arguments(
        argc: u32,
        argv: *mut PWSTR,
    ) -> Result<StartArgumentsWithPipe, String> {
        if argc as usize != MAX_SERVICE_ARGUMENTS || argv.is_null() {
            return Err("SCM 서비스 시작 인수의 개수가 올바르지 않습니다.".into());
        }
        let argv = unsafe { std::slice::from_raw_parts(argv, argc as usize) };
        let values = argv
            .iter()
            .map(|value| wide_argument(*value))
            .collect::<Result<Vec<_>, _>>()?;
        let expected_names = [
            SERVICE_NAME,
            "--protocol",
            "1",
            "--pipe-id",
            "",
            "--parent",
            "",
            "--parent-created-filetime",
            "",
            "--index-id",
            "",
            "--memory-mib",
            "",
        ];
        for (index, expected) in expected_names.iter().enumerate() {
            if !expected.is_empty() && values[index] != *expected {
                return Err(
                    "SCM 서비스 시작 인수 순서 또는 프로토콜 버전이 올바르지 않습니다.".into(),
                );
            }
        }
        let pipe_id = values[4].clone();
        if !valid_guid(&pipe_id) {
            return Err("SCM에서 전달된 pipe GUID가 올바르지 않습니다.".into());
        }
        let parent_pid = parse_decimal::<u32>(&values[6], "부모 PID")?;
        let parent_created_filetime = parse_decimal::<u64>(&values[8], "부모 생성 시각")?;
        let index_id = parse_decimal::<u64>(&values[10], "색인 ID")?;
        let memory_mib = parse_decimal::<u16>(&values[12], "메모리 상한")?;
        if parent_pid == 0 || parent_created_filetime == 0 || index_id == 0 {
            return Err("SCM 서비스 시작 인수에 0인 식별자가 있습니다.".into());
        }
        if !(128..=2048).contains(&memory_mib) {
            return Err("SCM 서비스 메모리 상한은 128~2048 MiB여야 합니다.".into());
        }
        Ok(StartArgumentsWithPipe {
            pipe_id,
            parent_pid,
            parent_created_filetime,
            index_id,
            memory_mib,
        })
    }

    struct StartArgumentsWithPipe {
        pipe_id: String,
        parent_pid: u32,
        parent_created_filetime: u64,
        index_id: u64,
        memory_mib: u16,
    }

    fn query_service_status(service: SC_HANDLE) -> Result<SERVICE_STATUS_PROCESS, String> {
        let mut status = SERVICE_STATUS_PROCESS::default();
        let bytes = unsafe {
            std::slice::from_raw_parts_mut(
                (&mut status as *mut SERVICE_STATUS_PROCESS).cast::<u8>(),
                size_of::<SERVICE_STATUS_PROCESS>(),
            )
        };
        let mut returned = 0u32;
        unsafe {
            QueryServiceStatusEx(service, SC_STATUS_PROCESS_INFO, Some(bytes), &mut returned)
        }
        .map_err(|error| format!("SCM 서비스 상태를 조회하지 못했습니다: {error}"))?;
        Ok(status)
    }

    pub(crate) fn prepare_resume_store(
        verified_owner_sid: &str,
    ) -> Result<crate::index_cache::CacheStore, String> {
        validate_service_identity()?;
        if !sid_equal(verified_owner_sid, &read_owner_sid()?) {
            return Err("색인 저장소의 등록 SID가 검증된 서비스 사용자와 다릅니다.".into());
        }
        let image = service_image_path()?;
        crate::index_cache::CacheStore::open_service_at(&image, verified_owner_sid)
            .map_err(|error| error.to_string())
    }

    fn service_image_path() -> Result<PathBuf, String> {
        use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
        use windows::Win32::System::Com::{
            COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize,
        };
        let initialization = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if initialization.is_err() && initialization != RPC_E_CHANGED_MODE {
            return Err(format!(
                "Program Files 경로 조회용 COM 초기화 실패: {initialization:?}"
            ));
        }
        let base = (|| {
            let path =
                unsafe { SHGetKnownFolderPath(&FOLDERID_ProgramFiles, KNOWN_FOLDER_FLAG(0), None) }
                    .map_err(|error| {
                        format!("Program Files 경로를 확인하지 못했습니다: {error}")
                    })?;
            let base = unsafe { path.to_string() };
            unsafe { CoTaskMemFree(Some(path.0.cast())) };
            base.map_err(|error| format!("Program Files 경로를 해독하지 못했습니다: {error}"))
        })();
        if initialization.is_ok() {
            unsafe { CoUninitialize() };
        }
        Ok(PathBuf::from(base?).join(SERVICE_FOLDER).join(SERVICE_EXE))
    }

    fn expected_service_image() -> Result<PathBuf, String> {
        use std::os::windows::fs::MetadataExt;
        let image = service_image_path()?;
        for component in image.ancestors().take(3) {
            let metadata = std::fs::symlink_metadata(component)
                .map_err(|error| format!("서비스 보호 경로 정보를 읽지 못했습니다: {error}"))?;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err("서비스 보호 경로의 재분석 지점을 사용할 수 없습니다.".into());
            }
        }
        std::fs::canonicalize(&image)
            .map_err(|error| format!("보호된 서비스 실행 파일 경로를 확인하지 못했습니다: {error}"))
    }

    fn process_image_path(process: HANDLE) -> Result<PathBuf, String> {
        let mut buffer = vec![0u16; 32_768];
        let mut length = buffer.len() as u32;
        unsafe {
            QueryFullProcessImageNameW(
                process,
                windows::Win32::System::Threading::PROCESS_NAME_FORMAT(0),
                PWSTR(buffer.as_mut_ptr()),
                &mut length,
            )
        }
        .map_err(|error| format!("서비스 프로세스 이미지 경로를 읽지 못했습니다: {error}"))?;
        let path = std::ffi::OsString::from_wide(&buffer[..length as usize]);
        Ok(PathBuf::from(path))
    }

    fn canonical_path_equal(left: &Path, right: &Path) -> Result<bool, String> {
        let left = std::fs::canonicalize(left)
            .map_err(|error| format!("실행 경로를 정규화하지 못했습니다: {error}"))?;
        let right = std::fs::canonicalize(right)
            .map_err(|error| format!("보호 경로를 정규화하지 못했습니다: {error}"))?;
        Ok(path_string_equal(
            &left.to_string_lossy(),
            &right.to_string_lossy(),
        ))
    }

    fn path_string_equal(left: &str, right: &str) -> bool {
        let left = left.replace('/', "\\");
        let right = right.replace('/', "\\");
        left.eq_ignore_ascii_case(&right)
    }

    fn read_registry_string(key_path_text: &str, value_name: &str) -> Result<String, String> {
        let key_path = wide(key_path_text);
        let mut key = HKEY::default();
        let status = unsafe {
            RegOpenKeyExW(
                HKEY_LOCAL_MACHINE,
                PCWSTR(key_path.as_ptr()),
                Some(0),
                KEY_QUERY_VALUE | KEY_WOW64_64KEY,
                &mut key,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(format!("HKLM 등록 키를 열지 못했습니다 ({}).", status.0));
        }
        let key = RegistryKey(key);
        let is_image_path = key_path_text == SERVICE_KEY && value_name == "ImagePath";
        let value_name = wide(value_name);
        let mut value_type = windows::Win32::System::Registry::REG_VALUE_TYPE(0);
        let mut byte_length = 0u32;
        let status = unsafe {
            RegQueryValueExW(
                key.0,
                PCWSTR(value_name.as_ptr()),
                None,
                Some(&mut value_type),
                None,
                Some(&mut byte_length),
            )
        };
        if status != ERROR_SUCCESS
            || !(value_type == REG_SZ || (is_image_path && value_type == REG_EXPAND_SZ))
        {
            return Err(format!(
                "HKLM 문자열 설정을 읽지 못했습니다 ({}).",
                status.0
            ));
        }
        if byte_length < 2 || byte_length > MAX_REGISTRY_VALUE_BYTES || byte_length % 2 != 0 {
            return Err("HKLM 문자열 설정의 크기 또는 UTF-16 길이가 잘못됐습니다.".into());
        }
        let mut data = vec![0u16; byte_length as usize / 2];
        let status = unsafe {
            RegQueryValueExW(
                key.0,
                PCWSTR(value_name.as_ptr()),
                None,
                Some(&mut value_type),
                Some(data.as_mut_ptr().cast::<u8>()),
                Some(&mut byte_length),
            )
        };
        if status != ERROR_SUCCESS
            || !(value_type == REG_SZ || (is_image_path && value_type == REG_EXPAND_SZ))
            || byte_length != (data.len() * size_of::<u16>()) as u32
        {
            return Err(format!(
                "HKLM 문자열 설정을 다시 읽지 못했습니다 ({}).",
                status.0
            ));
        }
        if data.last() != Some(&0) || data[..data.len() - 1].contains(&0) {
            return Err("HKLM 문자열 설정의 종료 문자가 잘못됐습니다.".into());
        }
        String::from_utf16(&data[..data.len() - 1])
            .map_err(|error| format!("HKLM 문자열 설정의 UTF-16이 잘못됐습니다: {error}"))
    }

    fn read_registry_dword(key_path: &str, value_name: &str) -> Result<u32, String> {
        let key_path = wide(key_path);
        let mut key = HKEY::default();
        let status = unsafe {
            RegOpenKeyExW(
                HKEY_LOCAL_MACHINE,
                PCWSTR(key_path.as_ptr()),
                Some(0),
                KEY_QUERY_VALUE | KEY_WOW64_64KEY,
                &mut key,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(format!("HKLM 등록 키를 열지 못했습니다 ({}).", status.0));
        }
        let key = RegistryKey(key);
        let value_name = wide(value_name);
        let mut value_type = windows::Win32::System::Registry::REG_VALUE_TYPE(0);
        let mut value = 0u32;
        let mut byte_length = size_of::<u32>() as u32;
        let status = unsafe {
            RegQueryValueExW(
                key.0,
                PCWSTR(value_name.as_ptr()),
                None,
                Some(&mut value_type),
                Some((&mut value as *mut u32).cast::<u8>()),
                Some(&mut byte_length),
            )
        };
        if status != ERROR_SUCCESS || value_type != REG_DWORD || byte_length != 4 {
            return Err(format!(
                "HKLM DWORD 설정이 없거나 잘못됐습니다 ({}).",
                status.0
            ));
        }
        Ok(value)
    }

    fn process_user_sid(process: HANDLE) -> Result<String, String> {
        let mut token = HANDLE::default();
        unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) }
            .map_err(|error| format!("프로세스 토큰을 열지 못했습니다: {error}"))?;
        let token = OwnedHandle(token);
        let mut required = 0u32;
        let _ = unsafe { GetTokenInformation(token.0, TokenUser, None, 0, &mut required) };
        if required < size_of::<TOKEN_USER>() as u32 || required > 64 * 1024 {
            return Err("프로세스 TokenUser 크기가 올바르지 않습니다.".into());
        }
        let mut storage = vec![0usize; (required as usize).div_ceil(size_of::<usize>())];
        unsafe {
            GetTokenInformation(
                token.0,
                TokenUser,
                Some(storage.as_mut_ptr().cast::<c_void>()),
                required,
                &mut required,
            )
        }
        .map_err(|error| format!("프로세스 TokenUser를 읽지 못했습니다: {error}"))?;
        let user = unsafe { &*storage.as_ptr().cast::<TOKEN_USER>() };
        let mut text = PWSTR::null();
        unsafe { ConvertSidToStringSidW(user.User.Sid, &mut text) }
            .map_err(|error| format!("프로세스 SID를 문자열로 변환하지 못했습니다: {error}"))?;
        let result = unsafe { text.to_string() }
            .map_err(|error| format!("프로세스 SID 문자열이 잘못됐습니다: {error}"));
        unsafe {
            let _ = windows::Win32::Foundation::LocalFree(Some(HLOCAL(text.0.cast())));
        }
        result
    }

    fn process_session_id(process: HANDLE) -> Result<u32, String> {
        let mut token = HANDLE::default();
        unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) }
            .map_err(|error| format!("프로세스 토큰을 열지 못했습니다: {error}"))?;
        let token = OwnedHandle(token);
        let mut session = 0u32;
        let mut returned = 0u32;
        unsafe {
            GetTokenInformation(
                token.0,
                TokenSessionId,
                Some((&mut session as *mut u32).cast::<c_void>()),
                size_of::<u32>() as u32,
                &mut returned,
            )
        }
        .map_err(|error| format!("프로세스 TokenSessionId를 읽지 못했습니다: {error}"))?;
        if returned != size_of::<u32>() as u32 {
            return Err("프로세스 TokenSessionId 크기가 잘못됐습니다.".into());
        }
        Ok(session)
    }

    fn current_process_user_sid() -> Result<String, String> {
        process_user_sid(unsafe { GetCurrentProcess() })
    }

    fn current_process_session_id() -> Result<u32, String> {
        process_session_id(unsafe { GetCurrentProcess() })
    }

    fn active_console_session() -> Result<u32, String> {
        let session = unsafe { WTSGetActiveConsoleSessionId() };
        if session == u32::MAX {
            Err("활성 로컬 콘솔 세션이 없습니다.".into())
        } else {
            Ok(session)
        }
    }

    fn valid_sid(value: &str) -> bool {
        let mut parts = value.split('-');
        if parts.next() != Some("S") || parts.next() != Some("1") {
            return false;
        }
        let rest = parts.collect::<Vec<_>>();
        rest.len() >= 2
            && rest
                .iter()
                .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
    }

    fn sid_equal(left: &str, right: &str) -> bool {
        left.eq_ignore_ascii_case(right)
    }

    fn valid_guid(value: &str) -> bool {
        value.len() == 36
            && value.bytes().enumerate().all(|(index, byte)| match index {
                8 | 13 | 18 | 23 => byte == b'-',
                _ => byte.is_ascii_hexdigit(),
            })
    }

    fn parse_decimal<T: std::str::FromStr>(value: &str, label: &str) -> Result<T, String> {
        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(format!("SCM 서비스 {label} 인수가 숫자가 아닙니다."));
        }
        value
            .parse()
            .map_err(|_| format!("SCM 서비스 {label} 인수의 범위가 잘못됐습니다."))
    }

    fn filetime_value(value: FILETIME) -> u64 {
        (u64::from(value.dwHighDateTime) << 32) | u64::from(value.dwLowDateTime)
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }

    fn wide_argument(value: PWSTR) -> Result<String, String> {
        if value.0.is_null() {
            return Err("SCM 서비스 인수에 null 포인터가 있습니다.".into());
        }
        let mut length = 0usize;
        while length < 32_768 {
            if unsafe { *value.0.add(length) } == 0 {
                return String::from_utf16(unsafe { std::slice::from_raw_parts(value.0, length) })
                    .map_err(|error| format!("SCM 서비스 인수가 UTF-16이 아닙니다: {error}"));
            }
            length += 1;
        }
        Err("SCM 서비스 인수가 허용 길이를 넘었습니다.".into())
    }
}

#[cfg(windows)]
pub use windows_service::dispatch;

#[cfg(windows)]
pub(crate) use windows_service::{
    current_process_created_filetime, open_for_client, prepare_resume_store, read_owner_sid,
    validate_parent_process, validate_service_identity,
};

#[cfg(not(windows))]
pub fn dispatch() -> Result<(), String> {
    Err("SDIR 인덱스 서비스는 Windows 전용입니다.".into())
}
