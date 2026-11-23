use crate::model::NativePath;
use std::{cell::RefCell, ffi::c_void, mem::size_of, os::windows::ffi::OsStrExt};
use windows::{
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
        System::{
            LibraryLoader::GetModuleHandleW,
            Ole::{OleInitialize, OleUninitialize},
        },
        UI::{
            HiDpi::{
                DPI_AWARENESS_CONTEXT, GetWindowDpiAwarenessContext, SetThreadDpiAwarenessContext,
            },
            Shell::{
                CMF_NORMAL, CMIC_MASK_PTINVOKE, CMINVOKECOMMANDINFO, CMINVOKECOMMANDINFOEX,
                Common::ITEMIDLIST, IContextMenu, IContextMenu2, IContextMenu3, ILFree,
                SEE_MASK_NOASYNC, SEE_MASK_UNICODE, SHGetDesktopFolder, SHParseDisplayName,
            },
            WindowsAndMessaging::{
                AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu,
                DestroyWindow, DispatchMessageW, GetForegroundWindow, GetWindowRect, IsWindow,
                MF_SEPARATOR, MF_STRING, PM_REMOVE, PeekMessageW, PostMessageW, RegisterClassW,
                SW_SHOWNORMAL, SetForegroundWindow, ShowWindow, TPM_RETURNCMD, TPM_RIGHTBUTTON,
                TrackPopupMenuEx, UnregisterClassW, WM_DRAWITEM, WM_INITMENUPOPUP, WM_MEASUREITEM,
                WM_MENUCHAR, WM_NULL, WNDCLASSW, WS_EX_TOOLWINDOW, WS_POPUP,
            },
        },
    },
    core::{Error, Interface, PCSTR, PCWSTR, PWSTR, w},
};
type AppResult<T> = Result<T, String>;
const COMMAND_FIRST: u32 = 1;
const COMMAND_LAST: u32 = 0x7fff;
const SDIR_COMMAND: u32 = COMMAND_LAST + 1;

thread_local! {
    static MENU_HANDLER: RefCell<Option<MenuHandler>> = const { RefCell::new(None) };
}

#[derive(Clone)]
enum MenuHandler {
    V3(IContextMenu3),
    V2(IContextMenu2),
}

unsafe extern "system" fn menu_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if matches!(
        message,
        WM_INITMENUPOPUP | WM_DRAWITEM | WM_MEASUREITEM | WM_MENUCHAR
    ) {
        let handler = MENU_HANDLER.with(|slot| slot.borrow().clone());
        if let Some(handler) = handler {
            match handler {
                MenuHandler::V3(menu) => {
                    let mut result = LRESULT(0);
                    if unsafe {
                        menu.HandleMenuMsg2(
                            message,
                            wparam,
                            lparam,
                            Some(&mut result as *mut LRESULT),
                        )
                    }
                    .is_ok()
                    {
                        return result;
                    }
                }
                MenuHandler::V2(menu)
                    if matches!(message, WM_INITMENUPOPUP | WM_DRAWITEM | WM_MEASUREITEM) =>
                {
                    if unsafe { menu.HandleMenuMsg(message, wparam, lparam) }.is_ok() {
                        return LRESULT(0);
                    }
                }
                MenuHandler::V2(_) => {}
            }
        }
    }
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

struct MenuHandlerScope;
impl MenuHandlerScope {
    fn install(menu: &IContextMenu) -> Self {
        let handler = menu
            .cast::<IContextMenu3>()
            .map(MenuHandler::V3)
            .or_else(|_| menu.cast::<IContextMenu2>().map(MenuHandler::V2))
            .ok();
        MENU_HANDLER.with(|slot| *slot.borrow_mut() = handler);
        Self
    }
}
impl Drop for MenuHandlerScope {
    fn drop(&mut self) {
        MENU_HANDLER.with(|slot| *slot.borrow_mut() = None);
    }
}
struct OleApartment;
impl OleApartment {
    fn initialize() -> AppResult<Self> {
        unsafe { OleInitialize(None) }.map_err(|error| error.to_string())?;
        Ok(Self)
    }
}
impl Drop for OleApartment {
    fn drop(&mut self) {
        unsafe { OleUninitialize() };
    }
}

