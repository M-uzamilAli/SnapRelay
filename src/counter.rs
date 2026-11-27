//! Demo counter (recording mode only): a strip in the bottom-left corner showing
//!   0:42        4             8
//!   time    screenshots   switches
//! so a video shows how much work each way takes, without editing.
//!
//! One shortcut starts a fresh count; pressing it again stops and freezes the
//! numbers on screen. Everything is counted by itself:
//! - screenshots: SnapRelay captures, plus any picture another tool (Snipping
//!   Tool) puts on the clipboard. SnapRelay's own pastes don't count.
//! - switches: the window you're looking at changes, or Chrome changes tab
//!   (its window title becomes the new tab's title).
//!
//! A temporary helper for making the demo; it can be removed afterwards.

use std::cell::RefCell;
use std::mem::size_of;
use std::sync::Once;
use std::time::{Duration, Instant};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateFontW, CreateSolidBrush, DeleteObject, DrawTextW, FillRect, GetMonitorInfoW,
    MonitorFromPoint, SelectObject, SetBkMode, SetTextColor, CLEARTYPE_QUALITY,
    CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DT_CENTER, DT_NOPREFIX, DT_SINGLELINE,
    DT_VCENTER, FW_NORMAL, FW_SEMIBOLD, HDC, HFONT, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    OUT_DEFAULT_PRECIS, TRANSPARENT,
};
use windows::Win32::System::DataExchange::{GetClipboardOwner, GetClipboardSequenceNumber, IsClipboardFormatAvailable};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetCursorPos, GetForegroundWindow, GetWindowTextW,
    GetWindowThreadProcessId, KillTimer, RegisterClassW, SetTimer, SetWindowPos, ShowWindow, HMENU,
    HTTRANSPARENT, HWND_TOPMOST, SWP_NOACTIVATE, SWP_SHOWWINDOW, SW_HIDE, WM_DPICHANGED,
    WM_NCHITTEST, WM_TIMER, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use crate::fonts::{REGULAR_FACE, SEMIBOLD_FACE};

const TICK_TIMER: usize = 1;
const TICK_MS: u32 = 200;
/// A title must stay the same this long to count (skips "Loading…" flickers).
const STEADY: Duration = Duration::from_millis(400);
/// One snip can set the clipboard a few times in a row; count it once.
const SNIP_GAP: Duration = Duration::from_millis(700);
const CF_BITMAP: u32 = 2;
const CF_DIB: u32 = 8;

/// Windows that aren't "where you work": the snipping overlay, taskbar, Start,
/// the screen recorder's own controls.
const IGNORED_APPS: [&str; 7] = [
    "ScreenClippingHost.exe", "SnippingTool.exe", "explorer.exe", "ShellExperienceHost.exe",
    "StartMenuExperienceHost.exe", "SearchHost.exe", "Cap.exe",
];

// Brand colours (0x00BBGGRR).
const BG: COLORREF = COLORREF(0x0025_281C); // Slate-Forest
const NUMBER: COLORREF = COLORREF(0x00F4_F6F5); // Linen
const LABEL: COLORREF = COLORREF(0x00B2_B8A9); // #A9B8B2
const LIVE: COLORREF = COLORREF(0x0084_C3E5); // Warm Champagne: shown while counting
const DIVIDER: COLORREF = COLORREF(0x003B_402E); // #2E403B

#[derive(Default)]
struct State {
    hwnd: Option<HWND>,
    running: bool,
    started: Option<Instant>,
    /// Time shown once stopped.
    frozen: Option<Duration>,
    shots: u32,
    switches: u32,
    /// The window/tab title we last counted as "where you are".
    current: Option<String>,
    /// A new title we've seen, and since when (counted once it's steady).
    candidate: Option<(String, Instant)>,
    clip_seq: u32,
    last_snip: Option<Instant>,
    /// SnapRelay's clipboard window: pictures it puts there are pastes, not snips.
    own_clipboard: Option<HWND>,
    png_format: u32,
    dpi: u32,
    /// Where the strip sits (bottom-left of the monitor it started on).
    origin: (i32, i32),
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

pub fn set_clipboard_owner(owner: HWND) {
    let png = unsafe { windows::Win32::System::DataExchange::RegisterClipboardFormatW(w!("PNG")) };
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.own_clipboard = Some(owner);
        s.png_format = png;
    });
}

