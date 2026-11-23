use super::DragTicket;
use std::{
    cell::RefCell,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    process::Child,
    sync::{Arc, Weak},
};
use windows::{
    Win32::{
        Foundation::{
            DUPLICATE_HANDLE_OPTIONS, DuplicateHandle, HANDLE, HINSTANCE, LPARAM, LRESULT,
            WAIT_OBJECT_0, WAIT_TIMEOUT, WPARAM,
        },
        System::{
            LibraryLoader::GetModuleHandleW,
            SystemServices::{MK_LBUTTON, MK_RBUTTON, MODIFIERKEYS_FLAGS},
            Threading::{
                CreateEventW, GetCurrentProcess, SYNCHRONIZATION_SYNCHRONIZE, SetEvent,
                WaitForSingleObject,
            },
        },
        UI::{
            Input::KeyboardAndMouse::{GetAsyncKeyState, VK_ESCAPE, VK_LBUTTON, VK_RBUTTON},
            WindowsAndMessaging::{
                CallNextHookEx, GetSystemMetrics, HC_ACTION, HHOOK, KBDLLHOOKSTRUCT, SM_SWAPBUTTON,
                SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN,
                WM_LBUTTONDOWN, WM_LBUTTONUP, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN,
            },
        },
    },
    core::PCWSTR,
};

thread_local! {
    static CAPTURE: RefCell<Weak<GestureEvents>> = RefCell::new(Weak::new());
}

pub struct MouseGesture {
    events: Arc<GestureEvents>,
    mouse: Option<HHOOK>,
    keyboard: Option<HHOOK>,
}

impl MouseGesture {
    /// Install on the desktop event thread at the file-row mouse-down.
    pub fn capture() -> Result<Option<Self>, String> {
        if CAPTURE.with(|current| current.borrow().strong_count() != 0) {
            return Ok(None);
        }
        let right = unsafe { GetSystemMetrics(SM_SWAPBUTTON) != 0 };
        if !pressed(right) {
            return Ok(None);
        }
        let mut gesture = Self {
            events: Arc::new(GestureEvents::new(right)?),
            mouse: None,
            keyboard: None,
        };
        let module = unsafe { GetModuleHandleW(PCWSTR::null()) }.map_err(|e| e.to_string())?;
        let instance = Some(HINSTANCE(module.0));
        CAPTURE.with(|current| *current.borrow_mut() = Arc::downgrade(&gesture.events));
        gesture.mouse = Some(
            unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse), instance, 0) }
                .map_err(|e| e.to_string())?,
        );
        gesture.keyboard = Some(
            unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard), instance, 0) }
                .map_err(|e| e.to_string())?,
        );
        Ok(gesture.events.ready().then_some(gesture))
    }

    pub fn events(&self) -> Arc<GestureEvents> {
        self.events.clone()
    }
}

impl Drop for MouseGesture {
    fn drop(&mut self) {
        self.events.abort();
        for hook in [self.mouse, self.keyboard].into_iter().flatten() {
            let _ = unsafe { UnhookWindowsHookEx(hook) };
        }
        CAPTURE.with(|current| *current.borrow_mut() = Weak::new());
    }
}

unsafe extern "system" fn mouse(code: i32, message: WPARAM, data: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let message = message.0 as u32;
        if matches!(
            message,
            WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONDOWN | WM_RBUTTONUP
        ) {
            CAPTURE.with(|slot| {
                if let Some(events) = slot.borrow().upgrade() {
                    events.observe_mouse(message);
                }
            });
        }
    }
    unsafe { CallNextHookEx(None, code, message, data) }
}

unsafe extern "system" fn keyboard(code: i32, message: WPARAM, data: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32
        && matches!(message.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN)
        && unsafe { (*(data.0 as *const KBDLLHOOKSTRUCT)).vkCode } == u32::from(VK_ESCAPE.0)
    {
        CAPTURE.with(|slot| {
            if let Some(events) = slot.borrow().upgrade() {
                events.abort();
            }
        });
    }
    unsafe { CallNextHookEx(None, code, message, data) }
}

fn raw(handle: &OwnedHandle) -> HANDLE {
    HANDLE(handle.as_raw_handle())
}

