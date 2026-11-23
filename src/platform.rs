use crate::{
    filesystem::validate_action,
    history::{self, Change},
    model::{FileAction, JobResult, NativePath},
};

pub fn run_action(action: &FileAction) -> JobResult {
    run_action_with_progress(action, |_| {})
}

pub(crate) fn run_action_with_progress<F>(action: &FileAction, progress: F) -> JobResult
where
    F: Fn(u8) + Send + Sync + 'static,
{
    if let Err(message) = validate_action(action) {
        return JobResult {
            ok: false,
            cancelled: false,
            message,
            destinations: vec![],
            undo: None,
            history_note: None,
            outcomes: action
                .sources()
                .into_iter()
                .map(|source| crate::model::ItemOutcome {
                    source,
                    ok: false,
                    message: "실행 전 검증 실패".into(),
                })
                .collect(),
        };
    }
    #[cfg(windows)]
    let (result, destinations, changes, mut outcomes) =
        windows_impl::perform(action, std::sync::Arc::new(progress));
    #[cfg(not(windows))]
    let _ = progress;
    #[cfg(not(windows))]
    let (result, destinations, changes, mut outcomes): (
        Result<bool, String>,
        Vec<NativePath>,
        Vec<Change>,
        Vec<crate::model::ItemOutcome>,
    ) = (
        Err("파일 작업은 Windows 빌드에서 지원합니다.".into()),
        vec![],
        vec![],
        vec![],
    );
    let (mut ok, cancelled, mut message) = match result {
        Ok(false) => (true, false, "파일 작업을 완료했습니다.".into()),
        Ok(true) => (
            false,
            true,
            "작업이 취소됐습니다. 일부 항목은 처리됐을 수 있습니다.".into(),
        ),
        Err(message) => (
            false,
            false,
            format!("작업을 완료하지 못했습니다. 일부 항목은 처리됐을 수 있습니다. {message}"),
        ),
    };
    for source in action.sources() {
        if !outcomes.iter().any(|o| o.source == source) {
            outcomes.push(crate::model::ItemOutcome {
                source,
                ok: false,
                message: "미처리 또는 건너뜀".into(),
            });
        }
    }
    if !ok {
        for o in &mut outcomes {
            if o.ok && o.source.to_path().is_dir() {
                o.ok = false;
                o.message = "폴더 일부 처리 가능 · 결과 확인 후 재시도".into();
            }
        }
    }
    let mut combined: Vec<crate::model::ItemOutcome> = vec![];
    for outcome in outcomes {
        if let Some(old) = combined.iter_mut().find(|o| o.source == outcome.source) {
            if !outcome.ok {
                *old = outcome;
            }
        } else {
            combined.push(outcome);
        }
    }
    let outcomes = combined;
    let failed = outcomes.iter().filter(|o| !o.ok).count();
    if failed > 0 {
        ok = false;
        message = format!(
            "완료 {}개 · 미완료 {}개. {}",
            outcomes.len() - failed,
            failed,
            message
        );
    }
    if ok {
        let completed = outcomes.iter().filter(|outcome| outcome.ok).count();
        match action {
            FileAction::Copy { destination, .. } => {
                message = format!(
                    "파일 작업을 완료했습니다. 복사 {completed}개 → {}",
                    destination.display()
                );
            }
            FileAction::Move { destination, .. } => {
                message = format!(
                    "파일 작업을 완료했습니다. 이동 {completed}개 → {}",
                    destination.display()
                );
            }
            _ => {}
        }
    }
    let (undo, history_note) = match history::inverse(changes) {
        Ok(record) => (record, None),
        Err(note) => (None, Some(note)),
    };
    JobResult {
        ok,
        cancelled,
        message,
        destinations,
        undo,
        history_note,
        outcomes,
    }
}

