//! Shortcut display for demo videos: when you press one of SnapRelay's shortcuts
//! in recording mode, a strip at the bottom of the screen shows the keys and what
//! they did ("Win + Shift + D   Capture screen"). Click-through, never takes focus.

use std::cell::Cell;
use std::mem::size_of;
use std::sync::Once;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateFontW, CreateSolidBrush, DeleteObject, DrawTextW, FillRect, GetDC, GetMonitorInfoW,
    MonitorFromPoint, ReleaseDC, SelectObject, SetBkMode, SetTextColor, CLEARTYPE_QUALITY,
    CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DRAW_TEXT_FORMAT, DT_CALCRECT, DT_CENTER, DT_LEFT,
    DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, FW_NORMAL, FW_SEMIBOLD, HDC, HFONT, MONITORINFO,
    MONITOR_DEFAULTTONEAREST, OUT_DEFAULT_PRECIS, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetCursorPos, KillTimer, RegisterClassW, SetTimer, SetWindowPos,
    ShowWindow, HMENU, HTTRANSPARENT, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SWP_SHOWWINDOW, SW_HIDE, WM_DPICHANGED, WM_NCHITTEST, WM_TIMER, WNDCLASSW, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use crate::fonts::{REGULAR_FACE, SEMIBOLD_FACE};

const HIDE_TIMER: usize = 1;
const RAISE_TIMER: usize = 2;
const SHOW_MS: u32 = 1800;

// Brand colours (0x00BBGGRR).
const BG: COLORREF = COLORREF(0x0025_281C); // Slate-Forest #1C2825
const KEY_FILL: COLORREF = COLORREF(0x003B_402E); // #2E403B
const KEY_EDGE: COLORREF = COLORREF(0x006C_784B); // Muted Sage #4B786C
const KEY_TEXT: COLORREF = COLORREF(0x00F4_F6F5); // Linen #F5F6F4
const PLUS: COLORREF = COLORREF(0x00B2_B8A9); // #A9B8B2
const ACTION: COLORREF = COLORREF(0x0084_C3E5); // Warm Champagne #E5C384

thread_local! {
    static WINDOW: Cell<Option<HWND>> = const { Cell::new(None) };
}

fn px(dpi: u32, v: i32) -> i32 {
    v * dpi as i32 / 96
}

struct Layout {
    w: i32,
    h: i32,
    caps: Vec<i32>,
    plus: i32,
}

/// Show "Win + Shift + D   Capture screen" for a moment (recording mode only).
pub fn show(shortcut: &str, action: &str) {
    if !crate::recording::is_on() {
        return;
    }
    let keys: Vec<&str> = shortcut.split('+').map(str::trim).collect();
    unsafe {
        let hwnd = window();
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut info);
        let (mut dpi, mut _y) = (96u32, 96u32);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dpi, &mut _y);

        let (key_font, action_font) = (font(dpi, 15, FW_SEMIBOLD.0, SEMIBOLD_FACE), font(dpi, 15, FW_NORMAL.0, REGULAR_FACE));
        let l = measure(hwnd, dpi, &keys, action, key_font, action_font);
        // Bottom centre, above the taskbar: where viewers look for it.
        let work = info.rcWork;
        let (x, y) = ((work.left + work.right - l.w) / 2, work.bottom - l.h - px(dpi, 56));
        let _ = KillTimer(hwnd, HIDE_TIMER);
        let _ = SetWindowPos(hwnd, HWND_TOPMOST, x, y, l.w, l.h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
        crate::smooth::present_rounded(hwnd, l.w, l.h, px(dpi, 14) as f32, None, |hdc| {
            draw(hdc, dpi, &l, &keys, action, key_font, action_font)
        });
        let _ = DeleteObject(key_font);
        let _ = DeleteObject(action_font);
        SetTimer(hwnd, HIDE_TIMER, SHOW_MS, None);
        // Stay above full-screen overlays (like the area picker) while showing.
        SetTimer(hwnd, RAISE_TIMER, 100, None);
    }
}

unsafe fn font(dpi: u32, size: i32, weight: u32, face: PCWSTR) -> HFONT {
    CreateFontW(
        -px(dpi, size), 0, 0, 0, weight as i32, 0, 0, 0,
        DEFAULT_CHARSET.0 as u32, OUT_DEFAULT_PRECIS.0 as u32, CLIP_DEFAULT_PRECIS.0 as u32,
        CLEARTYPE_QUALITY.0 as u32, 0, face,
    )
}

