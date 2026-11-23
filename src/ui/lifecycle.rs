use super::{AppModel, Bridge};
use dioxus::{
    desktop::{DesktopContext, WindowCloseBehaviour},
    prelude::*,
};
use sdir::settings::{SettingsWriter, WindowPlacement};
use std::{cell::RefCell, ffi::OsString, os::windows::ffi::OsStrExt, sync::Arc};
use windows::{
    Win32::{
        Foundation::{
            CloseHandle, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, GetLastError,
            HANDLE, WAIT_OBJECT_0,
        },
        System::{
            Registry::{
                HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE,
                REG_SZ, REG_VALUE_TYPE, RegCloseKey, RegCreateKeyExW, RegDeleteValueW,
                RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
            },
            Threading::{CreateEventW, SetEvent, WaitForSingleObject},
        },
    },
    core::{PCWSTR, w},
};

pub(super) const OPEN_ID: &str = "sdir-open-window";
pub(super) const EXIT_ID: &str = "sdir-exit";
const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: PCWSTR = w!("SDIR");

thread_local! {
    static ACTIVATION: RefCell<Option<Activation>> = const { RefCell::new(None) };
}

struct Activation(HANDLE);
impl Drop for Activation {
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

pub(super) fn claim_window(login_start: bool) -> Result<bool, String> {
    let folder = sdir::settings::data_dir();
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    let canonical = std::fs::canonicalize(folder).map_err(|e| e.to_string())?;
    let hash = canonical
        .to_string_lossy()
        .to_lowercase()
        .encode_utf16()
        .fold(0xcbf29ce484222325u64, |hash, unit| {
            (hash ^ u64::from(unit)).wrapping_mul(0x00000100000001b3)
        });
    let name: Vec<_> = format!("Local\\SDIR-Window-{hash:016x}")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let handle = unsafe { CreateEventW(None, false, false, PCWSTR(name.as_ptr())) }
        .map_err(|e| e.to_string())?;
    let existing = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    let event = Activation(handle);
    if existing {
        if !login_start {
            unsafe { SetEvent(event.0) }.map_err(|e| e.to_string())?;
        }
        return Ok(false);
    }
    ACTIVATION.with(|slot| *slot.borrow_mut() = Some(event));
    Ok(true)
}

pub(super) fn release_activation() {
    ACTIVATION.with(|slot| slot.borrow_mut().take());
}

pub(super) fn activation_pending() -> bool {
    ACTIVATION.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|event| unsafe { WaitForSingleObject(event.0, 0) == WAIT_OBJECT_0 })
    })
}

pub(super) fn make_tray() -> Result<dioxus::desktop::trayicon::TrayIcon, String> {
    use dioxus::desktop::trayicon::{
        Icon, TrayIconBuilder,
        menu::{Menu, MenuItem},
    };
    let menu = Menu::new();
    let open = MenuItem::with_id(OPEN_ID, "열기", true, None);
    let exit = MenuItem::with_id(EXIT_ID, "종료", true, None);
    menu.append_items(&[&open, &exit])
        .map_err(|e| e.to_string())?;
    let icon = Icon::from_resource(101, None).map_err(|e| e.to_string())?;
    TrayIconBuilder::new()
        .with_icon(icon)
        .with_tooltip("SDIR")
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()
        .map_err(|e| e.to_string())
}

pub(super) fn show_window(window: &DesktopContext) {
    window.window.set_visible(true);
    window.window.set_minimized(false);
    window.window.set_focus();
}

fn window_handle(window: &DesktopContext) -> windows::Win32::Foundation::HWND {
    use dioxus::desktop::tao::platform::windows::WindowExtWindows;
    windows::Win32::Foundation::HWND(window.window.hwnd() as *mut std::ffi::c_void)
}

pub(super) fn request_close(window: &DesktopContext) {
    use windows::Win32::{
        Foundation::{LPARAM, WPARAM},
        UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE},
    };
    let _ = unsafe { PostMessageW(Some(window_handle(window)), WM_CLOSE, WPARAM(0), LPARAM(0)) };
}

pub(super) fn restore_placement(
    window: &DesktopContext,
    saved: &WindowPlacement,
) -> WindowPlacement {
    use dioxus::desktop::tao::dpi::{LogicalSize, PhysicalPosition};
    use windows::Win32::{
        Foundation::POINT,
        Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint},
    };
    let point = POINT {
        x: saved.x.saturating_add(100),
        y: saved.y.saturating_add(60),
    };
    let monitor = unsafe { MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let mut normal = saved.clone();
    if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        let work = info.rcWork;
        let scale = window.scale_factor();
        let work_width = work.right - work.left;
        let work_height = work.bottom - work.top;
        let outer = window.outer_size();
        let inner = window.inner_size();
        let frame_width = outer.width.saturating_sub(inner.width) as f64 / scale;
        let frame_height = outer.height.saturating_sub(inner.height) as f64 / scale;
        let max_width = (work_width as f64 / scale - frame_width - 48.).max(980.);
        let max_height = (work_height as f64 / scale - frame_height - 48.).max(580.);
        if normal.width > max_width || normal.height > max_height {
            normal.width = normal.width.min(1380.);
            normal.height = normal.height.min(880.);
        }
        normal.width = normal.width.clamp(980., max_width);
        normal.height = normal.height.clamp(580., max_height);
        let outer_width = ((normal.width + frame_width) * scale).round() as i32;
        let outer_height = ((normal.height + frame_height) * scale).round() as i32;
        normal.x = normal
            .x
            .clamp(work.left, (work.right - outer_width).max(work.left));
        normal.y = normal
            .y
            .clamp(work.top, (work.bottom - outer_height).max(work.top));
    }
    window.set_inner_size(LogicalSize::new(normal.width, normal.height));
    window.set_outer_position(PhysicalPosition::new(normal.x, normal.y));
    window.set_maximized(normal.maximized);
    normal
}

