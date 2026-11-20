use crate::model::NativePath;
/// Select an executable explicitly; never changes the user's default file association.
pub fn run(path: &NativePath, owner: u64) -> Result<Option<u32>, String> {
    #[cfg(windows)]
    unsafe {
        use windows::{
            Win32::{
                Foundation::HWND,
                System::Com::{
                    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance,
                    CoInitializeEx, CoTaskMemFree, CoUninitialize,
                },
                UI::Shell::{
                    Common::COMDLG_FILTERSPEC, FOS_DONTADDTORECENT, FOS_FILEMUSTEXIST,
                    FOS_FORCEFILESYSTEM, FOS_NOCHANGEDIR, FOS_PATHMUSTEXIST, FileOpenDialog,
                    IFileOpenDialog, IShellItem, SHCreateItemFromParsingName, SIGDN_FILESYSPATH,
                },
            },
            core::{PCWSTR, w},
        };
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(|e| e.to_string())?;
        struct Com;
        impl Drop for Com {
            fn drop(&mut self) {
                unsafe { CoUninitialize() }
            }
        }
        let _com = Com;
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| e.to_string())?;
        dialog
            .SetOptions(
                dialog.GetOptions().map_err(|e| e.to_string())?
                    | FOS_FORCEFILESYSTEM
                    | FOS_FILEMUSTEXIST
                    | FOS_PATHMUSTEXIST
                    | FOS_NOCHANGEDIR
                    | FOS_DONTADDTORECENT,
            )
            .map_err(|e| e.to_string())?;
        let title: Vec<u16> = format!("연결 프로그램 선택 · {}", path.name())
            .encode_utf16()
            .chain(Some(0))
            .collect();
        dialog
            .SetTitle(PCWSTR(title.as_ptr()))
            .map_err(|e| e.to_string())?;
        dialog
            .SetOkButtonLabel(w!("이 프로그램으로 열기"))
            .map_err(|e| e.to_string())?;
        dialog
            .SetFileTypes(&[COMDLG_FILTERSPEC {
                pszName: w!("프로그램 (*.exe)"),
                pszSpec: w!("*.exe"),
            }])
            .map_err(|e| e.to_string())?;
        if let Some(folder) = std::env::var_os("ProgramFiles") {
            let mut value = NativePath::new(folder).0;
            value.push(0);
            if let Ok(item) =
                SHCreateItemFromParsingName::<_, _, IShellItem>(PCWSTR(value.as_ptr()), None)
            {
                let _ = dialog.SetDefaultFolder(&item);
            }
        }
        if let Err(e) = dialog.Show((owner != 0).then_some(HWND(owner as usize as *mut _))) {
            return if e.code().0 as u32 == 0x800704C7 {
                Ok(None)
            } else {
                Err(e.to_string())
            };
        }
        let item = dialog.GetResult().map_err(|e| e.to_string())?;
        let raw = item
            .GetDisplayName(SIGDN_FILESYSPATH)
            .map_err(|e| e.to_string())?;
        let mut len = 0;
        while *raw.0.add(len) != 0 {
            len += 1;
        }
        let executable = NativePath(std::slice::from_raw_parts(raw.0, len).to_vec());
        CoTaskMemFree(Some(raw.0.cast()));
        if !executable
            .to_path()
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
        {
            return Err("실행 파일(.exe)을 선택하세요.".into());
        }
        let child = std::process::Command::new(executable.to_path())
            .arg(path.to_path())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| e.to_string())?;
        Ok(Some(child.id()))
    }
    #[cfg(not(windows))]
    {
        let _ = (path, owner);
        Err("Windows 전용 기능입니다.".into())
    }
}
