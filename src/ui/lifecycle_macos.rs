use dioxus::desktop::{
    DesktopContext,
    tao::dpi::{LogicalSize, PhysicalPosition},
};
use sdir::settings::WindowPlacement;
use std::{
    fs::{File, OpenOptions},
    os::fd::AsRawFd,
};

thread_local! {
    static INSTANCE: std::cell::RefCell<Option<File>> = const { std::cell::RefCell::new(None) };
}

pub(super) fn claim_window(_: bool) -> Result<bool, String> {
    let folder = sdir::settings::data_dir();
    std::fs::create_dir_all(&folder).map_err(|error| error.to_string())?;
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(folder.join("session.lock"))
        .map_err(|error| error.to_string())?;
    // The OS releases the lock on exit, including on abnormal termination.
    let status = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if status != 0 {
        let error = std::io::Error::last_os_error();
        return if error.kind() == std::io::ErrorKind::WouldBlock {
            Ok(false)
        } else {
            Err(error.to_string())
        };
    }
    INSTANCE.with(|slot| *slot.borrow_mut() = Some(file));
    Ok(true)
}

pub(super) fn release_activation() {
    INSTANCE.with(|slot| slot.borrow_mut().take());
}

pub(super) fn activation_pending() -> bool {
    false
}

pub(super) fn show_window(window: &DesktopContext) {
    window.set_visible(true);
    window.set_minimized(false);
    window.set_focus();
}

pub(super) fn restore_placement(
    window: &DesktopContext,
    saved: &WindowPlacement,
) -> WindowPlacement {
    let mut normal = saved.clone();
    if let Some(monitor) = window
        .current_monitor()
        .or_else(|| window.primary_monitor())
    {
        let scale = monitor.scale_factor();
        let area = monitor.size();
        let origin = monitor.position();
        normal.width = normal
            .width
            .clamp(980., (f64::from(area.width) / scale - 30.).max(980.));
        normal.height = normal
            .height
            .clamp(580., (f64::from(area.height) / scale - 80.).max(580.));
        let right = origin
            .x
            .saturating_add((f64::from(area.width) - normal.width * scale).max(0.) as i32);
        let bottom = origin
            .y
            .saturating_add((f64::from(area.height) - normal.height * scale - 40.).max(0.) as i32);
        normal.x = normal.x.clamp(origin.x, right);
        normal.y = normal.y.clamp(origin.y, bottom);
        window.set_outer_position(PhysicalPosition::new(normal.x, normal.y));
    }
    window.set_inner_size(LogicalSize::new(normal.width, normal.height));
    window.set_maximized(normal.maximized);
    normal
}

pub(super) fn observe_placement(
    window: &DesktopContext,
    previous: Option<&WindowPlacement>,
) -> Option<WindowPlacement> {
    if window.fullscreen().is_some() || window.is_minimized() {
        return None;
    }
    if window.is_maximized() {
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
        width: f64::from(size.width) / scale,
        height: f64::from(size.height) / scale,
        maximized: false,
    })
}

pub(super) fn set_login_start(_: bool) -> Result<(), String> {
    Err("macOS에서는 시스템 설정의 로그인 항목을 사용하세요.".into())
}