pub(super) fn observe_placement(
    window: &DesktopContext,
    previous: Option<&WindowPlacement>,
) -> Option<WindowPlacement> {
    use windows::Win32::UI::WindowsAndMessaging::{IsIconic, IsZoomed};
    let hwnd = window_handle(window);
    if window.fullscreen().is_some() || unsafe { IsIconic(hwnd) }.as_bool() {
        return None;
    }
    if unsafe { IsZoomed(hwnd) }.as_bool() {
        let mut saved = previous?.clone();
        saved.maximized = true;
        return Some(saved);
    }
    let pos = window.outer_position().ok()?;
    let size = window.inner_size();
    if size.width == 0 || size.height == 0 {
        return None;
    }
    let scale = window.scale_factor();
    Some(WindowPlacement {
        x: pos.x,
        y: pos.y,
        width: size.width as f64 / scale,
        height: size.height as f64 / scale,
        maximized: false,
    })
}

pub(super) fn save_then_close(
    mut model: Signal<AppModel>,
    writer: &SettingsWriter,
    window: &DesktopContext,
    bridge: &Arc<Bridge>,
) {
    window.set_close_behavior(WindowCloseBehaviour::WindowHides);
    if model.read().closing {
        return;
    }
    if model.read().close_save_failed {
        bridge.search.cancel();
        window.set_close_behavior(WindowCloseBehaviour::WindowCloses);
        window.close();
        return;
    }
    let settings = {
        let mut state = model.write();
        if let Some(placement) = observe_placement(window, state.window.as_ref()) {
            state.window = Some(placement);
        }
        state.closing = true;
        state.save_due = None;
        state.settings()
    };
    let finished = writer.flush(settings);
    let window = window.clone();
    let bridge = bridge.clone();
    spawn(async move {
        let result = tokio::task::spawn_blocking(move || finished.recv()).await;
        if matches!(result, Ok(Ok(Ok(())))) {
            bridge.search.cancel();
            window.set_close_behavior(WindowCloseBehaviour::WindowCloses);
            window.close();
        } else {
            let mut state = model.write();
            state.closing = false;
            state.close_save_failed = true;
            state.notice =
                "마지막 설정을 저장하지 못했습니다. 다시 종료하면 저장하지 않고 닫습니다.".into();
            show_window(&window);
        }
    });
}

pub(super) fn set_login_start(enabled: bool) -> Result<(), String> {
    if std::env::var_os("SDIR_PROFILE_DIR").is_some() {
        return Err("격리된 프로필에서는 Windows 로그인 시 실행을 등록할 수 없습니다.".into());
    }
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut command = OsString::from("\"");
    command.push(executable);
    command.push("\" --login-start");
    let value: Vec<u8> = command
        .encode_wide()
        .chain(Some(0))
        .flat_map(u16::to_le_bytes)
        .collect();
    let mut handle = HKEY::default();
    let access = KEY_SET_VALUE | KEY_QUERY_VALUE;
    let status = unsafe {
        if enabled {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                access,
                None,
                &mut handle,
                None,
            )
        } else {
            RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, access, &mut handle)
        }
    };
    if !enabled && status == ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    if status != ERROR_SUCCESS {
        return Err(format!(
            "Windows 로그인 실행 항목을 열지 못했습니다 ({})",
            status.0
        ));
    }
    let key = RegistryKey(handle);
    let status = if enabled {
        unsafe { RegSetValueExW(key.0, RUN_VALUE, None, REG_SZ, Some(&value)) }
    } else {
        let mut existing = vec![0; value.len()];
        let mut size = existing.len() as u32;
        let mut value_type = REG_VALUE_TYPE::default();
        let status = unsafe {
            RegQueryValueExW(
                key.0,
                RUN_VALUE,
                None,
                Some(&mut value_type),
                Some(existing.as_mut_ptr()),
                Some(&mut size),
            )
        };
        if status != ERROR_SUCCESS
            || value_type != REG_SZ
            || size as usize != value.len()
            || existing != value
        {
            return Ok(());
        }
        unsafe { RegDeleteValueW(key.0, RUN_VALUE) }
    };
    if status == ERROR_SUCCESS || (!enabled && status == ERROR_FILE_NOT_FOUND) {
        Ok(())
    } else {
        Err(format!(
            "Windows 로그인 실행 설정을 변경하지 못했습니다 ({})",
            status.0
        ))
    }
}