struct ThreadDpiContext(DPI_AWARENESS_CONTEXT);
impl ThreadDpiContext {
    fn set_from_window(hwnd: HWND) -> AppResult<Self> {
        let context = unsafe { GetWindowDpiAwarenessContext(hwnd) };
        let previous = unsafe { SetThreadDpiAwarenessContext(context) };
        if previous.0.is_null() {
            return Err(Error::from_thread().to_string());
        }
        Ok(Self(previous))
    }
}
impl Drop for ThreadDpiContext {
    fn drop(&mut self) {
        unsafe { SetThreadDpiAwarenessContext(self.0) };
    }
}

struct OwnedMenu(windows::Win32::UI::WindowsAndMessaging::HMENU);
impl Drop for OwnedMenu {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyMenu(self.0);
        }
    }
}

struct OwnedPidl(*mut ITEMIDLIST);
impl Drop for OwnedPidl {
    fn drop(&mut self) {
        unsafe {
            ILFree(Some(self.0.cast_const()));
        }
    }
}

struct WindowClass {
    name: Vec<u16>,
    instance: HINSTANCE,
}
impl WindowClass {
    fn register() -> AppResult<Self> {
        let mut name = format!("SdirFolderMenu-{}", std::process::id())
            .encode_utf16()
            .collect::<Vec<_>>();
        name.push(0);
        let module = unsafe { GetModuleHandleW(None) }.map_err(|error| error.to_string())?;
        let instance = HINSTANCE(module.0);
        let class = WNDCLASSW {
            lpfnWndProc: Some(menu_window_proc),
            hInstance: instance,
            lpszClassName: PCWSTR(name.as_ptr()),
            ..Default::default()
        };
        if unsafe { RegisterClassW(&class) } == 0 {
            return Err(Error::from_thread().to_string());
        }
        Ok(Self { name, instance })
    }

    fn create_popup(self, x: i32, y: i32) -> AppResult<PopupWindow> {
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                PCWSTR(self.name.as_ptr()),
                PCWSTR(std::ptr::null()),
                WS_POPUP,
                x,
                y,
                1,
                1,
                None,
                None,
                Some(self.instance),
                None,
            )
        }
        .map_err(|error| error.to_string())?;
        Ok(PopupWindow { hwnd, _class: self })
    }
}
impl Drop for WindowClass {
    fn drop(&mut self) {
        unsafe {
            let _ = UnregisterClassW(PCWSTR(self.name.as_ptr()), Some(self.instance));
        }
    }
}

struct PopupWindow {
    hwnd: HWND,
    _class: WindowClass,
}
impl Drop for PopupWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

struct RestoreForeground {
    owner: HWND,
    popup: HWND,
}
impl Drop for RestoreForeground {
    fn drop(&mut self) {
        let foreground = unsafe { GetForegroundWindow() };
        if foreground == self.popup || foreground == self.owner {
            unsafe {
                let _ = SetForegroundWindow(self.owner);
            }
        }
    }
}
fn window_center(hwnd: HWND) -> AppResult<(i32, i32)> {
    let mut rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut rect) }.map_err(|error| error.to_string())?;
    Ok((
        rect.left + (rect.right - rect.left) / 2,
        rect.top + (rect.bottom - rect.top) / 2,
    ))
}

