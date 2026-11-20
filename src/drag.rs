use crate::model::NativePath;
pub fn run(paths: &[NativePath], owner: u64) -> Result<Option<u32>, String> {
    if paths.is_empty()
        || paths.len() > 500
        || paths.iter().map(|p| p.0.len()).sum::<usize>() > 50000
    {
        return Err("드래그는 1~500개 항목으로 나눠 주세요.".into());
    }
    #[cfg(windows)]
    unsafe {
        use windows::{
            Win32::{
                System::{
                    Com::IDataObject,
                    Ole::{
                        DROPEFFECT, DROPEFFECT_COPY, DoDragDrop, OleInitialize, OleUninitialize,
                    },
                },
                UI::Shell::{
                    Common::ITEMIDLIST, ILFree, IShellFolder, SHBindToParent, SHCreateDataObject,
                    SHParseDisplayName,
                },
            },
            core::PCWSTR,
        };
        OleInitialize(None).map_err(|e| e.to_string())?;
        struct Ole;
        impl Drop for Ole {
            fn drop(&mut self) {
                unsafe { OleUninitialize() }
            }
        }
        let _ole = Ole;
        struct Pidl(*mut ITEMIDLIST);
        impl Drop for Pidl {
            fn drop(&mut self) {
                unsafe { ILFree(Some(self.0)) }
            }
        }
        let parent = paths[0]
            .parent()
            .ok_or("드라이브 루트는 드래그할 수 없습니다.")?;
        if paths.iter().any(|p| p.parent().as_ref() != Some(&parent)) {
            return Err("같은 폴더 안의 항목을 선택하세요.".into());
        }
        let mut folder = std::ptr::null_mut();
        let mut text = parent.0.clone();
        text.push(0);
        SHParseDisplayName(PCWSTR(text.as_ptr()), None, &mut folder, 0, None)
            .map_err(|e| e.to_string())?;
        let folder = Pidl(folder);
        let mut owned = vec![];
        let mut children = vec![];
        for p in paths {
            let mut text = p.0.clone();
            text.push(0);
            let mut full = std::ptr::null_mut();
            SHParseDisplayName(PCWSTR(text.as_ptr()), None, &mut full, 0, None)
                .map_err(|e| e.to_string())?;
            let full = Pidl(full);
            let mut child = std::ptr::null_mut();
            let _: IShellFolder =
                SHBindToParent(full.0, Some(&mut child)).map_err(|e| e.to_string())?;
            children.push(child as *const ITEMIDLIST);
            owned.push(full);
        }
        let data: IDataObject =
            SHCreateDataObject(Some(folder.0), Some(&children), None::<&IDataObject>)
                .map_err(|e| e.to_string())?;
        // Copy-only: the external target owns transfer completion; SDIR never deletes originals.
        use windows::Win32::{
            Foundation::HWND,
            System::Threading::{AttachThreadInput, GetCurrentThreadId},
            UI::WindowsAndMessaging::GetWindowThreadProcessId,
        };
        let current = GetCurrentThreadId();
        let source = GetWindowThreadProcessId(HWND(owner as usize as *mut _), None);
        if owner == 0 || source == 0 {
            return Err("드래그를 시작한 창을 찾을 수 없습니다.".into());
        }
        AttachThreadInput(current, source, true)
            .ok()
            .map_err(|e| e.to_string())?;
        struct Attached(u32, u32);
        impl Drop for Attached {
            fn drop(&mut self) {
                unsafe {
                    let _ = AttachThreadInput(self.0, self.1, false);
                }
            }
        }
        let _attached = Attached(current, source);
        let source = source::make();
        let mut effect = DROPEFFECT::default();
        let result = DoDragDrop(&data, &source, DROPEFFECT_COPY, &mut effect);
        if result == windows::Win32::Foundation::DRAGDROP_S_CANCEL {
            return Ok(Some(0));
        }
        result.ok().map_err(|e| e.to_string())?;
        Ok(Some(effect.0))
    }
    #[cfg(not(windows))]
    {
        let _ = owner;
        Err("Windows 전용 기능입니다.".into())
    }
}

#[cfg(windows)]
mod source {
    use windows::{
        Win32::{
            Foundation::{DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS},
            System::{
                Ole::{DROPEFFECT, IDropSource, IDropSource_Impl},
                SystemServices::MODIFIERKEYS_FLAGS,
            },
            UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON},
        },
        core::{BOOL, HRESULT, implement},
    };
    #[implement(IDropSource)]
    struct Source;
    #[allow(non_snake_case)]
    impl IDropSource_Impl for Source_Impl {
        fn QueryContinueDrag(&self, escape: BOOL, _keys: MODIFIERKEYS_FLAGS) -> HRESULT {
            if escape.as_bool() {
                DRAGDROP_S_CANCEL
            } else if unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) } as u16 & 0x8000 == 0 {
                DRAGDROP_S_DROP
            } else {
                HRESULT(0)
            }
        }
        fn GiveFeedback(&self, _effect: DROPEFFECT) -> HRESULT {
            DRAGDROP_S_USEDEFAULTCURSORS
        }
    }
    pub(super) fn make() -> IDropSource {
        Source.into()
    }
}
