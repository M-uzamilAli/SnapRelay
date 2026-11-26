//! On-screen text notices in the bottom-right corner ("Captured · 3 in queue",
//! "Sending 2 of 5…", "5 of 5 delivered"). Click-through and never takes focus,
//! so it can't pull focus away from the chat box we paste into.

use std::cell::{Cell, RefCell};
use std::mem::size_of;
use std::sync::Once;

use windows::core::w;
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateRoundRectRgn, CreateSolidBrush, DeleteObject, DrawTextW,
    EndPaint, FillRect, GetDC, GetMonitorInfoW, InvalidateRect, MonitorFromPoint, ReleaseDC,
    SelectObject, SetBkMode, SetTextColor, SetWindowRgn, UpdateWindow, CLEARTYPE_QUALITY,
    CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DT_CALCRECT, DT_NOPREFIX, DT_WORDBREAK, FW_SEMIBOLD,
    HFONT, MONITORINFO, MONITOR_DEFAULTTONEAREST, OUT_DEFAULT_PRECIS, PAINTSTRUCT, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetClientRect, GetCursorPos, KillTimer, RegisterClassW,
    MoveWindow, SetLayeredWindowAttributes, SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, SetTimer, SetWindowPos, ShowWindow, HMENU,
    HTTRANSPARENT, HWND_TOPMOST, LWA_ALPHA, SWP_NOACTIVATE, SWP_SHOWWINDOW, SW_HIDE,
    SW_SHOWNOACTIVATE, WM_DPICHANGED, WM_NCHITTEST, WM_PAINT,
    WM_TIMER, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT, WS_POPUP,
};

const HIDE_TIMER: usize = 1;
const BG: COLORREF = COLORREF(0x0025_281C); // 0x00BBGGRR: brand Slate-Forest #1C2825
const FG: COLORREF = COLORREF(0x00F4_F6F5); // Linen #F5F6F4

thread_local! {
    static WINDOW: Cell<Option<HWND>> = const { Cell::new(None) };
    static TEXT: RefCell<Vec<u16>> = const { RefCell::new(Vec::new()) };
    static SCALE: Cell<u32> = const { Cell::new(96) };
}

/// Show `text` for `ms` milliseconds, replacing any notice already showing.
pub fn show(text: &str, ms: u32) {
    unsafe {
        let hwnd = window();
        TEXT.with(|t| *t.borrow_mut() = text.encode_utf16().collect());

        // Place it on the monitor the mouse is on, above the taskbar.
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut info);
        let (mut dpi, mut _y) = (96u32, 96u32);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dpi, &mut _y);
        SCALE.with(|s| s.set(dpi));
        let px = |v: i32| v * dpi as i32 / 96;

        // Measure the text to size the box.
        let hdc = GetDC(hwnd);
        let font = make_font(dpi);
        let old = SelectObject(hdc, font);
        let mut rc = RECT { left: 0, top: 0, right: px(440), bottom: 0 };
        TEXT.with(|t| {
            let mut t = t.borrow().clone();
            DrawTextW(hdc, &mut t, &mut rc, DT_CALCRECT | DT_WORDBREAK | DT_NOPREFIX);
        });
        SelectObject(hdc, old);
        let _ = DeleteObject(font);
        ReleaseDC(hwnd, hdc);

        let work = info.rcWork;
        let w = (rc.right + px(36)).min(work.right - work.left - px(40));
        let h = rc.bottom + px(24);
        let (mut x, mut y) = (work.right - w - px(20), work.bottom - h - px(20));
        // Stack above the queue window if it's open on this monitor.
        if let Some(q) = crate::queue::visible_rect() {
            if q.left < work.right && q.right > work.left && q.top < work.bottom && q.bottom > work.top {
                x = q.right - w;
                y = q.top - h - px(10);
            }
        }
        let _ = KillTimer(hwnd, HIDE_TIMER);
        if SetWindowPos(hwnd, HWND_TOPMOST, x, y, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW).is_err() {
            // Fallback so the box never keeps an old, smaller size.
            let _ = MoveWindow(hwnd, x, y, w, h, false);
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        SetWindowRgn(hwnd, CreateRoundRectRgn(0, 0, w + 1, h + 1, px(14), px(14)), true);
        let _ = InvalidateRect(hwnd, None, true);
        let _ = UpdateWindow(hwnd); // paint now, even if we're busy releasing
        SetTimer(hwnd, HIDE_TIMER, ms, None);
    }
}

unsafe fn make_font(dpi: u32) -> HFONT {
    CreateFontW(
        -(15 * dpi as i32 / 96), 0, 0, 0, FW_SEMIBOLD.0 as i32, 0, 0, 0,
        DEFAULT_CHARSET.0 as u32, OUT_DEFAULT_PRECIS.0 as u32, CLIP_DEFAULT_PRECIS.0 as u32,
        CLEARTYPE_QUALITY.0 as u32, 0,
        w!("Segoe UI"),
    )
}

unsafe fn window() -> HWND {
    if let Some(h) = WINDOW.with(|w| w.get()) {
        return h;
    }
    static REGISTER: Once = Once::new();
    let hinst = GetModuleHandleW(None).map(|m| HINSTANCE(m.0)).unwrap_or_default();
    REGISTER.call_once(|| {
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(notice_proc),
            hInstance: hinst,
            lpszClassName: w!("SnapRelayNotice"),
            ..Default::default()
        });
    });
    let hwnd = CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
        w!("SnapRelayNotice"), w!(""), WS_POPUP,
        0, 0, 10, 10, HWND::default(), HMENU::default(), hinst, None,
    )
    .expect("couldn't create the notice window");
    let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 240, LWA_ALPHA);
    // Keep SnapRelay's own notices out of the screenshots it takes.
    // (Set SNAPRELAY_VISIBLE_IN_SCREENSHOTS=1 to turn this off, e.g. to take screenshots for docs/tests.)
    if std::env::var_os("SNAPRELAY_VISIBLE_IN_SCREENSHOTS").is_none() {
        let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
    }
    WINDOW.with(|w| w.set(Some(hwnd)));
    hwnd
}

extern "system" fn notice_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                let bg = CreateSolidBrush(BG);
                FillRect(hdc, &rc, bg);
                let _ = DeleteObject(bg);

                let dpi = SCALE.with(|s| s.get());
                let px = |v: i32| v * dpi as i32 / 96;
                let font = make_font(dpi);
                let old = SelectObject(hdc, font);
                SetBkMode(hdc, TRANSPARENT);
                SetTextColor(hdc, FG);
                let mut text_rc = RECT { left: px(18), top: px(12), right: rc.right - px(18), bottom: rc.bottom - px(12) };
                TEXT.with(|t| {
                    let mut t = t.borrow().clone();
                    DrawTextW(hdc, &mut t, &mut text_rc, DT_WORDBREAK | DT_NOPREFIX);
                });
                SelectObject(hdc, old);
                let _ = DeleteObject(font);
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_TIMER if wp.0 == HIDE_TIMER => {
                let _ = KillTimer(hwnd, HIDE_TIMER);
                let _ = ShowWindow(hwnd, SW_HIDE);
                LRESULT(0)
            }
            WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
            // We size ourselves for the monitor's scale; don't let Windows resize us.
            WM_DPICHANGED => LRESULT(0),
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}