/// The start/stop shortcut: start a fresh count, or stop and freeze the numbers.
pub fn toggle() {
    let running = STATE.with(|s| s.borrow().running);
    if running {
        stop();
    } else {
        start();
    }
}

/// SnapRelay took a screenshot.
pub fn screenshot_taken() {
    let counted = STATE.with(|s| {
        let mut s = s.borrow_mut();
        if s.running {
            s.shots += 1;
        }
        s.running
    });
    if counted {
        present();
    }
}

/// Recording mode turned off: put the counter away.
pub fn hide() {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.running = false;
        if let Some(h) = s.hwnd {
            unsafe {
                let _ = KillTimer(h, TICK_TIMER);
                let _ = ShowWindow(h, SW_HIDE);
            }
        }
    });
}

fn start() {
    unsafe {
        let hwnd = window();
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut info);
        let (mut dpi, mut _y) = (96u32, 96u32);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dpi, &mut _y);
        let work = info.rcWork;
        let (_, h) = size(dpi);
        let (now_title, seq) = (where_you_are(), GetClipboardSequenceNumber());
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            s.running = true;
            s.started = Some(Instant::now());
            s.frozen = None;
            s.shots = 0;
            s.switches = 0;
            s.current = now_title;
            s.candidate = None;
            s.clip_seq = seq;
            s.last_snip = None;
            s.dpi = dpi;
            s.origin = (work.left + px(dpi, 24), work.bottom - h - px(dpi, 24));
        });
        SetTimer(hwnd, TICK_TIMER, TICK_MS, None);
    }
    present();
}

fn stop() {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.running = false;
        s.frozen = s.started.map(|t| t.elapsed());
        if let Some(h) = s.hwnd {
            unsafe {
                let _ = KillTimer(h, TICK_TIMER);
            }
        }
    });
    present();
}

/// Title of the window you're working in, or None for windows we ignore.
fn where_you_are() -> Option<String> {
    unsafe {
        let h = GetForegroundWindow();
        if h.is_invalid() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, Some(&mut pid));
        if pid == std::process::id() {
            return None;
        }
        let app = crate::release::app_name(pid);
        if IGNORED_APPS.iter().any(|a| a.eq_ignore_ascii_case(&app)) {
            return None;
        }
        let mut buf = [0u16; 512];
        let n = GetWindowTextW(h, &mut buf);
        let title = String::from_utf16_lossy(&buf[..n.max(0) as usize]);
        if title.trim().is_empty() {
            return None;
        }
        Some(format!("{app}|{title}"))
    }
}

/// Every 200 ms while counting: check for a tab/window switch and a new snip.
fn tick() {
    let here = where_you_are();
    let (clip_now, clip_owner) = unsafe { (GetClipboardSequenceNumber(), GetClipboardOwner().ok()) };
    let has_picture = |png: u32| unsafe {
        IsClipboardFormatAvailable(CF_DIB).is_ok()
            || IsClipboardFormatAvailable(CF_BITMAP).is_ok()
            || (png != 0 && IsClipboardFormatAvailable(png).is_ok())
    };
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        if !s.running {
            return;
        }
        // Switches: a different title that has stayed put for a moment.
        if let Some(t) = here {
            if s.current.as_deref() == Some(t.as_str()) {
                s.candidate = None;
            } else {
                match &s.candidate {
                    Some((c, since)) if *c == t => {
                        if since.elapsed() >= STEADY {
                            if s.current.is_some() {
                                s.switches += 1;
                            }
                            s.current = Some(t);
                            s.candidate = None;
                        }
                    }
                    _ => s.candidate = Some((t, Instant::now())),
                }
            }
        }
        // Snips: a new picture on the clipboard that SnapRelay didn't put there.
        if clip_now != s.clip_seq {
            s.clip_seq = clip_now;
            let ours = clip_owner.is_some() && clip_owner == s.own_clipboard;
            let fresh = s.last_snip.map_or(true, |t| t.elapsed() >= SNIP_GAP);
            if !ours && fresh && has_picture(s.png_format) {
                s.shots += 1;
                s.last_snip = Some(Instant::now());
            }
        }
    });
    present();
}

fn px(dpi: u32, v: i32) -> i32 {
    v * dpi as i32 / 96
}

/// Fixed size, so the strip doesn't jiggle as the numbers change.
fn size(dpi: u32) -> (i32, i32) {
    (px(dpi, 3 * 112 + 32), px(dpi, 68))
}

