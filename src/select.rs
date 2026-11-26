//! Area selection (part of the Capture Engine): shows the frozen screenshot dimmed
//! over the monitor; the user drags a box (shown at full brightness). Mouse up =
//! capture, Esc or right-click = cancel. Focus goes back to the previous window.

use std::cell::RefCell;
use std::mem::size_of;
use std::sync::Once;

use windows::core::w;
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateDIBSection,
    CreateSolidBrush, DeleteDC, DeleteObject, EndPaint, FillRect, FrameRect, InvalidateRect,
    ScreenToClient, SelectObject, HDC,
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, PAINTSTRUCT, SRCCOPY,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, SetFocus, VK_ESCAPE};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetCursorPos, GetForegroundWindow,
    GetMessageW, LoadCursorW, RegisterClassW, SetForegroundWindow, ShowWindow, TranslateMessage,
    HMENU, IDC_CROSS, MSG, SW_SHOW, WM_ERASEBKGND, WM_HOTKEY, WM_KEYDOWN, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT, WM_RBUTTONDOWN, WNDCLASSW, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
};

#[derive(Default)]
struct Selecting {
    bright: HBITMAP,
    dim: HBITMAP,
    w: i32,
    h: i32,
    start: Option<(i32, i32)>,
    cur: (i32, i32),
    /// None = still selecting; Some(None) = cancelled; Some(Some(r)) = chosen.
    done: Option<Option<RECT>>,
}

thread_local! {
    static STATE: RefCell<Selecting> = RefCell::new(Selecting::default());
}

/// Let the user pick an area of the frozen screenshot. `area` is the monitor
/// rectangle; `bgra` is its pixels, rows top-down. Returns the chosen rectangle
/// in pixel coordinates of the screenshot, or None if cancelled.
pub fn select_area(area: RECT, w: u32, h: u32, bgra: &[u8]) -> Option<RECT> {
    unsafe {
        let previous = GetForegroundWindow();
        let dimmed: Vec<u8> = bgra
            .chunks_exact(4)
            .flat_map(|p| [p[0] / 2, p[1] / 2, p[2] / 2, 255])
            .collect();
        let (Some(bright), Some(dim)) = (make_bitmap(w, h, bgra), make_bitmap(w, h, &dimmed)) else {
            return None;
        };
        STATE.with(|s| {
            *s.borrow_mut() = Selecting { bright, dim, w: w as i32, h: h as i32, ..Default::default() }
        });

        static REGISTER: Once = Once::new();
        let hinst = GetModuleHandleW(None).map(|m| HINSTANCE(m.0)).unwrap_or_default();
        REGISTER.call_once(|| {
            RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(select_proc),
                hInstance: hinst,
                hCursor: LoadCursorW(None, IDC_CROSS).unwrap_or_default(),
                lpszClassName: w!("SnapRelaySelect"),
                ..Default::default()
            });
        });
        let Ok(hwnd) = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW, w!("SnapRelaySelect"), w!("SnapRelay area"), WS_POPUP,
            area.left, area.top, area.right - area.left, area.bottom - area.top,
            HWND::default(), HMENU::default(), hinst, None,
        ) else {
            cleanup();
            return None;
        };
        // Start the guide lines where the mouse already is.
        let mut pt = POINT::default();
        if GetCursorPos(&mut pt).is_ok() && ScreenToClient(hwnd, &mut pt).as_bool() {
            STATE.with(|s| s.borrow_mut().cur = (pt.x, pt.y));
        }
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(hwnd);

        // Our own little message loop until the user finishes. Hotkeys are ignored meanwhile.
        let mut msg = MSG::default();
        while STATE.with(|s| s.borrow().done.is_none()) {
            if GetMessageW(&mut msg, HWND::default(), 0, 0).0 <= 0 {
                break;
            }
            if msg.message == WM_HOTKEY {
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let result = STATE.with(|s| s.borrow().done).flatten();
        let _ = DestroyWindow(hwnd);
        cleanup();
        // Give focus back to where the user was (e.g. the chat box).
        if !previous.is_invalid() {
            let _ = SetForegroundWindow(previous);
        }
        result
    }
}

unsafe fn cleanup() {
    STATE.with(|s| {
        let s = s.borrow();
        let _ = DeleteObject(s.bright);
        let _ = DeleteObject(s.dim);
    });
}

