use crate::model::NativePath;
#[cfg(windows)]
mod folder_menu;
#[cfg(windows)]
mod network;
#[cfg(windows)]
mod shortcuts;
use serde::{Deserialize, Serialize};
pub const SDIR_MENU_SELECTED: u32 = 1;
pub const SDIR_DRAG_CANCELLED: u32 = 0;
pub const SDIR_DRAG_EXTERNAL_HANDOFF: u32 = 1;
pub const SDIR_DRAG_VERIFIED_MOVE: u32 = 2;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Utility {
    CopyPaths(Vec<NativePath>),
    CreateShortcuts {
        folder: NativePath,
        paths: Vec<NativePath>,
    },
    Drag {
        id: u64,
        paths: Vec<NativePath>,
        owner: u64,
        ticket: Option<crate::drag::DragTicket>,
    },
    OpenWith(NativePath, u64),
    NetworkDrive {
        drive: String,
        remote: Option<String>,
        remember: bool,
    },
    PropertiesSelection {
        folder: NativePath,
        entries: Vec<NativePath>,
    },
    Terminal(NativePath),
    Explorer(NativePath),
    FolderMenu {
        request: u64,
        folder: NativePath,
        entries: Vec<NativePath>,
        owner: u64,
        point: Option<(i32, i32)>,
    },
}
pub fn validate_network_drive(drive: &str, remote: Option<&str>) -> Result<(), String> {
    if !matches!(drive.as_bytes(), [letter, b':'] if letter.is_ascii_alphabetic()) {
        return Err("드라이브 문자는 Z:처럼 입력하세요.".into());
    }
    if let Some(remote) = remote {
        let Some(rest) = remote.strip_prefix(r"\\") else {
            return Err("네트워크 경로는 \\\\서버\\공유 형식이어야 합니다.".into());
        };
        let mut parts = rest.trim_end_matches('\\').split('\\');
        let valid = |part: &str| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && !part
                    .chars()
                    .any(|ch| ch.is_control() || "/:?*\"<>|".contains(ch))
        };
        if !parts.next().is_some_and(valid)
            || !parts.next().is_some_and(valid)
            || !parts.all(valid)
            || remote.encode_utf16().count() > 2048
        {
            return Err("네트워크 경로는 \\\\서버\\공유 형식이어야 합니다.".into());
        }
    }
    Ok(())
}