fn event() -> Result<OwnedHandle, String> {
    let handle =
        unsafe { CreateEventW(None, true, false, PCWSTR::null()) }.map_err(|e| e.to_string())?;
    Ok(unsafe { OwnedHandle::from_raw_handle(handle.0) })
}

fn pressed(right: bool) -> bool {
    let button = if right { VK_RBUTTON } else { VK_LBUTTON };
    unsafe { GetAsyncKeyState(i32::from(button.0)) < 0 }
}

pub struct GestureEvents {
    released: OwnedHandle,
    aborted: OwnedHandle,
    right: bool,
}

impl GestureEvents {
    fn new(right: bool) -> Result<Self, String> {
        Ok(Self {
            released: event()?,
            aborted: event()?,
            right,
        })
    }

    pub fn ready(&self) -> bool {
        self.accepts_start(pressed(self.right))
    }

    fn accepts_start(&self, pressed: bool) -> bool {
        pressed && !self.released() && !self.aborted()
    }

    fn observe_mouse(&self, message: u32) {
        let (down, up) = if self.right {
            (WM_RBUTTONDOWN, WM_RBUTTONUP)
        } else {
            (WM_LBUTTONDOWN, WM_LBUTTONUP)
        };
        if message == up {
            let _ = unsafe { SetEvent(raw(&self.released)) };
        } else if message == down && self.released() {
            self.abort();
        }
    }

    pub fn released(&self) -> bool {
        unsafe { WaitForSingleObject(raw(&self.released), 0) == WAIT_OBJECT_0 }
    }

    pub fn aborted(&self) -> bool {
        unsafe { WaitForSingleObject(raw(&self.aborted), 0) != WAIT_TIMEOUT }
    }

    pub fn still_pressed(&self, keys: MODIFIERKEYS_FLAGS) -> bool {
        let button = if self.right { MK_RBUTTON } else { MK_LBUTTON };
        keys.0 & button.0 != 0
    }

    fn abort(&self) {
        let _ = unsafe { SetEvent(raw(&self.aborted)) };
    }

    pub fn duplicate_for(&self, child: &Child) -> Result<DragTicket, String> {
        self.duplicate_to(HANDLE(child.as_raw_handle()))
    }

    fn duplicate_to(&self, process: HANDLE) -> Result<DragTicket, String> {
        let duplicate = |handle: &OwnedHandle| {
            let mut result = HANDLE::default();
            unsafe {
                DuplicateHandle(
                    GetCurrentProcess(),
                    raw(handle),
                    process,
                    &mut result,
                    SYNCHRONIZATION_SYNCHRONIZE.0,
                    false,
                    DUPLICATE_HANDLE_OPTIONS(0),
                )
            }
            .map_err(|e| e.to_string())?;
            Ok::<_, String>(result.0 as usize as u64)
        };
        Ok(DragTicket {
            released: duplicate(&self.released)?,
            aborted: duplicate(&self.aborted)?,
            right: self.right,
        })
    }

    // The worker owns these unnamed event handles after the parent duplicates them into it.
    pub(super) fn from_ticket(ticket: &DragTicket) -> Result<Self, String> {
        if ticket.released == 0 || ticket.aborted == 0 || ticket.released == ticket.aborted {
            return Err("드래그 입력을 확인하지 못했습니다.".into());
        }
        Ok(Self {
            released: unsafe { OwnedHandle::from_raw_handle(ticket.released as usize as *mut _) },
            aborted: unsafe { OwnedHandle::from_raw_handle(ticket.aborted as usize as *mut _) },
            right: ticket.right,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_observes_release_and_abort_through_duplicated_events() {
        let parent = GestureEvents::new(false).unwrap();
        let ticket = parent.duplicate_to(unsafe { GetCurrentProcess() }).unwrap();
        let worker = GestureEvents::from_ticket(&ticket).unwrap();
        assert!(worker.accepts_start(true));
        parent.observe_mouse(WM_LBUTTONUP);
        assert!(worker.released());
        assert!(!worker.aborted());
        assert!(!worker.accepts_start(true));
        parent.observe_mouse(WM_LBUTTONDOWN);
        assert!(worker.aborted());
        assert!(!worker.accepts_start(true));
    }
}