pub(super) fn run(
    path: &NativePath,
    entries: &[NativePath],
    owner_value: u64,
    point: Option<(i32, i32)>,
) -> AppResult<Option<u32>> {
    if !path.to_path().is_dir() {
        return Err("Windows 메뉴를 열 폴더에 접근할 수 없습니다.".into());
    }
    let owner = HWND(owner_value as usize as *mut c_void);
    if owner_value == 0 || !unsafe { IsWindow(Some(owner)) }.as_bool() {
        return Err("Windows 메뉴를 열 창을 찾을 수 없습니다.".into());
    }
    if unsafe { GetForegroundWindow() } != owner {
        return Err("Windows 메뉴를 열 SDIR 창이 활성화되어 있지 않습니다.".into());
    }
    let _dpi = ThreadDpiContext::set_from_window(owner)?;
    let _ole = OleApartment::initialize()?;
    let (x, y) = match point {
        Some(point) => point,
        None => window_center(owner)?,
    };
    let class = WindowClass::register()?;
    let popup = class.create_popup(x, y)?;
    let _restore = RestoreForeground {
        owner,
        popup: popup.hwnd,
    };
    let mut folder_name = path.0.clone();
    folder_name.push(0);
    let mut raw_pidl = std::ptr::null_mut();
    unsafe { SHParseDisplayName(PCWSTR(folder_name.as_ptr()), None, &mut raw_pidl, 0, None) }
        .map_err(|error| format!("Windows 폴더 메뉴를 준비하지 못했습니다: {error}"))?;
    if raw_pidl.is_null() {
        return Err("Windows 폴더 메뉴를 준비하지 못했습니다.".into());
    }
    let pidl = OwnedPidl(raw_pidl);
    let desktop = unsafe { SHGetDesktopFolder() }
        .map_err(|error| format!("Windows 폴더 메뉴를 준비하지 못했습니다: {error}"))?;
    let folder =
        unsafe { desktop.BindToObject::<_, windows::Win32::UI::Shell::IShellFolder>(pidl.0, None) }
            .map_err(|error| format!("Windows 폴더 메뉴를 준비하지 못했습니다: {error}"))?;
    let children = entries
        .iter()
        .map(|entry| {
            let path = entry.to_path();
            let mut name = path
                .file_name()
                .ok_or("Windows 메뉴에 사용할 항목 이름이 없습니다.")?
                .encode_wide()
                .chain(Some(0))
                .collect::<Vec<_>>();
            let mut raw = std::ptr::null_mut();
            let mut attributes = 0;
            unsafe {
                folder.ParseDisplayName(
                    popup.hwnd,
                    None,
                    PWSTR(name.as_mut_ptr()),
                    None,
                    &mut raw,
                    &mut attributes,
                )
            }
            .map_err(|error| format!("Windows 메뉴 항목을 준비하지 못했습니다: {error}"))?;
            if raw.is_null() {
                return Err("Windows 메뉴 항목을 준비하지 못했습니다.".to_owned());
            }
            Ok(OwnedPidl(raw))
        })
        .collect::<AppResult<Vec<_>>>()?;
    let context = if children.is_empty() {
        unsafe { folder.CreateViewObject::<IContextMenu>(popup.hwnd) }
    } else {
        let relative = children
            .iter()
            .map(|pidl| pidl.0.cast_const())
            .collect::<Vec<_>>();
        unsafe { folder.GetUIObjectOf::<IContextMenu>(popup.hwnd, &relative, None) }
    }
    .map_err(|error| format!("Windows 메뉴를 만들 수 없습니다: {error}"))?;
    let menu = OwnedMenu(
        unsafe { CreatePopupMenu() }
            .map_err(|error| format!("Windows 폴더 메뉴를 만들 수 없습니다: {error}"))?,
    );
    let query =
        unsafe { context.QueryContextMenu(menu.0, 0, COMMAND_FIRST, COMMAND_LAST, CMF_NORMAL) };
    if query.0 < 0 {
        return Err(format!("Windows 폴더 메뉴를 만들 수 없습니다: {query}"));
    }
    let command_count = (query.0 as u32) & 0xffff;
    if command_count == 0 {
        return Err("Windows 폴더 메뉴를 만들 수 없습니다.".into());
    }
    unsafe {
        AppendMenuW(menu.0, MF_SEPARATOR, 0, PCWSTR::null())
            .and_then(|()| AppendMenuW(menu.0, MF_STRING, SDIR_COMMAND as usize, w!("SDIR 도구…")))
    }
    .map_err(|error| format!("Windows 메뉴에 SDIR 도구를 추가하지 못했습니다: {error}"))?;
    if unsafe { GetForegroundWindow() } != owner {
        return Err("Windows 메뉴를 열기 전에 다른 창이 활성화됐습니다.".into());
    }
    unsafe {
        let _ = ShowWindow(popup.hwnd, SW_SHOWNORMAL);
        let _ = SetForegroundWindow(popup.hwnd);
    }
    if unsafe { GetForegroundWindow() } != popup.hwnd {
        return Err("Windows 팝업 메뉴를 활성화하지 못했습니다.".into());
    }
    let _handler = MenuHandlerScope::install(&context);
    let selected = unsafe {
        TrackPopupMenuEx(
            menu.0,
            (TPM_RETURNCMD | TPM_RIGHTBUTTON).0,
            x,
            y,
            popup.hwnd,
            None,
        )
    }
    .0;
    unsafe {
        let _ = PostMessageW(Some(popup.hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let mut message = windows::Win32::UI::WindowsAndMessaging::MSG::default();
        while PeekMessageW(&mut message, Some(popup.hwnd), WM_NULL, WM_NULL, PM_REMOVE).as_bool() {
            DispatchMessageW(&message);
        }
    }
    if selected <= 0 {
        return Ok(None);
    }
    let selected = selected as u32;
    if selected == SDIR_COMMAND {
        return Ok(Some(super::SDIR_MENU_SELECTED));
    }
    let Some(command_offset) = selected.checked_sub(COMMAND_FIRST) else {
        return Ok(None);
    };
    if command_offset >= command_count {
        return Ok(None);
    }
    let mut invoke = CMINVOKECOMMANDINFOEX::default();
    invoke.cbSize = size_of::<CMINVOKECOMMANDINFOEX>() as u32;
    invoke.fMask = SEE_MASK_UNICODE | SEE_MASK_NOASYNC | CMIC_MASK_PTINVOKE;
    invoke.hwnd = popup.hwnd;
    invoke.lpVerb = PCSTR(command_offset as usize as *const u8);
    invoke.lpVerbW = PCWSTR(command_offset as usize as *const u16);
    invoke.lpDirectoryW = PCWSTR(folder_name.as_ptr());
    invoke.nShow = SW_SHOWNORMAL.0;
    invoke.ptInvoke = POINT { x, y };
    let base = (&invoke as *const CMINVOKECOMMANDINFOEX).cast::<CMINVOKECOMMANDINFO>();
    unsafe { context.InvokeCommand(base) }
        .map_err(|error| format!("Windows 메뉴 명령을 실행하지 못했습니다: {error}"))?;
    Ok(None)
}

pub(super) fn properties(path: &NativePath, entries: &[NativePath]) -> AppResult<()> {
    use windows::Win32::{
        System::Com::IDataObject,
        UI::Shell::{IShellFolder, SHBindToParent, SHCreateDataObject, SHMultiFileProperties},
    };
    if !(2..=500).contains(&entries.len()) {
        return Err("다중 속성은 한 폴더의 항목 2~500개를 선택하세요.".into());
    }
    let parent = crate::tree::tree_key(path.clone());
    if !entries
        .iter()
        .all(|entry| entry.parent().map(crate::tree::tree_key).as_ref() == Some(&parent))
    {
        return Err("같은 폴더 안의 항목을 선택하세요.".into());
    }
    let _ole = OleApartment::initialize()?;
    fn parse(path: &NativePath) -> AppResult<OwnedPidl> {
        let text: Vec<_> = path.0.iter().copied().chain(Some(0)).collect();
        let mut raw = std::ptr::null_mut();
        unsafe { SHParseDisplayName(PCWSTR(text.as_ptr()), None, &mut raw, 0, None) }
            .map_err(|error| error.to_string())?;
        if raw.is_null() {
            return Err("Windows 항목을 찾을 수 없습니다.".into());
        }
        Ok(OwnedPidl(raw))
    }
    let folder = parse(path)?;
    let children = entries.iter().map(parse).collect::<AppResult<Vec<_>>>()?;
    let pointers = children
        .iter()
        .map(|pidl| {
            let mut child = std::ptr::null_mut();
            let _: IShellFolder = unsafe { SHBindToParent(pidl.0, Some(&mut child)) }
                .map_err(|error| error.to_string())?;
            Ok(child.cast_const())
        })
        .collect::<AppResult<Vec<_>>>()?;
    let data: IDataObject =
        unsafe { SHCreateDataObject(Some(folder.0), Some(&pointers), None::<&IDataObject>) }
            .map_err(|error| error.to_string())?;
    unsafe {
        SHMultiFileProperties(&data, 0).map_err(|error| error.to_string())?;
        crate::platform::wait_shell_windows();
    }
    Ok(())
}
