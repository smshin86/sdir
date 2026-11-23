use crate::model::NativePath;

#[cfg(windows)]
mod gesture;
#[cfg(windows)]
pub use gesture::{GestureEvents, MouseGesture};

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct DragTicket {
    released: u64,
    aborted: u64,
    right: bool,
}

#[cfg(windows)]
pub fn same_volume(source: &NativePath, destination: &NativePath) -> bool {
    use windows::{
        Win32::Storage::FileSystem::{GetVolumeNameForVolumeMountPointW, GetVolumePathNameW},
        core::PCWSTR,
    };

    fn volume(path: &NativePath) -> Option<String> {
        let mut path = path.0.clone();
        // UNC providers determine their own copy/move behavior.
        if path.first() == Some(&92) && path.get(1) == Some(&92) {
            return None;
        }
        path.push(0);
        let mut mount = vec![0u16; 32768];
        unsafe { GetVolumePathNameW(PCWSTR(path.as_ptr()), &mut mount) }.ok()?;
        let mut name = [0u16; 260];
        unsafe { GetVolumeNameForVolumeMountPointW(PCWSTR(mount.as_ptr()), &mut name) }.ok()?;
        let length = name.iter().position(|&unit| unit == 0)?;
        if length == 0 {
            return None;
        }
        String::from_utf16(&name[..length]).ok()
    }

    match (volume(source), volume(destination)) {
        (Some(source), Some(destination)) => source.eq_ignore_ascii_case(&destination),
        _ => false,
    }
}

#[cfg(windows)]
#[derive(PartialEq, Eq)]
pub enum CursorWindow {
    Own,
    Other,
    Unknown,
}

#[cfg(windows)]
pub fn cursor_window(owner: u64) -> CursorWindow {
    use windows::Win32::{
        Foundation::{HWND, POINT},
        UI::WindowsAndMessaging::{
            GA_ROOT, GA_ROOTOWNER, GetAncestor, GetCursorPos, IsWindow, WindowFromPoint,
        },
    };
    let owner = HWND(owner as usize as *mut _);
    let mut point = POINT::default();
    unsafe {
        if !IsWindow(Some(owner)).as_bool() || GetCursorPos(&mut point).is_err() {
            return CursorWindow::Unknown;
        }
        let root = GetAncestor(owner, GA_ROOT);
        let hit = WindowFromPoint(point);
        if root.0.is_null() || hit.0.is_null() {
            return CursorWindow::Unknown;
        }
        if GetAncestor(hit, GA_ROOT) == root || GetAncestor(hit, GA_ROOTOWNER) == root {
            CursorWindow::Own
        } else {
            CursorWindow::Other
        }
    }
}

#[cfg(windows)]
fn shift_only_pressed() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, VK_CONTROL, VK_MENU, VK_SHIFT,
    };

    unsafe {
        GetAsyncKeyState(i32::from(VK_SHIFT.0)) < 0
            && GetAsyncKeyState(i32::from(VK_CONTROL.0)) >= 0
            && GetAsyncKeyState(i32::from(VK_MENU.0)) >= 0
    }
}

