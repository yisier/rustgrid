//! Restores native window-edge resizing for the custom titlebar on Windows.
//!
//! gpui hides the OS titlebar, so `WM_NCCALCSIZE` removes the standard frame and
//! `DefWindowProc` only reports a 1px resize border. On top of that, the titlebar's
//! `WindowControlArea::Drag` claims the top edge before gpui checks for a resize area. We
//! install a window subclass that answers `WM_NCHITTEST` with the proper resize codes,
//! computed from the system frame metrics, before gpui sees the message.

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::Gdi::ScreenToClient,
    UI::{
        HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi},
        Shell::{DefSubclassProc, SetWindowSubclass},
        WindowsAndMessaging::{
            GetClientRect, HTBOTTOM, HTBOTTOMLEFT, HTBOTTOMRIGHT, HTLEFT, HTRIGHT, HTTOP,
            HTTOPLEFT, HTTOPRIGHT, IsZoomed, SM_CXPADDEDBORDER, SM_CXSIZEFRAME, SM_CYSIZEFRAME,
            WM_NCHITTEST,
        },
    },
};

const SUBCLASS_ID: usize = 1;
const MIN_GRAB_THICKNESS: i32 = 6;

pub fn install(window: &gpui::Window) {
    let hwnd = match <gpui::Window as HasWindowHandle>::window_handle(window) {
        Ok(handle) => match handle.as_raw() {
            RawWindowHandle::Win32(handle) => HWND(handle.hwnd.get() as *mut core::ffi::c_void),
            _ => return,
        },
        Err(_) => return,
    };
    unsafe {
        let _ = SetWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID, 0);
    }
}

unsafe extern "system" fn subclass_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    _ref_data: usize,
) -> LRESULT {
    if message == WM_NCHITTEST
        && let Some(code) = unsafe { resize_hit_test(hwnd, lparam) }
    {
        return LRESULT(code as isize);
    }
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

unsafe fn resize_hit_test(hwnd: HWND, lparam: LPARAM) -> Option<u32> {
    if unsafe { IsZoomed(hwnd) }.as_bool() {
        return None;
    }

    let dpi = unsafe { GetDpiForWindow(hwnd) };
    let min_grab = ((MIN_GRAB_THICKNESS as f32) * (dpi as f32 / 96.0)).round() as i32;
    let border_x = (unsafe { GetSystemMetricsForDpi(SM_CXSIZEFRAME, dpi) }
        + unsafe { GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi) })
    .max(min_grab);
    let border_y = (unsafe { GetSystemMetricsForDpi(SM_CYSIZEFRAME, dpi) }
        + unsafe { GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi) })
    .max(min_grab);

    let mut point = POINT {
        x: (lparam.0 & 0xffff) as u16 as i16 as i32,
        y: (((lparam.0 >> 16) & 0xffff) as u16 as i16) as i32,
    };
    unsafe {
        let _ = ScreenToClient(hwnd, &mut point);
    };

    let mut rect = RECT::default();
    unsafe { GetClientRect(hwnd, &mut rect) }.ok()?;

    let on_left = point.x < border_x;
    let on_right = point.x >= rect.right - border_x;
    let on_top = point.y < border_y;
    let on_bottom = point.y >= rect.bottom - border_y;

    match (on_top, on_bottom, on_left, on_right) {
        (true, _, true, _) => Some(HTTOPLEFT),
        (true, _, _, true) => Some(HTTOPRIGHT),
        (_, true, true, _) => Some(HTBOTTOMLEFT),
        (_, true, _, true) => Some(HTBOTTOMRIGHT),
        (true, _, _, _) => Some(HTTOP),
        (_, true, _, _) => Some(HTBOTTOM),
        (_, _, true, _) => Some(HTLEFT),
        (_, _, _, true) => Some(HTRIGHT),
        _ => None,
    }
}
