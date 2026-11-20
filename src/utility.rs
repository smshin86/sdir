use crate::model::NativePath;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Utility {
    CopyPaths(Vec<NativePath>),
    Drag(Vec<NativePath>, u64),
    OpenWith(NativePath, u64),
    Terminal(NativePath),
    Explorer(NativePath),
}
pub fn run(action: &Utility) -> Result<Option<u32>, String> {
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
            Utility::Drag(paths, owner) => crate::drag::run(paths, *owner),
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
        }
    }
    #[cfg(not(windows))]
    {
        let _ = action;
        Err("Windows 전용 기능입니다.".into())
    }
}
