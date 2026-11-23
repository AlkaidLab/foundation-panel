//! Touch support for the tray icon's right-click menu.
//!
//! A touch long-press on a notification icon does not produce the WM_RBUTTONUP that the
//! tray-icon crate listens for: the shell sends WM_CONTEXTMENU, and tray-icon (0.26.1 and older)
//! has no arm for it and emits no event, so the gesture never opened the Material menu on a
//! touch screen (a mouse right-click still went through WM_RBUTTONUP as usual). This module
//! subclasses tray-icon's message window (class tray_icon_app, this process only) and turns
//! WM_CONTEXTMENU into the tray callback message with WM_RBUTTONUP — the exact shape a real
//! mouse right-click delivers — so the usual event path (popup_input::on_icon_event) runs
//! unchanged.

use log::info;
use std::sync::atomic::{AtomicIsize, Ordering};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, DefWindowProcW, FindWindowExW, GetWindowThreadProcessId, PostMessageW,
    SetWindowLongPtrW, GWLP_WNDPROC, WM_CONTEXTMENU, WM_RBUTTONUP,
};

/// The taskbar callback message tray-icon registers for its icons. The constant is private in
/// the crate but fixed there (6002) and pinned by Cargo.lock; a change upstream needs the same
/// change here.
const TRAY_CALLBACK_MSG: u32 = 6002;

/// The message window this process has hooked, 0 when none.
static HOOKED: AtomicIsize = AtomicIsize::new(0);
/// The window procedure that was on the window before the hook.
static ORIGINAL: AtomicIsize = AtomicIsize::new(0);

/// Installs the WM_CONTEXTMENU hook on the tray message window when it is not hooked yet. Cheap
/// to call repeatedly: it re-hooks only when tray-icon makes a new window.
pub fn ensure_installed() {
    let Some(hwnd) = find_tray_message_window() else { return };
    if HOOKED.load(Ordering::Relaxed) == hwnd.0 as isize {
        return;
    }
    let previous = unsafe { SetWindowLongPtrW(hwnd, GWLP_WNDPROC, hook_proc as *const () as isize) };
    if previous == 0 {
        return;
    }
    ORIGINAL.store(previous, Ordering::Relaxed);
    HOOKED.store(hwnd.0 as isize, Ordering::Relaxed);
    info!("[tray-touch] WM_CONTEXTMENU hook installed on the tray message window");
}

/// The message window of this process's tray icons: every application built on the crate makes
/// a tray_icon_app window, so the search filters by process id.
fn find_tray_message_window() -> Option<HWND> {
    let mut hwnd = HWND::default();
    for _ in 0..64 {
        hwnd = unsafe { FindWindowExW(None, Some(hwnd), w!("tray_icon_app"), PCWSTR::null()) }.unwrap_or_default();
        if hwnd.is_invalid() {
            return None;
        }
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if pid == std::process::id() {
            return Some(hwnd);
        }
    }
    None
}

unsafe extern "system" fn hook_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if message == WM_CONTEXTMENU {
            // Replay the gesture as the tray callback a real mouse right-click delivers, and
            // stop here: tray-icon has no arm for WM_CONTEXTMENU and the default proc does
            // nothing.
            let _ = PostMessageW(Some(hwnd), TRAY_CALLBACK_MSG, WPARAM(0), LPARAM(WM_RBUTTONUP as isize));
            return LRESULT(0);
        }
        let original = ORIGINAL.load(Ordering::Relaxed);
        if original == 0 {
            return DefWindowProcW(hwnd, message, wparam, lparam);
        }
        CallWindowProcW(
            Some(std::mem::transmute::<
                isize,
                unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT,
            >(original)),
            hwnd,
            message,
            wparam,
            lparam,
        )
    }
}