fn clock(d: Duration) -> String {
    let secs = d.as_secs();
    format!("{}:{:02}", secs / 60, secs % 60)
}

unsafe fn font(dpi: u32, size: i32, weight: u32, face: PCWSTR) -> HFONT {
    CreateFontW(
        -px(dpi, size), 0, 0, 0, weight as i32, 0, 0, 0,
        DEFAULT_CHARSET.0 as u32, OUT_DEFAULT_PRECIS.0 as u32, CLIP_DEFAULT_PRECIS.0 as u32,
        CLEARTYPE_QUALITY.0 as u32, 0, face,
    )
}

unsafe fn text(hdc: HDC, s: &str, r: RECT, font: HFONT, c: COLORREF) {
    let old = SelectObject(hdc, font);
    SetTextColor(hdc, c);
    let mut t: Vec<u16> = s.encode_utf16().collect();
    let mut r = r;
    DrawTextW(hdc, &mut t, &mut r, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
    SelectObject(hdc, old);
}

fn present() {
    let snapshot = STATE.with(|s| {
        let s = s.borrow();
        let hwnd = s.hwnd?;
        let elapsed = s.frozen.or_else(|| s.started.map(|t| t.elapsed())).unwrap_or_default();
        Some((hwnd, s.dpi, s.origin, s.running, clock(elapsed), s.shots, s.switches))
    });
    let Some((hwnd, dpi, (x, y), running, time, shots, switches)) = snapshot else { return };
    let (w, h) = size(dpi);
    unsafe {
        let _ = SetWindowPos(hwnd, HWND_TOPMOST, x, y, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
        let (big, small) = (font(dpi, 24, FW_SEMIBOLD.0, SEMIBOLD_FACE), font(dpi, 12, FW_NORMAL.0, REGULAR_FACE));
        crate::smooth::present_rounded(hwnd, w, h, px(dpi, 14) as f32, None, |hdc| {
            let bg = CreateSolidBrush(BG);
            FillRect(hdc, &RECT { left: 0, top: 0, right: w, bottom: h }, bg);
            let _ = DeleteObject(bg);
            SetBkMode(hdc, TRANSPARENT);
            let col = px(dpi, 112);
            let left = px(dpi, 16);
            let stats = [
                (time.as_str(), "time", if running { LIVE } else { NUMBER }),
                (&*shots.to_string(), if shots == 1 { "screenshot" } else { "screenshots" }, NUMBER),
                (&*switches.to_string(), if switches == 1 { "switch" } else { "switches" }, NUMBER),
            ];
            for (i, (value, label, colour)) in stats.iter().enumerate() {
                let x0 = left + i as i32 * col;
                if i > 0 {
                    let d = CreateSolidBrush(DIVIDER);
                    FillRect(hdc, &RECT { left: x0, top: px(dpi, 14), right: x0 + px(dpi, 1).max(1), bottom: h - px(dpi, 14) }, d);
                    let _ = DeleteObject(d);
                }
                text(hdc, value, RECT { left: x0, top: px(dpi, 8), right: x0 + col, bottom: px(dpi, 42) }, big, *colour);
                text(hdc, label, RECT { left: x0, top: px(dpi, 40), right: x0 + col, bottom: h - px(dpi, 8) }, small, LABEL);
            }
        });
        let _ = DeleteObject(big);
        let _ = DeleteObject(small);
    }
}

unsafe fn window() -> HWND {
    if let Some(h) = STATE.with(|s| s.borrow().hwnd) {
        return h;
    }
    static REGISTER: Once = Once::new();
    let hinst = GetModuleHandleW(None).map(|m| HINSTANCE(m.0)).unwrap_or_default();
    REGISTER.call_once(|| {
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(counter_proc),
            hInstance: hinst,
            lpszClassName: w!("SnapRelayCounter"),
            ..Default::default()
        });
    });
    let hwnd = CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
        w!("SnapRelayCounter"), w!(""), WS_POPUP,
        0, 0, 10, 10, HWND::default(), HMENU::default(), hinst, None,
    )
    .expect("couldn't create the counter window");
    crate::recording::register(hwnd);
    STATE.with(|s| s.borrow_mut().hwnd = Some(hwnd));
    hwnd
}

extern "system" fn counter_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_TIMER if wp.0 == TICK_TIMER => {
                tick();
                LRESULT(0)
            }
            WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
            WM_DPICHANGED => LRESULT(0),
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}