unsafe fn text_width(hdc: HDC, font: HFONT, s: &str) -> i32 {
    let old = SelectObject(hdc, font);
    let mut r = RECT::default();
    let mut t: Vec<u16> = s.encode_utf16().collect();
    DrawTextW(hdc, &mut t, &mut r, DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX);
    SelectObject(hdc, old);
    r.right - r.left
}

unsafe fn measure(hwnd: HWND, dpi: u32, keys: &[&str], action: &str, key_font: HFONT, action_font: HFONT) -> Layout {
    let hdc = GetDC(hwnd);
    let caps: Vec<i32> = keys.iter().map(|k| (text_width(hdc, key_font, k) + 2 * px(dpi, 12)).max(px(dpi, 34))).collect();
    let plus = text_width(hdc, key_font, "+") + 2 * px(dpi, 8);
    let action_w = text_width(hdc, action_font, action);
    ReleaseDC(hwnd, hdc);
    let w = px(dpi, 16) + caps.iter().sum::<i32>() + plus * (caps.len() as i32 - 1).max(0) + px(dpi, 18) + action_w + px(dpi, 20);
    Layout { w, h: px(dpi, 56), caps, plus }
}

unsafe fn text(hdc: HDC, s: &str, r: RECT, font: HFONT, c: COLORREF, flags: DRAW_TEXT_FORMAT) {
    let old = SelectObject(hdc, font);
    SetTextColor(hdc, c);
    let mut t: Vec<u16> = s.encode_utf16().collect();
    let mut r = r;
    DrawTextW(hdc, &mut t, &mut r, flags | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX);
    SelectObject(hdc, old);
}

unsafe fn draw(hdc: HDC, dpi: u32, l: &Layout, keys: &[&str], action: &str, key_font: HFONT, action_font: HFONT) {
    let bg = CreateSolidBrush(BG);
    FillRect(hdc, &RECT { left: 0, top: 0, right: l.w, bottom: l.h }, bg);
    let _ = DeleteObject(bg);
    SetBkMode(hdc, TRANSPARENT);
    let cap_h = px(dpi, 34);
    let (top, bottom) = ((l.h - cap_h) / 2, (l.h + cap_h) / 2);
    let mut x = px(dpi, 16);
    for (i, (key, &cw)) in keys.iter().zip(&l.caps).enumerate() {
        if i > 0 {
            text(hdc, "+", RECT { left: x, top, right: x + l.plus, bottom }, key_font, PLUS, DT_CENTER);
            x += l.plus;
        }
        // A key cap: rounded, slightly lighter than the strip, with a soft edge.
        let cap = RECT { left: x, top, right: x + cw, bottom };
        crate::smooth::rounded(hdc, &cap, px(dpi, 7) as f32, KEY_FILL, Some(KEY_EDGE));
        text(hdc, key, cap, key_font, KEY_TEXT, DT_CENTER);
        x += cw;
    }
    x += px(dpi, 18);
    text(hdc, action, RECT { left: x, top: 0, right: l.w - px(dpi, 12), bottom: l.h }, action_font, ACTION, DT_LEFT);
}

unsafe fn window() -> HWND {
    if let Some(h) = WINDOW.with(|w| w.get()) {
        return h;
    }
    static REGISTER: Once = Once::new();
    let hinst = GetModuleHandleW(None).map(|m| HINSTANCE(m.0)).unwrap_or_default();
    REGISTER.call_once(|| {
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(keys_proc),
            hInstance: hinst,
            lpszClassName: w!("SnapRelayKeys"),
            ..Default::default()
        });
    });
    let hwnd = CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
        w!("SnapRelayKeys"), w!(""), WS_POPUP,
        0, 0, 10, 10, HWND::default(), HMENU::default(), hinst, None,
    )
    .expect("couldn't create the shortcut display window");
    crate::recording::register(hwnd);
    WINDOW.with(|w| w.set(Some(hwnd)));
    hwnd
}

extern "system" fn keys_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_TIMER if wp.0 == HIDE_TIMER => {
                let _ = KillTimer(hwnd, HIDE_TIMER);
                let _ = KillTimer(hwnd, RAISE_TIMER);
                let _ = ShowWindow(hwnd, SW_HIDE);
                LRESULT(0)
            }
            WM_TIMER if wp.0 == RAISE_TIMER => {
                let _ = SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                LRESULT(0)
            }
            WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
            WM_DPICHANGED => LRESULT(0),
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}