pub fn open_path(path: &NativePath) -> Result<(), String> {
    #[cfg(windows)]
    {
        windows_impl::open(path)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err("Windows 전용 기능입니다.".into())
    }
}
pub fn clipboard_set(paths: &[NativePath], cut: bool) -> Result<(), String> {
    #[cfg(windows)]
    {
        windows_impl::clipboard_set(paths, cut)
    }
    #[cfg(not(windows))]
    {
        let _ = (paths, cut);
        Err("Windows 전용 기능입니다.".into())
    }
}
pub fn clipboard_get() -> Result<(Vec<NativePath>, bool), String> {
    #[cfg(windows)]
    {
        windows_impl::clipboard_get()
    }
    #[cfg(not(windows))]
    {
        Err("Windows 전용 기능입니다.".into())
    }
}
/// A nonblocking sequence check invalidates visual marks after another app replaces the clipboard.
pub fn clipboard_sequence() -> u32 {
    #[cfg(windows)]
    unsafe {
        windows::Win32::System::DataExchange::GetClipboardSequenceNumber()
    }
    #[cfg(not(windows))]
    {
        0
    }
}
#[cfg(windows)]
pub(crate) unsafe fn wait_shell_windows() {
    unsafe {
        use windows::Win32::{
            Foundation::{HWND, LPARAM},
            System::Threading::GetCurrentProcessId,
            UI::WindowsAndMessaging::{
                DispatchMessageW, EnumWindows, GetWindowThreadProcessId, IsWindowVisible, MSG,
                PM_REMOVE, PeekMessageW, TranslateMessage, WM_QUIT,
            },
        };
        unsafe extern "system" fn count_window(hwnd: HWND, param: LPARAM) -> windows::core::BOOL {
            unsafe {
                let mut pid = 0;
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
                if pid == GetCurrentProcessId() && IsWindowVisible(hwnd).as_bool() {
                    *(param.0 as *mut usize) += 1;
                }
                windows::core::BOOL(1)
            }
        }
        let start = std::time::Instant::now();
        let mut seen = false;
        loop {
            let mut msg = MSG::default();
            let mut quit = false;
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    quit = true;
                    break;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            let mut count = 0usize;
            let _ = EnumWindows(
                Some(count_window),
                LPARAM((&mut count as *mut usize) as isize),
            );
            seen |= count > 0;
            if quit
                || (seen && count == 0)
                || (!seen && start.elapsed() > std::time::Duration::from_secs(3))
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

pub fn properties(path: &NativePath) -> Result<(), String> {
    #[cfg(windows)]
    unsafe {
        use windows::{
            Win32::{
                System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize},
                UI::Shell::{SHOP_FILEPATH, SHObjectProperties},
            },
            core::PCWSTR,
        };
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(|e| e.to_string())?;
        let mut value = path.0.clone();
        value.push(0);
        let ok = SHObjectProperties(None, SHOP_FILEPATH, PCWSTR(value.as_ptr()), PCWSTR::null())
            .as_bool();
        if ok {
            wait_shell_windows();
        }
        CoUninitialize();
        if ok {
            Ok(())
        } else {
            Err("속성 창을 열지 못했습니다.".into())
        }
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err("Windows 전용 기능입니다.".into())
    }
}

pub fn navigation_locations() -> Vec<(String, NativePath)> {
    #[cfg(windows)]
    {
        use windows::Win32::{
            System::Com::CoTaskMemFree,
            UI::Shell::{
                FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, KF_FLAG_DONT_VERIFY,
                SHGetKnownFolderPath,
            },
        };
        [
            ("바탕 화면", "Desktop", FOLDERID_Desktop),
            ("다운로드", "Downloads", FOLDERID_Downloads),
            ("문서", "Documents", FOLDERID_Documents),
        ]
        .into_iter()
        .map(|(name, fallback, id)| {
            let path = unsafe { SHGetKnownFolderPath(&id, KF_FLAG_DONT_VERIFY, None) }
                .ok()
                .map(|raw| unsafe {
                    let path = NativePath(raw.as_wide().to_vec());
                    CoTaskMemFree(Some(raw.0.cast()));
                    path
                })
                .unwrap_or_else(|| {
                    NativePath::new(crate::settings::home().to_path().join(fallback))
                });
            (name.into(), path)
        })
        .collect()
    }
    #[cfg(not(windows))]
    {
        vec![]
    }
}
pub fn navigation_drives() -> Vec<NativePath> {
    #[cfg(windows)]
    {
        let mask = unsafe { windows::Win32::Storage::FileSystem::GetLogicalDrives() };
        (0..26)
            .filter(|n| mask & (1u32 << n) != 0)
            .map(|n| NativePath::new(format!("{}:\\", (b'A' + n) as char)))
            .collect()
    }
    #[cfg(not(windows))]
    {
        vec![]
    }
}

#[cfg(windows)]
mod windows_impl {
    use super::*;
    use crate::history::{ChangeKind, ItemSource, ItemTarget, RecycledItem};
    use windows::{
        Win32::{
            System::Com::{
                CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
                CoTaskMemFree, CoUninitialize,
            },
            UI::{
                Shell::{
                    COPYENGINE_E_CANCELLED, COPYENGINE_E_USER_CANCELLED, FOF_ALLOWUNDO,
                    FOF_NO_CONNECTED_ELEMENTS, FOF_NOCONFIRMATION, FOF_RENAMEONCOLLISION,
                    FOF_WANTNUKEWARNING, FOFX_ADDUNDORECORD, FOFX_RECYCLEONDELETE, FileOperation,
                    IFileOperation, IFileOperationProgressSink, IFileOperationProgressSink_Impl,
                    IShellItem, SHCreateItemFromParsingName, SICHINT_CANONICAL, SIGDN_FILESYSPATH,
                    ShellExecuteW,
                },
                WindowsAndMessaging::SW_SHOWNORMAL,
            },
        },
        core::{PCWSTR, w},
    };
    pub fn perform(
        action: &FileAction,
        progress: std::sync::Arc<dyn Fn(u8) + Send + Sync>,
    ) -> (
        Result<bool, String>,
        Vec<NativePath>,
        Vec<Change>,
        Vec<crate::model::ItemOutcome>,
    ) {
        let paths = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
        let changes = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
        let outcomes = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
        let result = perform_inner(
            action,
            paths.clone(),
            changes.clone(),
            outcomes.clone(),
            progress,
        );
        let destinations = paths.lock().unwrap().clone();
        let completed = changes.lock().unwrap().clone();
        let outcomes = outcomes.lock().unwrap().clone();
        (result, destinations, completed, outcomes)
    }
    fn result_parents(action: &FileAction) -> Vec<NativePath> {
        match action {
            FileAction::Copy { destination, .. }
            | FileAction::Move { destination, .. }
            | FileAction::NewFolder { destination, .. } => vec![destination.clone()],
            FileAction::Rename { source, .. } => source.parent().into_iter().collect(),
            FileAction::BatchRename { pairs } => {
                pairs.iter().filter_map(|p| p.source.parent()).collect()
            }
            FileAction::Recycle { .. } | FileAction::Delete { .. } => vec![],
            FileAction::History { record } => record
                .items
                .iter()
                .filter_map(|i| {
                    if let ItemTarget::Path(p) = &i.target {
                        p.parent()
                    } else {
                        None
                    }
                })
                .collect(),
        }
    }
    fn action_sources(action: &FileAction) -> Vec<ItemSource> {
        match action {
            FileAction::Copy { sources, .. }
            | FileAction::Move { sources, .. }
            | FileAction::Recycle { sources }
            | FileAction::Delete { sources } => {
                sources.iter().cloned().map(ItemSource::File).collect()
            }
            FileAction::Rename { source, .. } => vec![ItemSource::File(source.clone())],
            FileAction::BatchRename { pairs } => pairs
                .iter()
                .map(|p| ItemSource::File(p.source.clone()))
                .collect(),
            FileAction::History { record } => {
                record.items.iter().map(|i| i.source.clone()).collect()
            }
            _ => vec![],
        }
    }
    fn expected_targets(action: &FileAction) -> Vec<NativePath> {
        match action {
            FileAction::Copy {
                sources,
                destination,
            }
            | FileAction::Move {
                sources,
                destination,
            } => sources
                .iter()
                .map(|p| NativePath::new(destination.to_path().join(p.name())))
                .collect(),
            FileAction::BatchRename { pairs } => pairs
                .iter()
                .filter_map(|p| {
                    p.source
                        .parent()
                        .map(|parent| NativePath::new(parent.to_path().join(&p.name)))
                })
                .collect(),
            FileAction::Rename { source, name } => source
                .parent()
                .map(|p| NativePath::new(p.to_path().join(name)))
                .into_iter()
                .collect(),
            FileAction::NewFolder { destination, name } => {
                vec![NativePath::new(destination.to_path().join(name))]
            }
            FileAction::History { record } => record
                .items
                .iter()
                .filter_map(|i| {
                    if let ItemTarget::Path(p) = &i.target {
                        Some(p.clone())
                    } else {
                        None
                    }
                })
                .collect(),
            _ => vec![],
        }
    }
    unsafe fn native_path(item: &IShellItem) -> windows::core::Result<NativePath> {
        unsafe {
            let name = item.GetDisplayName(SIGDN_FILESYSPATH)?;
            let path = NativePath(name.as_wide().to_vec());
            CoTaskMemFree(Some(name.0.cast()));
            Ok(path)
        }
    }
    fn source_item(source: &ItemSource) -> windows::core::Result<IShellItem> {
        match source {
            ItemSource::File(p) => item(p),
            ItemSource::Recycle(r) => {
                if !history::valid_pidl(&r.pidl) {
                    return Err(windows::core::Error::from(windows::core::HRESULT(
                        0x80070057u32 as i32,
                    )));
                }
                let mut aligned = vec![0u16; r.pidl.len().div_ceil(2)];
                for (i, b) in r.pidl.iter().enumerate() {
                    aligned[i / 2] |= u16::from(*b) << ((i % 2) * 8);
                }
                unsafe {
                    windows::Win32::UI::Shell::SHCreateItemFromIDList(aligned.as_ptr().cast())
                }
            }
        }
    }
    #[windows::core::implement(IFileOperationProgressSink)]
    struct OperationSink {
        paths: std::sync::Arc<std::sync::Mutex<Vec<NativePath>>>,
        changes: std::sync::Arc<std::sync::Mutex<Vec<Change>>>,
        parents: Vec<(NativePath, IShellItem)>,
        sources: Vec<(ItemSource, IShellItem)>,
        existed: Vec<IShellItem>,
        outcomes: std::sync::Arc<std::sync::Mutex<Vec<crate::model::ItemOutcome>>>,
        progress: std::sync::Arc<dyn Fn(u8) + Send + Sync>,
        last_progress: std::sync::Mutex<Option<u8>>,
    }
    fn completed_status(result: windows::core::HRESULT) -> bool {
        matches!(
            result.0 as u32,
            0 | 0x00270001 | 0x00270006 | 0x00270008 | 0x0027000A | 0x0027000C | 0x0027000E
        )
    }
    impl OperationSink {
        fn outcome(
            &self,
            old: &windows::core::Ref<IShellItem>,
            ok: bool,
            result: windows::core::HRESULT,
        ) {
            let source = self.source(old).or_else(|| {
                if ok {
                    return None;
                }
                let mut current = old.as_ref().cloned();
                for _ in 0..129 {
                    let item = current?;
                    if let Some((source, _)) = self.sources.iter().find(|(_, p)| unsafe {
                        p.Compare(&item, SICHINT_CANONICAL.0 as u32)
                            .is_ok_and(|n| n == 0)
                    }) {
                        return Some(source.clone());
                    }
                    current = unsafe { item.GetParent().ok() };
                }
                None
            });
            if let Some(source) = source {
                self.outcomes
                    .lock()
                    .unwrap()
                    .push(crate::model::ItemOutcome {
                        source: source.path().clone(),
                        ok,
                        message: if ok {
                            "완료".into()
                        } else {
                            format!("미완료 · HRESULT 0x{:08X}", result.0 as u32)
                        },
                    });
            }
        }

        fn source(&self, item: &windows::core::Ref<IShellItem>) -> Option<ItemSource> {
            let item = item.as_ref()?;
            self.sources
                .iter()
                .find(|(_, s)| unsafe {
                    s.Compare(item, SICHINT_CANONICAL.0 as u32)
                        .is_ok_and(|n| n == 0)
                })
                .map(|(p, _)| p.clone())
        }
        fn record(
            &self,
            result: windows::core::HRESULT,
            item: windows::core::Ref<IShellItem>,
        ) -> Option<NativePath> {
            if !completed_status(result) {
                return None;
            }
            let item = item.as_ref()?;
            unsafe {
                let path = native_path(item).ok()?;
                let actual_parent = item.GetParent().ok()?;
                let (parent, _) = self.parents.iter().find(|(_, p)| {
                    p.Compare(&actual_parent, SICHINT_CANONICAL.0 as u32)
                        .is_ok_and(|n| n == 0)
                })?;
                let resolved = path.to_path();
                let path = NativePath::new(parent.to_path().join(resolved.file_name()?));
                let mut paths = self.paths.lock().unwrap();
                if !paths.contains(&path) {
                    paths.push(path.clone());
                }
                Some(path)
            }
        }
        fn completed(
            &self,
            kind: ChangeKind,
            result: windows::core::HRESULT,
            old: windows::core::Ref<IShellItem>,
            new: windows::core::Ref<IShellItem>,
        ) {
            self.outcome(
                &old,
                completed_status(result) && new.as_ref().is_some(),
                result,
            );
            let overwritten = new.as_ref().is_some_and(|item| {
                self.existed.iter().any(|old| unsafe {
                    old.Compare(item, SICHINT_CANONICAL.0 as u32)
                        .is_ok_and(|n| n == 0)
                })
            });
            if let Some(destination) = self.record(result, new) {
                let source = self.source(&old);
                let kind = if matches!(kind, ChangeKind::Move)
                    && matches!(source, Some(ItemSource::Recycle(_)))
                {
                    ChangeKind::Restore
                } else {
                    kind
                };
                self.changes.lock().unwrap().push(Change {
                    kind,
                    source,
                    destination: Some(destination),
                    recycled: None,
                    overwritten,
                });
            }
        }
        fn recycled(
            &self,
            result: windows::core::HRESULT,
            old: windows::core::Ref<IShellItem>,
            new: windows::core::Ref<IShellItem>,
        ) {
            self.outcome(&old, completed_status(result), result);
            if !completed_status(result) {
                return;
            }
            let Some(source) = self.source(&old) else {
                return;
            };
            let recycled = new.as_ref().and_then(|item| unsafe {
                let path = native_path(item).ok()?;
                let pidl = windows::Win32::UI::Shell::SHGetIDListFromObject(item).ok()?;
                let size = windows::Win32::UI::Shell::ILGetSize(Some(pidl));
                let bytes = if (2..=65536).contains(&size) {
                    Some(std::slice::from_raw_parts(pidl.cast::<u8>(), size as usize).to_vec())
                } else {
                    None
                };
                CoTaskMemFree(Some(pidl.cast()));
                Some(RecycledItem { path, pidl: bytes? })
            });
            self.changes.lock().unwrap().push(Change {
                kind: ChangeKind::Recycle,
                source: Some(source),
                destination: None,
                recycled,
                overwritten: false,
            });
        }
    }
    #[allow(non_snake_case, unused_variables)]
    impl IFileOperationProgressSink_Impl for OperationSink_Impl {
        fn StartOperations(&self) -> windows::core::Result<()> {
            Ok(())
        }
        fn FinishOperations(&self, hrresult: windows::core::HRESULT) -> windows::core::Result<()> {
            Ok(())
        }
        fn PreRenameItem(
            &self,
            dwflags: u32,
            psiitem: windows::core::Ref<IShellItem>,
            psznewname: &windows::core::PCWSTR,
        ) -> windows::core::Result<()> {
            Ok(())
        }
        fn PostRenameItem(
            &self,
            dwflags: u32,
            psiitem: windows::core::Ref<IShellItem>,
            psznewname: &windows::core::PCWSTR,
            hrrename: windows::core::HRESULT,
            psinewlycreated: windows::core::Ref<IShellItem>,
        ) -> windows::core::Result<()> {
            self.completed(ChangeKind::Move, hrrename, psiitem, psinewlycreated);
            Ok(())
        }
        fn PreMoveItem(
            &self,
            dwflags: u32,
            psiitem: windows::core::Ref<IShellItem>,
            psidestinationfolder: windows::core::Ref<IShellItem>,
            psznewname: &windows::core::PCWSTR,
        ) -> windows::core::Result<()> {
            Ok(())
        }
        fn PostMoveItem(
            &self,
            dwflags: u32,
            psiitem: windows::core::Ref<IShellItem>,
            psidestinationfolder: windows::core::Ref<IShellItem>,
            psznewname: &windows::core::PCWSTR,
            hrmove: windows::core::HRESULT,
            psinewlycreated: windows::core::Ref<IShellItem>,
        ) -> windows::core::Result<()> {
            self.completed(ChangeKind::Move, hrmove, psiitem, psinewlycreated);
            Ok(())
        }
        fn PreCopyItem(
            &self,
            dwflags: u32,
            psiitem: windows::core::Ref<IShellItem>,
            psidestinationfolder: windows::core::Ref<IShellItem>,
            psznewname: &windows::core::PCWSTR,
        ) -> windows::core::Result<()> {
            Ok(())
        }
        fn PostCopyItem(
            &self,
            dwflags: u32,
            psiitem: windows::core::Ref<IShellItem>,
            psidestinationfolder: windows::core::Ref<IShellItem>,
            psznewname: &windows::core::PCWSTR,
            hrcopy: windows::core::HRESULT,
            psinewlycreated: windows::core::Ref<IShellItem>,
        ) -> windows::core::Result<()> {
            self.completed(ChangeKind::Copy, hrcopy, psiitem, psinewlycreated);
            Ok(())
        }
        fn PreDeleteItem(
            &self,
            dwflags: u32,
            psiitem: windows::core::Ref<IShellItem>,
        ) -> windows::core::Result<()> {
            Ok(())
        }
        fn PostDeleteItem(
            &self,
            dwflags: u32,
            psiitem: windows::core::Ref<IShellItem>,
            hrdelete: windows::core::HRESULT,
            psinewlycreated: windows::core::Ref<IShellItem>,
        ) -> windows::core::Result<()> {
            self.recycled(hrdelete, psiitem, psinewlycreated);
            Ok(())
        }
        fn PreNewItem(
            &self,
            dwflags: u32,
            psidestinationfolder: windows::core::Ref<IShellItem>,
            psznewname: &windows::core::PCWSTR,
        ) -> windows::core::Result<()> {
            Ok(())
        }
        fn PostNewItem(
            &self,
            dwflags: u32,
            psidestinationfolder: windows::core::Ref<IShellItem>,
            psznewname: &windows::core::PCWSTR,
            psztemplatename: &windows::core::PCWSTR,
            dwfileattributes: u32,
            hrnew: windows::core::HRESULT,
            psinewitem: windows::core::Ref<IShellItem>,
        ) -> windows::core::Result<()> {
            self.completed(
                ChangeKind::New,
                hrnew,
                windows::core::Ref::from(None::<&IShellItem>),
                psinewitem,
            );
            Ok(())
        }
        fn UpdateProgress(&self, iworktotal: u32, iworksofar: u32) -> windows::core::Result<()> {
            if iworktotal > 0 && iworksofar > 0 && iworksofar < iworktotal {
                let percent =
                    ((u64::from(iworksofar) * 100) / u64::from(iworktotal)).clamp(1, 99) as u8;
                let should_emit = {
                    let mut last = self.last_progress.lock().unwrap();
                    if *last == Some(percent) {
                        false
                    } else {
                        *last = Some(percent);
                        true
                    }
                };
                if should_emit {
                    (self.progress)(percent);
                }
            }
            Ok(())
        }
        fn ResetTimer(&self) -> windows::core::Result<()> {
            Ok(())
        }
        fn PauseTimer(&self) -> windows::core::Result<()> {
            Ok(())
        }
        fn ResumeTimer(&self) -> windows::core::Result<()> {
            Ok(())
        }
    }
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }
    fn item(p: &NativePath) -> windows::core::Result<IShellItem> {
        let mut text = p.0.clone();
        text.push(0);
        unsafe { SHCreateItemFromParsingName(PCWSTR(text.as_ptr()), None) }
    }
    fn perform_inner(
        action: &FileAction,
        paths: std::sync::Arc<std::sync::Mutex<Vec<NativePath>>>,
        changes: std::sync::Arc<std::sync::Mutex<Vec<Change>>>,
        outcomes: std::sync::Arc<std::sync::Mutex<Vec<crate::model::ItemOutcome>>>,
        progress: std::sync::Arc<dyn Fn(u8) + Send + Sync>,
    ) -> Result<bool, String> {
        let copy_in_place = if let FileAction::Copy {
            sources,
            destination,
        } = action
        {
            let dest = std::fs::canonicalize(destination.to_path()).map_err(|e| e.to_string())?;
            let mut same = false;
            for source in sources {
                let source = std::fs::canonicalize(source.to_path()).map_err(|e| e.to_string())?;
                same |= source.parent() == Some(dest.as_path());
            }
            same
        } else {
            false
        };
        // Every worker process enters COM on its main thread, an STA.
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED)
                .ok()
                .map_err(|e| e.to_string())?;
        }
        struct Com;
        impl Drop for Com {
            fn drop(&mut self) {
                unsafe {
                    CoUninitialize();
                }
            }
        }
        let _com = Com;
        let run = || -> windows::core::Result<bool> {
            unsafe {
                let operation: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_ALL)?;
                let mut flags = if matches!(action, FileAction::Delete { .. }) {
                    // The app's confirmation already covered these exact source items.
                    FOF_NOCONFIRMATION | FOF_NO_CONNECTED_ELEMENTS
                } else {
                    FOF_ALLOWUNDO | FOF_WANTNUKEWARNING | FOFX_ADDUNDORECORD | FOFX_RECYCLEONDELETE
                };
                if copy_in_place || matches!(action, FileAction::History { .. }) {
                    flags |= FOF_RENAMEONCOLLISION;
                }
                operation.SetOperationFlags(flags)?;
                let sink: IFileOperationProgressSink = OperationSink {
                    paths,
                    changes,
                    outcomes,
                    progress: progress.clone(),
                    last_progress: std::sync::Mutex::new(None),
                    existed: expected_targets(action)
                        .into_iter()
                        .filter(|p| p.to_path().exists())
                        .map(|p| item(&p))
                        .collect::<windows::core::Result<_>>()?,
                    sources: action_sources(action)
                        .into_iter()
                        .map(|source| {
                            let native = source_item(&source)?;
                            Ok((source, native))
                        })
                        .collect::<windows::core::Result<_>>()?,
                    parents: result_parents(action)
                        .into_iter()
                        .map(|path| {
                            let native = item(&path)?;
                            Ok((path, native))
                        })
                        .collect::<windows::core::Result<_>>()?,
                }
                .into();
                let cookie = operation.Advise(&sink)?;
                let none: Option<&IFileOperationProgressSink> = None;
                match action {
                    FileAction::History { record } => {
                        for change in &record.items {
                            let source = source_item(&change.source)?;
                            match &change.target {
                                ItemTarget::Recycle => operation.DeleteItem(&source, none)?,
                                ItemTarget::Path(path) => {
                                    let parent = path.parent().ok_or_else(|| {
                                        windows::core::Error::from(windows::core::HRESULT(
                                            0x80070057u32 as i32,
                                        ))
                                    })?;
                                    let target_path = path.to_path();
                                    let component = target_path.file_name().ok_or_else(|| {
                                        windows::core::Error::from(windows::core::HRESULT(
                                            0x80070057u32 as i32,
                                        ))
                                    })?;
                                    let mut name = NativePath::new(component).0;
                                    name.push(0);
                                    operation.MoveItem(
                                        &source,
                                        &item(&parent)?,
                                        PCWSTR(name.as_ptr()),
                                        none,
                                    )?;
                                }
                            }
                        }
                    }
                    FileAction::Copy {
                        sources,
                        destination,
                    }
                    | FileAction::Move {
                        sources,
                        destination,
                    } => {
                        let dest = item(destination)?;
                        for source in sources {
                            let source = item(source)?;
                            if matches!(action, FileAction::Copy { .. }) {
                                operation.CopyItem(&source, &dest, PCWSTR::null(), none)?;
                            } else {
                                operation.MoveItem(&source, &dest, PCWSTR::null(), none)?;
                            }
                        }
                    }
                    FileAction::Recycle { sources } | FileAction::Delete { sources } => {
                        for source in sources {
                            operation.DeleteItem(&item(source)?, none)?;
                        }
                    }
                    FileAction::BatchRename { pairs } => {
                        for pair in pairs {
                            let name = wide(&pair.name);
                            operation.RenameItem(
                                &item(&pair.source)?,
                                PCWSTR(name.as_ptr()),
                                none,
                            )?;
                        }
                    }
                    FileAction::Rename { source, name } => {
                        let name = wide(name);
                        operation.RenameItem(&item(source)?, PCWSTR(name.as_ptr()), none)?;
                    }
                    FileAction::NewFolder { destination, name } => {
                        let name = wide(name);
                        operation.NewItem(
                            &item(destination)?,
                            0x10,
                            PCWSTR(name.as_ptr()),
                            PCWSTR::null(),
                            none,
                        )?;
                    }
                }
                let performed = operation.PerformOperations();
                let _ = operation.Unadvise(cookie);
                if let Err(ref error) = performed {
                    if error.code() == COPYENGINE_E_USER_CANCELLED
                        || error.code() == COPYENGINE_E_CANCELLED
                        || error.code() == windows::core::HRESULT::from_win32(1223)
                    {
                        return Ok(true);
                    }
                }
                performed?;
                Ok(operation.GetAnyOperationsAborted()?.as_bool())
            }
        };
        run().map_err(|e| e.to_string())
    }
    pub fn open(path: &NativePath) -> Result<(), String> {
        let mut text = path.0.clone();
        text.push(0);
        let result = unsafe {
            ShellExecuteW(
                None,
                w!("open"),
                PCWSTR(text.as_ptr()),
                None,
                None,
                SW_SHOWNORMAL,
            )
        };
        if result.0 as isize <= 32 {
            Err(format!("열지 못했습니다 ({}).", result.0 as isize))
        } else {
            Ok(())
        }
    }
    pub fn clipboard_set(paths: &[NativePath], cut: bool) -> Result<(), String> {
        use clipboard_win::{Clipboard, Getter, formats, raw};
        let _clipboard = Clipboard::new_attempts(5).map_err(|e| e.to_string())?;
        raw::empty().map_err(|e| e.to_string())?;
        // DROPFILES header followed by double-NUL-terminated UTF-16 paths.
        let mut bytes = vec![0u8; 20];
        bytes[..4].copy_from_slice(&20u32.to_le_bytes());
        bytes[16..20].copy_from_slice(&1u32.to_le_bytes());
        for path in paths {
            for unit in path.0.iter().chain(std::iter::once(&0u16)) {
                bytes.extend_from_slice(&unit.to_le_bytes());
            }
        }
        bytes.extend_from_slice(&[0, 0]);
        raw::set_without_clear(formats::CF_HDROP, &bytes).map_err(|e| e.to_string())?;
        let id = raw::register_format("Preferred DropEffect")
            .ok_or("클립보드 형식을 만들지 못했습니다.")?;
        raw::set_without_clear(id.get(), &(if cut { 2u32 } else { 1u32 }).to_le_bytes())
            .map_err(|e| e.to_string())?;
        let mut recorded: Vec<std::path::PathBuf> = vec![];
        formats::FileList
            .read_clipboard(&mut recorded)
            .map_err(|e| e.to_string())?;
        if recorded.iter().map(NativePath::new).collect::<Vec<_>>() != paths {
            return Err("클립보드에 파일 목록을 기록하지 못했습니다.".into());
        }
        Ok(())
    }
    pub fn clipboard_get() -> Result<(Vec<NativePath>, bool), String> {
        use clipboard_win::{Clipboard, Getter, formats, raw};
        let _clipboard = Clipboard::new_attempts(5).map_err(|e| e.to_string())?;
        let mut paths: Vec<std::path::PathBuf> = vec![];
        formats::FileList
            .read_clipboard(&mut paths)
            .map_err(|e| e.to_string())?;
        let mut bytes = vec![];
        let cut = raw::register_format("Preferred DropEffect")
            .and_then(|id| formats::RawData(id.get()).read_clipboard(&mut bytes).ok())
            .is_some()
            && bytes.len() >= 4
            && u32::from_le_bytes(bytes[..4].try_into().unwrap()) == 2;
        Ok((paths.iter().map(NativePath::new).collect(), cut))
    }
}