#[cfg(windows)]
pub fn screen_cursor_position() -> Result<(i32, i32), String> {
    use windows::{Win32::Foundation::POINT, Win32::UI::WindowsAndMessaging::GetCursorPos};
    let mut point = POINT::default();
    unsafe { GetCursorPos(&mut point) }.map_err(|e| e.to_string())?;
    Ok((point.x, point.y))
}
pub fn run(action: &Utility) -> Result<Option<u32>, String> {
    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        match action {
            Utility::CopyPaths(paths) => crate::macos::copy_path_text(paths).map(|()| None),
            Utility::Explorer(path) => {
                crate::macos::reveal(path);
                Ok(None)
            }
            Utility::Terminal(path) => {
                let status = Command::new("/usr/bin/open")
                    .args(["-a", "Terminal"])
                    .arg(path.to_path())
                    .status()
                    .map_err(|error| error.to_string())?;
                status
                    .success()
                    .then_some(None)
                    .ok_or_else(|| "터미널을 열 수 없습니다.".into())
            }
            Utility::PropertiesSelection { entries, .. } => entries
                .first()
                .ok_or_else(|| "항목을 선택해 주세요.".to_string())
                .and_then(crate::macos::properties)
                .map(|()| None),
            _ => Err("이 작업은 macOS에서 지원하지 않습니다.".into()),
        }
    }
    #[cfg(windows)]
    {
        use windows::{
            Win32::{
                Foundation::CloseHandle,
                System::Threading::{
                    CREATE_NEW_CONSOLE, CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW,
                },
            },
            core::{PCWSTR, PWSTR},
        };
        match action {
            Utility::Drag {
                paths,
                owner,
                ticket: Some(ticket),
                ..
            } => crate::drag::run(paths, *owner, ticket),
            Utility::Drag { ticket: None, .. } => Err("드래그 입력 정보를 받지 못했습니다.".into()),
            Utility::CreateShortcuts { folder, paths } => {
                shortcuts::create(folder, paths)?;
                Ok(None)
            }
            Utility::CopyPaths(paths) => {
                let text = paths
                    .iter()
                    .map(|p| format!("\"{}\"", p.display()))
                    .collect::<Vec<_>>()
                    .join("\r\n");
                clipboard_win::set_clipboard_string(&text).map_err(|e| e.to_string())?;
                Ok(None)
            }
            Utility::OpenWith(path, owner) => crate::open_with::run(path, *owner),
            Utility::NetworkDrive {
                drive,
                remote,
                remember,
            } => {
                network::change(drive, remote.as_deref(), *remember)?;
                Ok(Some(1))
            }
            Utility::PropertiesSelection { folder, entries } => {
                folder_menu::properties(folder, entries)?;
                Ok(None)
            }
            Utility::Terminal(path) => unsafe {
                if !path.to_path().is_dir() {
                    return Err(
                        "터미널을 열 폴더에 접근할 수 없습니다. 연결과 권한을 확인하세요.".into(),
                    );
                }
                let app = std::path::PathBuf::from(
                    std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into()),
                )
                .join("System32/WindowsPowerShell/v1.0/powershell.exe");
                let mut app = NativePath::new(app).0;
                app.push(0);
                let mut dir = path.0.clone();
                dir.push(0);
                let mut args: Vec<u16> = "powershell.exe -NoLogo -NoProfile -NoExit"
                    .encode_utf16()
                    .chain(Some(0))
                    .collect();
                let startup = STARTUPINFOW {
                    cb: std::mem::size_of::<STARTUPINFOW>() as u32,
                    ..Default::default()
                };
                let mut info = PROCESS_INFORMATION::default();
                CreateProcessW(
                    PCWSTR(app.as_ptr()),
                    Some(PWSTR(args.as_mut_ptr())),
                    None,
                    None,
                    false,
                    CREATE_NEW_CONSOLE,
                    None,
                    PCWSTR(dir.as_ptr()),
                    &startup,
                    &mut info,
                )
                .map_err(|e| e.to_string())?;
                let pid = info.dwProcessId;
                let _ = CloseHandle(info.hThread);
                let _ = CloseHandle(info.hProcess);
                Ok(Some(pid))
            },
            Utility::Explorer(path) => {
                let mut arg = std::ffi::OsString::from("/select,");
                arg.push(path.to_path());
                let c = std::process::Command::new("explorer.exe")
                    .arg(arg)
                    .spawn()
                    .map_err(|e| e.to_string())?;
                Ok(Some(c.id()))
            }
            Utility::FolderMenu {
                folder,
                entries,
                owner,
                point,
                ..
            } => folder_menu::run(folder, entries, *owner, *point),
        }
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = action;
        Err("Windows 전용 기능입니다.".into())
    }
}

#[cfg(test)]
mod network_tests {
    use super::validate_network_drive;

    #[test]
    fn accepts_only_normal_drive_letters_and_unc_share_paths() {
        assert!(validate_network_drive("Z:", Some(r"\\테스트서버\팀 자료\보고서")).is_ok());
        assert!(validate_network_drive("X:", None).is_ok());
        for path in [
            r"Z:\local",
            r"\\server",
            r"\\?\UNC\srv\share",
            r"\\server\share\..",
        ] {
            assert!(validate_network_drive("Z:", Some(path)).is_err(), "{path}");
        }
        assert!(validate_network_drive("1:", None).is_err());
        assert!(validate_network_drive("ZZ", None).is_err());
    }
}