pub fn run(paths: &[NativePath], owner: u64, ticket: &DragTicket) -> Result<Option<u32>, String> {
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
                        DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_MOVE, DoDragDrop, OleInitialize,
                        OleUninitialize,
                    },
                },
                UI::Shell::{
                    Common::ITEMIDLIST, ILFree, IShellFolder, SHBindToParent, SHCreateDataObject,
                    SHParseDisplayName,
                },
            },
            core::PCWSTR,
        };
        let events = gesture::GestureEvents::from_ticket(ticket)?;
        let shift_move = !ticket.right && shift_only_pressed();
        if !events.ready() || cursor_window(owner) != CursorWindow::Other {
            return Ok(Some(crate::utility::SDIR_DRAG_CANCELLED));
        }
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
        // The external target owns transfer completion; SDIR never deletes originals.
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
        if shift_move && !shift_only_pressed() {
            return Ok(Some(crate::utility::SDIR_DRAG_CANCELLED));
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
        if !events.ready()
            || cursor_window(owner) != CursorWindow::Other
            || (shift_move && !shift_only_pressed())
        {
            return Ok(Some(crate::utility::SDIR_DRAG_CANCELLED));
        }
        let source = source::make(owner, events, shift_move);
        let mut effect = DROPEFFECT::default();
        let allowed_effects = if shift_move {
            DROPEFFECT(DROPEFFECT_COPY.0 | DROPEFFECT_MOVE.0)
        } else {
            DROPEFFECT_COPY
        };
        let result = DoDragDrop(&data, &source, allowed_effects, &mut effect);
        if result == windows::Win32::Foundation::DRAGDROP_S_CANCEL {
            return Ok(Some(crate::utility::SDIR_DRAG_CANCELLED));
        }
        if result != windows::Win32::Foundation::DRAGDROP_S_DROP {
            result.ok().map_err(|e| e.to_string())?;
            return Ok(Some(crate::utility::SDIR_DRAG_CANCELLED));
        }
        let verified_move = shift_move
            && effect.0 & DROPEFFECT_MOVE.0 != 0
            && paths
                .iter()
                .all(|path| matches!(path.to_path().try_exists(), Ok(false)));
        Ok(Some(if verified_move {
            crate::utility::SDIR_DRAG_VERIFIED_MOVE
        } else {
            crate::utility::SDIR_DRAG_EXTERNAL_HANDOFF
        }))
    }
    #[cfg(not(windows))]
    {
        let _ = (owner, ticket);
        Err("Windows 전용 기능입니다.".into())
    }
}

#[cfg(windows)]
mod source {
    use super::{CursorWindow, GestureEvents, cursor_window};
    use std::cell::Cell;
    use windows::{
        Win32::{
            Foundation::{DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS},
            System::{
                Ole::{DROPEFFECT, IDropSource, IDropSource_Impl},
                SystemServices::{MK_CONTROL, MK_SHIFT, MODIFIERKEYS_FLAGS},
            },
            UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_MENU},
        },
        core::{BOOL, HRESULT, implement},
    };
    #[implement(IDropSource)]
    struct Source {
        owner: u64,
        events: GestureEvents,
        shift_move: bool,
        started: Cell<bool>,
    }
    #[allow(non_snake_case)]
    impl IDropSource_Impl for Source_Impl {
        fn QueryContinueDrag(&self, escape: BOOL, keys: MODIFIERKEYS_FLAGS) -> HRESULT {
            if escape.as_bool() || self.events.aborted() {
                return DRAGDROP_S_CANCEL;
            }
            let held = self.events.still_pressed(keys);
            if self.shift_move
                && (keys.0 & MK_SHIFT.0 == 0
                    || keys.0 & MK_CONTROL.0 != 0
                    || unsafe { GetAsyncKeyState(i32::from(VK_MENU.0)) < 0 })
            {
                return DRAGDROP_S_CANCEL;
            }
            if !self.started.get() {
                if !held || !self.events.ready() {
                    return DRAGDROP_S_CANCEL;
                }
                self.started.set(true);
            }
            if held {
                HRESULT(0)
            } else if self.events.released() && cursor_window(self.owner) == CursorWindow::Other {
                DRAGDROP_S_DROP
            } else {
                DRAGDROP_S_CANCEL
            }
        }
        fn GiveFeedback(&self, _effect: DROPEFFECT) -> HRESULT {
            DRAGDROP_S_USEDEFAULTCURSORS
        }
    }
    pub(super) fn make(owner: u64, events: GestureEvents, shift_move: bool) -> IDropSource {
        Source {
            owner,
            events,
            shift_move,
            started: Cell::new(false),
        }
        .into()
    }
}