/// A Windows bitmap holding these pixels (BGRA, rows top-down). Caller deletes it.
pub(crate) unsafe fn make_bitmap(w: u32, h: u32, bgra: &[u8]) -> Option<HBITMAP> {
    let bi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            biHeight: -(h as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits = std::ptr::null_mut();
    let bmp = CreateDIBSection(None, &bi, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
    std::ptr::copy_nonoverlapping(bgra.as_ptr(), bits.cast::<u8>(), bgra.len());
    Some(bmp)
}

/// Full-width guide lines through the mouse: white with a dark outline, so they
/// stay visible on any colour. (Windows' own crosshair inverts the colours under
/// it, which makes it vanish on the mid-grey of a dimmed white page.)
unsafe fn draw_guides(hdc: HDC, (x, y): (i32, i32), w: i32, h: i32) {
    let dark = CreateSolidBrush(COLORREF(0x0020_2020));
    let light = CreateSolidBrush(COLORREF(0x00FF_FFFF));
    FillRect(hdc, &RECT { left: 0, top: y - 1, right: w, bottom: y + 2 }, dark);
    FillRect(hdc, &RECT { left: x - 1, top: 0, right: x + 2, bottom: h }, dark);
    FillRect(hdc, &RECT { left: 0, top: y, right: w, bottom: y + 1 }, light);
    FillRect(hdc, &RECT { left: x, top: 0, right: x + 1, bottom: h }, light);
    let _ = DeleteObject(dark);
    let _ = DeleteObject(light);
}

fn point(lp: LPARAM) -> (i32, i32) {
    ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32)
}

fn normalized(a: (i32, i32), b: (i32, i32)) -> RECT {
    RECT { left: a.0.min(b.0), top: a.1.min(b.1), right: a.0.max(b.0), bottom: a.1.max(b.1) }
}

extern "system" fn select_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_LBUTTONDOWN => {
                let p = point(lp);
                STATE.with(|s| {
                    let mut s = s.borrow_mut();
                    s.start = Some(p);
                    s.cur = p;
                });
                SetCapture(hwnd);
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                // Repaint on every move so the guide lines follow the mouse.
                STATE.with(|s| s.borrow_mut().cur = point(lp));
                let _ = InvalidateRect(hwnd, None, false);
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let _ = ReleaseCapture();
                STATE.with(|s| {
                    let mut s = s.borrow_mut();
                    let Some(start) = s.start else { return };
                    let r = normalized(start, point(lp));
                    let r = RECT { left: r.left.max(0), top: r.top.max(0), right: r.right.min(s.w), bottom: r.bottom.min(s.h) };
                    // A click without a real drag counts as cancel.
                    s.done = Some(if r.right - r.left >= 5 && r.bottom - r.top >= 5 { Some(r) } else { None });
                });
                LRESULT(0)
            }
            WM_RBUTTONDOWN => {
                STATE.with(|s| s.borrow_mut().done = Some(None));
                LRESULT(0)
            }
            WM_KEYDOWN if wp.0 as u16 == VK_ESCAPE.0 => {
                STATE.with(|s| s.borrow_mut().done = Some(None));
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1), // we paint everything; skipping this avoids flicker
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);
                STATE.with(|s| {
                    let s = s.borrow();
                    // Draw into an off-screen buffer first, then copy once: no flicker.
                    let buf_dc = CreateCompatibleDC(hdc);
                    let buf = CreateCompatibleBitmap(hdc, s.w, s.h);
                    let old_buf = SelectObject(buf_dc, buf);
                    let src = CreateCompatibleDC(hdc);
                    let old_src = SelectObject(src, s.dim);
                    let _ = BitBlt(buf_dc, 0, 0, s.w, s.h, src, 0, 0, SRCCOPY);
                    if let Some(start) = s.start {
                        let r = normalized(start, s.cur);
                        SelectObject(src, s.bright);
                        let _ = BitBlt(buf_dc, r.left, r.top, r.right - r.left, r.bottom - r.top, src, r.left, r.top, SRCCOPY);
                        let border = CreateSolidBrush(COLORREF(0x0084_C3E5)); // brand Warm Champagne #E5C384
                        FrameRect(buf_dc, &r, border);
                        let inner = RECT { left: r.left + 1, top: r.top + 1, right: r.right - 1, bottom: r.bottom - 1 };
                        FrameRect(buf_dc, &inner, border);
                        let _ = DeleteObject(border);
                    }
                    draw_guides(buf_dc, s.cur, s.w, s.h);
                    let _ = BitBlt(hdc, 0, 0, s.w, s.h, buf_dc, 0, 0, SRCCOPY);
                    SelectObject(src, old_src);
                    let _ = DeleteDC(src);
                    SelectObject(buf_dc, old_buf);
                    let _ = DeleteObject(buf);
                    let _ = DeleteDC(buf_dc);
                });
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}
