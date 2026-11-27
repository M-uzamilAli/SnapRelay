//! Settings window (tray menu > Settings…): the four shortcuts, start with Windows,
//! capture sound and flash, the queue's memory limit and auto-hide. Every change
//! applies and saves straight away; there's no Save button.
//!
//! Changing a shortcut: click its row, press the new keys. While listening, all of
//! SnapRelay's shortcuts are paused so any combination reaches this window.

use std::cell::RefCell;
use std::mem::size_of;
use std::sync::Once;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateFontW, CreateSolidBrush, DeleteObject, DrawTextW, FillRect, GetMonitorInfoW, MonitorFromPoint,
    SelectObject, SetBkMode, SetTextColor, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET,
    DRAW_TEXT_FORMAT, DT_CALCRECT, DT_CENTER, DT_LEFT, DT_NOPREFIX, DT_RIGHT, DT_SINGLELINE, DT_VCENTER,
    FW_NORMAL, FW_SEMIBOLD, HDC, HFONT, MONITORINFO, MONITOR_DEFAULTTONEAREST, OUT_DEFAULT_PRECIS, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, SetFocus, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_KEYUP, VIRTUAL_KEY, VK_CONTROL, VK_ESCAPE, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, PostMessageW, SetWindowsHookExW, UnhookWindowsHookEx, HHOOK, KBDLLHOOKSTRUCT,
    WH_KEYBOARD_LL, WM_APP,
    CreateWindowExW, DefWindowProcW, GetCursorPos, GetWindowRect, LoadCursorW, RegisterClassW, SetCursor,
    SetForegroundWindow, SetWindowPos, ShowWindow, HMENU, HTCAPTION, HTCLIENT, HWND_TOP, IDC_ARROW, IDC_HAND,
    SWP_SHOWWINDOW, SW_HIDE, WA_INACTIVE, WM_ACTIVATE, WM_CLOSE, WM_DPICHANGED, WM_KEYDOWN, WM_LBUTTONUP,
    WM_MOUSEMOVE, WM_NCHITTEST, WM_SETCURSOR, WM_SYSKEYDOWN, WNDCLASSW, WS_EX_LAYERED, WS_EX_TOOLWINDOW,
    WS_POPUP,
};

use crate::fonts::{REGULAR_FACE, SEMIBOLD_FACE};
use crate::hotkeys;
use crate::settings::{self, Combo, Shortcut, ALT, CTRL, IDLE_CHOICES, LIMITS_MB, SHIFT, WIN};

// ---------- colours (0x00BBGGRR), following the Windows light/dark setting ----------
struct Colors {
    bg: COLORREF,
    hover: COLORREF,
    border: COLORREF,
    outline: COLORREF,
    text: COLORREF,
    muted: COLORREF,
    danger: COLORREF,
    success: COLORREF,
    primary: COLORREF,
    cap: COLORREF,
    cap_edge: COLORREF,
    track: COLORREF,
}
const LIGHT: Colors = Colors {
    bg: COLORREF(0x00F4_F6F5), hover: COLORREF(0x00E8_ECEA), border: COLORREF(0x00DE_E3E0),
    outline: COLORREF(0x0094_A07A), text: COLORREF(0x0025_281C), muted: COLORREF(0x0066_6B5A),
    danger: COLORREF(0x003D_48B5), success: COLORREF(0x0057_7A2F), primary: COLORREF(0x0084_C3E5),
    cap: COLORREF(0x00FF_FFFF), cap_edge: COLORREF(0x00CD_D1C9), track: COLORREF(0x00CD_D1C9),
};
const DARK: Colors = Colors {
    bg: COLORREF(0x0025_281C), hover: COLORREF(0x002F_3324), border: COLORREF(0x003B_402E),
    outline: COLORREF(0x006C_784B), text: COLORREF(0x00F4_F6F5), muted: COLORREF(0x00B2_B8A9),
    danger: COLORREF(0x006E_7AE0), success: COLORREF(0x00A0_C87F), primary: COLORREF(0x0084_C3E5),
    cap: COLORREF(0x003B_402E), cap_edge: COLORREF(0x006C_784B), track: COLORREF(0x0045_4A3B),
};

#[derive(Clone, Copy, PartialEq)]
enum Hit {
    Nothing,
    Close,
    Key(Shortcut),
    StartWithWindows,
    Sound,
    Flash,
    Limit(usize),
    Idle(usize),
    Reset,
}

impl Hit {
    fn clickable(self) -> bool {
        !matches!(self, Hit::Nothing)
    }
}

#[derive(Default)]
struct State {
    hwnd: Option<HWND>,
    dpi: u32,
    light: bool,
    hover: Option<Hit>,
    listening: Option<Shortcut>,
    /// A one-line result under the shortcuts: (text, is an error).
    message: Option<(String, bool)>,
    start_with_windows: bool,
    /// Title, body, small.
    fonts: Option<[HFONT; 3]>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

// ---------- layout (96-dpi units, scaled) ----------
const WIDTH: i32 = 460;
const PAD: i32 = 22;
const HEADER: i32 = 56;
const SECTION: i32 = 34;
const ROW: i32 = 46;
const MESSAGE: i32 = 26;
const FOOTER: i32 = 64;

struct Layout {
    dpi: i32,
    w: i32,
    h: i32,
    close: RECT,
    sections: [i32; 3],
    keys: [RECT; 4],
    message: RECT,
    toggles: [RECT; 3],
    limit_row: RECT,
    idle_row: RECT,
    footer: i32,
    reset: RECT,
}

impl Layout {
    fn px(&self, v: i32) -> i32 {
        v * self.dpi / 96
    }
    fn toggle_switch(&self, row: &RECT) -> RECT {
        let (w, h) = (self.px(40), self.px(22));
        let top = row.top + (row.bottom - row.top - h) / 2;
        RECT { left: row.right - self.px(PAD) - w, top, right: row.right - self.px(PAD), bottom: top + h }
    }
    /// Option chips at the right of a row, e.g. [100 MB][200 MB][500 MB].
    fn chips(&self, row: &RECT, n: usize) -> Vec<RECT> {
        let (w, h, gap) = (self.px(64), self.px(28), self.px(6));
        let top = row.top + (row.bottom - row.top - h) / 2;
        let right = row.right - self.px(PAD);
        (0..n)
            .map(|i| {
                let l = right - (n - i) as i32 * w - (n - 1 - i) as i32 * gap;
                RECT { left: l, top, right: l + w, bottom: top + h }
            })
            .collect()
    }
}

fn layout(dpi: u32) -> Layout {
    let d = dpi as i32;
    let p = |v: i32| v * d / 96;
    let w = p(WIDTH);
    let row = |top: i32| RECT { left: 0, top, right: w, bottom: top + p(ROW) };
    let mut y = p(HEADER);
    let s0 = y;
    y += p(SECTION);
    let keys = [0, 1, 2, 3].map(|i| row(y + i * p(ROW)));
    y += 4 * p(ROW);
    let message = RECT { left: p(PAD), top: y, right: w - p(PAD), bottom: y + p(MESSAGE) };
    y += p(MESSAGE);
    let s1 = y;
    y += p(SECTION);
    let toggles = [0, 1, 2].map(|i| row(y + i * p(ROW)));
    y += 3 * p(ROW);
    let s2 = y;
    y += p(SECTION);
    let limit_row = row(y);
    let idle_row = row(y + p(ROW));
    y += 2 * p(ROW);
    let footer = y;
    let b = p(36);
    let reset = RECT { left: p(PAD), top: footer + (p(FOOTER) - b) / 2, right: p(PAD) + p(150), bottom: footer + (p(FOOTER) + b) / 2 };
    let cb = p(28);
    Layout {
        dpi: d, w, h: footer + p(FOOTER),
        close: RECT { left: w - p(PAD) - cb, top: (p(HEADER) - cb) / 2, right: w - p(PAD), bottom: (p(HEADER) + cb) / 2 },
        sections: [s0, s1, s2], keys, message, toggles, limit_row, idle_row, footer, reset,
    }
}

fn inside(r: &RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

fn hit(l: &Layout, x: i32, y: i32) -> Hit {
    if inside(&l.close, x, y) {
        return Hit::Close;
    }
    for (i, r) in l.keys.iter().enumerate() {
        if inside(r, x, y) {
            return Hit::Key(Shortcut::ALL[i]);
        }
    }
    for (i, r) in l.toggles.iter().enumerate() {
        if inside(r, x, y) {
            return [Hit::StartWithWindows, Hit::Sound, Hit::Flash][i];
        }
    }
    for (i, c) in l.chips(&l.limit_row, LIMITS_MB.len()).iter().enumerate() {
        if inside(c, x, y) {
            return Hit::Limit(i);
        }
    }
    for (i, c) in l.chips(&l.idle_row, IDLE_CHOICES.len()).iter().enumerate() {
        if inside(c, x, y) {
            return Hit::Idle(i);
        }
    }
    if inside(&l.reset, x, y) {
        return Hit::Reset;
    }
    Hit::Nothing
}

// ---------- opening and closing ----------

/// Show the Settings window (tray menu). Brings it to the front if it's already open.
pub fn open() {
    unsafe {
        let hwnd = window();
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let (mut dpi, mut _y) = (96u32, 96u32);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dpi, &mut _y);
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut info);
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            if s.fonts.is_none() {
                s.fonts = Some([
                    font(dpi, 18, FW_SEMIBOLD.0, SEMIBOLD_FACE),
                    font(dpi, 14, FW_NORMAL.0, REGULAR_FACE),
                    font(dpi, 12, FW_SEMIBOLD.0, SEMIBOLD_FACE),
                ]);
            }
            s.dpi = dpi;
            s.light = crate::theme::is_light();
            s.start_with_windows = settings::starts_with_windows();
            s.message = None;
            s.hover = None;
        });
        let l = layout(dpi);
        let r = info.rcWork;
        let (x, y) = (r.left + (r.right - r.left - l.w) / 2, r.top + (r.bottom - r.top - l.h) / 2);
        let _ = SetWindowPos(hwnd, HWND_TOP, x, y, l.w, l.h, SWP_SHOWWINDOW);
        present(hwnd);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(hwnd);
    }
}

fn close() {
    stop_listening(None);
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        if let Some(h) = s.hwnd {
            unsafe {
                let _ = ShowWindow(h, SW_HIDE);
            }
        }
        for f in s.fonts.take().into_iter().flatten() {
            unsafe {
                let _ = DeleteObject(f);
            }
        }
    });
}

thread_local! {
    /// While listening: a keyboard hook that hears keys before any other app, so
    /// even a combination another app has reserved (and would swallow) reaches us.
    static HOOK: std::cell::Cell<Option<HHOOK>> = const { std::cell::Cell::new(None) };
}

/// Posted by the hook to the window: a key went down (wParam = key code).
const WM_HEARD_KEY: u32 = WM_APP + 21;

fn start_listening(sc: Shortcut) {
    hotkeys::suspend();
    unsafe {
        let hinst = GetModuleHandleW(None).map(|m| HINSTANCE(m.0)).unwrap_or_default();
        if let Ok(h) = SetWindowsHookExW(WH_KEYBOARD_LL, Some(heard_key), hinst, 0) {
            HOOK.with(|x| x.set(Some(h)));
        }
    }
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.listening = Some(sc);
        s.message = Some(("Press the new keys \u{00B7} Esc to cancel".into(), false));
    });
}

/// Stop listening for keys and put the shortcuts back; `message` is the outcome.
fn stop_listening(message: Option<(String, bool)>) {
    if let Some(h) = HOOK.with(|x| x.take()) {
        unsafe {
            let _ = UnhookWindowsHookEx(h);
        }
    }
    let was = STATE.with(|s| s.borrow_mut().listening.take());
    if was.is_some() {
        hotkeys::resume();
        STATE.with(|s| s.borrow_mut().message = message);
    }
}

/// The keyboard hook (only while listening). Modifiers pass through; the first
/// other key is kept from every app (so another app's shortcut doesn't fire) and
/// handed to the window.
extern "system" fn heard_key(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if code >= 0 && (wp.0 as u32 == WM_KEYDOWN || wp.0 as u32 == WM_SYSKEYDOWN) {
            let k = &*(lp.0 as *const KBDLLHOOKSTRUCT);
            // 0xE8 is our own harmless key (below); let it through.
            if k.vkCode != 0xE8 && !is_modifier(k.vkCode) {
                if let Some(h) = STATE.with(|s| s.try_borrow().ok().and_then(|s| s.hwnd)) {
                    let _ = PostMessageW(h, WM_HEARD_KEY, WPARAM(k.vkCode as usize), LPARAM(0));
                    // With Win held, a swallowed key would leave a "lone Win" press that
                    // opens Start; a harmless injected key in between prevents that.
                    if down(VK_LWIN) || down(VK_RWIN) {
                        tap_unassigned();
                    }
                    return LRESULT(1);
                }
            }
        }
        CallNextHookEx(None, code, wp, lp)
    }
}

fn is_modifier(vk: u32) -> bool {
    matches!(vk, 0x10 | 0x11 | 0x12 | 0x5B | 0x5C | 0xA0..=0xA5)
}

fn tap_unassigned() {
    let tap = |up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: VIRTUAL_KEY(0xE8), wScan: 0, dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) }, time: 0, dwExtraInfo: 0 },
        },
    };
    unsafe {
        SendInput(&[tap(false), tap(true)], size_of::<INPUT>() as i32);
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
            lpfnWndProc: Some(settings_proc),
            hInstance: hinst,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: w!("SnapRelaySettings"),
            ..Default::default()
        });
    });
    // Layered: we draw the whole picture, including its smooth rounded shape.
    // Unlike the queue, this window takes focus: it needs the keyboard.
    let hwnd = CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TOOLWINDOW, w!("SnapRelaySettings"), w!("SnapRelay settings"), WS_POPUP,
        0, 0, 10, 10, HWND::default(), HMENU::default(), hinst, None,
    )
    .expect("couldn't create the settings window");
    STATE.with(|s| s.borrow_mut().hwnd = Some(hwnd));
    hwnd
}

// ---------- input ----------

/// Is the key held right now? (Physical state: the hook runs before the window's own.)
fn down(vk: VIRTUAL_KEY) -> bool {
    unsafe { GetAsyncKeyState(vk.0 as i32) < 0 }
}

/// A key went down while listening: modifiers alone wait; Esc cancels; anything
/// else together with the held modifiers becomes the new shortcut.
fn key_pressed(sc: Shortcut, vk: u32) {
    if is_modifier(vk) {
        return;
    }
    if vk == VK_ESCAPE.0 as u32 {
        stop_listening(Some(("Unchanged".into(), false)));
        return;
    }
    let mut mods = 0;
    if down(VK_LWIN) || down(VK_RWIN) {
        mods |= WIN;
    }
    if down(VK_CONTROL) {
        mods |= CTRL;
    }
    if down(VK_MENU) {
        mods |= ALT;
    }
    if down(VK_SHIFT) {
        mods |= SHIFT;
    }
    let combo = Combo { mods, vk };
    let outcome = match hotkeys::change(sc, combo) {
        Ok(()) => (format!("{} \u{2192} {}", sc.title(), settings::label(combo)), false),
        Err(why) => (why, true),
    };
    stop_listening(Some(outcome));
}

fn click(h: Hit) {
    match h {
        Hit::Close => close(),
        Hit::Key(sc) => {
            // Clicking the row that's listening cancels; another row switches to it.
            let current = STATE.with(|s| s.borrow().listening);
            stop_listening(None);
            if current != Some(sc) {
                start_listening(sc);
            }
        }
        Hit::StartWithWindows => {
            let on = !STATE.with(|s| s.borrow().start_with_windows);
            let result = settings::set_start_with_windows(on);
            STATE.with(|s| {
                let mut s = s.borrow_mut();
                s.start_with_windows = settings::starts_with_windows();
                if let Err(e) = result {
                    s.message = Some((format!("Couldn't change start-up: {e}"), true));
                }
            });
        }
        Hit::Sound => settings::update(|s| s.sound = !s.sound),
        Hit::Flash => settings::update(|s| s.flash = !s.flash),
        Hit::Limit(i) => settings::update(|s| s.limit_mb = LIMITS_MB[i]),
        Hit::Idle(i) => settings::update(|s| s.idle_secs = IDLE_CHOICES[i]),
        Hit::Reset => {
            stop_listening(None);
            hotkeys::suspend();
            hotkeys::reset();
            hotkeys::resume();
            let d = settings::Settings::default();
            settings::update(|s| {
                s.sound = d.sound;
                s.flash = d.flash;
                s.limit_mb = d.limit_mb;
                s.idle_secs = d.idle_secs;
            });
            STATE.with(|s| s.borrow_mut().message = Some(("Back to the defaults".into(), false)));
        }
        Hit::Nothing => {}
    }
}

fn point(lp: LPARAM) -> (i32, i32) {
    ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32)
}

extern "system" fn settings_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        let dpi = STATE.with(|s| s.try_borrow().map(|s| s.dpi).unwrap_or(96));
        match msg {
            // Drag the window by its header.
            WM_NCHITTEST => {
                let (sx, sy) = point(lp);
                let mut r = RECT::default();
                let _ = GetWindowRect(hwnd, &mut r);
                let l = layout(dpi);
                let (x, y) = (sx - r.left, sy - r.top);
                if y < l.px(HEADER) && !inside(&l.close, x, y) {
                    LRESULT(HTCAPTION as isize)
                } else {
                    LRESULT(HTCLIENT as isize)
                }
            }
            WM_MOUSEMOVE => {
                let (x, y) = point(lp);
                let h = hit(&layout(dpi), x, y);
                let changed = STATE.with(|s| s.borrow_mut().hover.replace(h) != Some(h));
                if changed {
                    present(hwnd);
                }
                LRESULT(0)
            }
            WM_SETCURSOR => {
                let on = STATE.with(|s| s.try_borrow().ok().and_then(|s| s.hover).map(Hit::clickable).unwrap_or(false));
                if on && (lp.0 & 0xFFFF) as u32 == HTCLIENT {
                    SetCursor(LoadCursorW(None, IDC_HAND).unwrap_or_default());
                    LRESULT(1)
                } else {
                    DefWindowProcW(hwnd, msg, wp, lp)
                }
            }
            WM_LBUTTONUP => {
                let (x, y) = point(lp);
                click(hit(&layout(dpi), x, y));
                present(hwnd);
                LRESULT(0)
            }
            WM_HEARD_KEY => {
                if let Some(sc) = STATE.with(|s| s.borrow().listening) {
                    key_pressed(sc, wp.0 as u32);
                    present(hwnd);
                }
                LRESULT(0)
            }
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                let listening = STATE.with(|s| s.borrow().listening);
                match listening {
                    Some(sc) => {
                        key_pressed(sc, wp.0 as u32);
                        present(hwnd);
                        LRESULT(0)
                    }
                    None if msg == WM_KEYDOWN && wp.0 as u16 == VK_ESCAPE.0 => {
                        close();
                        LRESULT(0)
                    }
                    None => DefWindowProcW(hwnd, msg, wp, lp),
                }
            }
            // Clicked away while listening: give up listening, keep the old shortcut.
            WM_ACTIVATE if (wp.0 & 0xFFFF) as u32 == WA_INACTIVE => {
                stop_listening(None);
                present(hwnd);
                LRESULT(0)
            }
            WM_CLOSE => {
                close();
                LRESULT(0)
            }
            WM_DPICHANGED => LRESULT(0),
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

// ---------- drawing ----------

unsafe fn font(dpi: u32, size: i32, weight: u32, face: PCWSTR) -> HFONT {
    CreateFontW(
        -(size * dpi as i32 / 96), 0, 0, 0, weight as i32, 0, 0, 0,
        DEFAULT_CHARSET.0 as u32, OUT_DEFAULT_PRECIS.0 as u32, CLIP_DEFAULT_PRECIS.0 as u32,
        CLEARTYPE_QUALITY.0 as u32, 0, face,
    )
}

unsafe fn fill(hdc: HDC, r: &RECT, c: COLORREF) {
    let b = CreateSolidBrush(c);
    FillRect(hdc, r, b);
    let _ = DeleteObject(b);
}

unsafe fn text(hdc: HDC, s: &str, r: RECT, font: HFONT, c: COLORREF, flags: DRAW_TEXT_FORMAT) {
    let old = SelectObject(hdc, font);
    SetTextColor(hdc, c);
    let mut t: Vec<u16> = s.encode_utf16().collect();
    let mut r = r;
    DrawTextW(hdc, &mut t, &mut r, flags | DT_NOPREFIX | DT_SINGLELINE | DT_VCENTER);
    SelectObject(hdc, old);
}

unsafe fn width(hdc: HDC, s: &str, font: HFONT) -> i32 {
    let old = SelectObject(hdc, font);
    let mut r = RECT::default();
    let mut t: Vec<u16> = s.encode_utf16().collect();
    DrawTextW(hdc, &mut t, &mut r, DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX);
    SelectObject(hdc, old);
    r.right - r.left
}

/// Key caps right-aligned in `row`: [Win] + [Shift] + [D].
unsafe fn caps(hdc: HDC, l: &Layout, row: &RECT, label: &str, small: HFONT, c: &Colors, edge: COLORREF) {
    let h = l.px(26);
    let top = row.top + (row.bottom - row.top - h) / 2;
    let mut x = row.right - l.px(PAD);
    let parts: Vec<&str> = label.split('+').collect();
    for (i, part) in parts.iter().enumerate().rev() {
        let w = width(hdc, part, small) + l.px(16);
        let r = RECT { left: x - w, top, right: x, bottom: top + h };
        crate::smooth::rounded(hdc, &r, l.px(6) as f32, c.cap, Some(edge));
        text(hdc, part, r, small, c.text, DT_CENTER);
        x -= w;
        if i > 0 {
            let plus = RECT { left: x - l.px(14), top, right: x, bottom: top + h };
            text(hdc, "+", plus, small, c.muted, DT_CENTER);
            x -= l.px(14);
        }
    }
}

unsafe fn switch(hdc: HDC, l: &Layout, r: &RECT, on: bool, c: &Colors) {
    let h = r.bottom - r.top;
    crate::smooth::rounded(hdc, r, h as f32 / 2.0, if on { c.text } else { c.track }, None);
    let k = h - l.px(4);
    let left = if on { r.right - l.px(2) - k } else { r.left + l.px(2) };
    let knob = RECT { left, top: r.top + l.px(2), right: left + k, bottom: r.top + l.px(2) + k };
    crate::smooth::rounded(hdc, &knob, k as f32 / 2.0, if on { c.bg } else { COLORREF(0x00FF_FFFF) }, None);
}

unsafe fn chips(hdc: HDC, l: &Layout, rects: &[RECT], labels: &[String], selected: usize, hover: Option<usize>, small: HFONT, c: &Colors) {
    for (i, r) in rects.iter().enumerate() {
        if i == selected {
            crate::smooth::rounded(hdc, r, l.px(7) as f32, c.text, None);
            text(hdc, &labels[i], *r, small, c.bg, DT_CENTER);
        } else {
            let fill_c = if hover == Some(i) { c.hover } else { c.bg };
            crate::smooth::rounded(hdc, r, l.px(7) as f32, fill_c, Some(c.border));
            text(hdc, &labels[i], *r, small, c.text, DT_CENTER);
        }
    }
}

fn present(hwnd: HWND) {
    STATE.with(|s| {
        let Ok(s) = s.try_borrow() else { return };
        let Some(fonts) = s.fonts else { return };
        let l = layout(s.dpi);
        let c = if s.light { &LIGHT } else { &DARK };
        let st = settings::get();
        unsafe {
            crate::smooth::present_rounded(hwnd, l.w, l.h, l.px(10) as f32, Some(c.outline), |hdc| {
                render(hdc, &s, &l, c, fonts, &st)
            });
        }
    });
}

unsafe fn render(hdc: HDC, s: &State, l: &Layout, c: &Colors, [f_title, f_body, f_small]: [HFONT; 3], st: &settings::Settings) {
    let pad = l.px(PAD);
    fill(hdc, &RECT { left: 0, top: 0, right: l.w, bottom: l.h }, c.bg);
    SetBkMode(hdc, TRANSPARENT);
    let hover = s.hover.unwrap_or(Hit::Nothing);

    // Header
    text(hdc, "Settings", RECT { left: pad, top: 0, right: l.w - pad, bottom: l.px(HEADER) }, f_title, c.text, DT_LEFT);
    if hover == Hit::Close {
        crate::smooth::rounded(hdc, &l.close, l.px(8) as f32, c.hover, None);
    }
    let (cx, cy, half) = ((l.close.left + l.close.right) as f32 / 2.0, (l.close.top + l.close.bottom) as f32 / 2.0, l.px(5) as f32);
    let xc = if hover == Hit::Close { c.text } else { c.muted };
    crate::smooth::line(hdc, (cx - half, cy - half), (cx + half, cy + half), xc, l.px(2) as f32 * 0.8);
    crate::smooth::line(hdc, (cx + half, cy - half), (cx - half, cy + half), xc, l.px(2) as f32 * 0.8);
    fill(hdc, &RECT { left: 0, top: l.px(HEADER) - 1, right: l.w, bottom: l.px(HEADER) }, c.border);

    // Section titles
    for (y, name) in l.sections.iter().zip(["SHORTCUTS", "GENERAL", "QUEUE"]) {
        text(hdc, name, RECT { left: pad, top: *y + l.px(10), right: l.w, bottom: *y + l.px(SECTION) }, f_small, c.muted, DT_LEFT);
    }

    // Shortcuts
    for (i, sc) in Shortcut::ALL.into_iter().enumerate() {
        let r = l.keys[i];
        let listening = s.listening == Some(sc);
        if listening || hover == Hit::Key(sc) {
            fill(hdc, &r, c.hover);
        }
        text(hdc, sc.title(), RECT { left: pad, ..r }, f_body, c.text, DT_LEFT);
        if listening {
            let w = width(hdc, "Press keys\u{2026}", f_small) + l.px(20);
            let h = l.px(26);
            let top = r.top + (l.px(ROW) - h) / 2;
            let cap = RECT { left: r.right - pad - w, top, right: r.right - pad, bottom: top + h };
            crate::smooth::rounded(hdc, &cap, l.px(6) as f32, c.cap, Some(c.primary));
            text(hdc, "Press keys\u{2026}", cap, f_small, c.text, DT_CENTER);
        } else if hotkeys::is_active(sc) {
            caps(hdc, l, &r, &hotkeys::label(sc), f_small, c, c.cap_edge);
        } else {
            text(hdc, "Not active \u{00B7} click to set", RECT { right: r.right - pad, ..r }, f_small, c.danger, DT_RIGHT);
        }
    }
    if let Some((m, is_error)) = &s.message {
        text(hdc, m, l.message, f_small, if *is_error { c.danger } else { c.success }, DT_RIGHT);
    }

    // General
    let toggles = [("Start with Windows", s.start_with_windows, Hit::StartWithWindows), ("Capture sound", st.sound, Hit::Sound), ("Capture flash", st.flash, Hit::Flash)];
    for (r, (name, on, h)) in l.toggles.iter().zip(toggles) {
        if hover == h {
            fill(hdc, r, c.hover);
        }
        text(hdc, name, RECT { left: pad, ..*r }, f_body, c.text, DT_LEFT);
        switch(hdc, l, &l.toggle_switch(r), on, c);
    }

    // Queue
    text(hdc, "Memory limit", RECT { left: pad, ..l.limit_row }, f_body, c.text, DT_LEFT);
    let limit_labels: Vec<String> = LIMITS_MB.iter().map(|m| format!("{m} MB")).collect();
    let sel = LIMITS_MB.iter().position(|m| *m == st.limit_mb).unwrap_or(1);
    let hov = if let Hit::Limit(i) = hover { Some(i) } else { None };
    chips(hdc, l, &l.chips(&l.limit_row, LIMITS_MB.len()), &limit_labels, sel, hov, f_small, c);

    text(hdc, "Hide queue when idle", RECT { left: pad, ..l.idle_row }, f_body, c.text, DT_LEFT);
    let idle_labels: Vec<String> = IDLE_CHOICES.iter().map(|s| if *s == 0 { "Never".into() } else { format!("{s} s") }).collect();
    let sel = IDLE_CHOICES.iter().position(|x| *x == st.idle_secs).unwrap_or(1);
    let hov = if let Hit::Idle(i) = hover { Some(i) } else { None };
    chips(hdc, l, &l.chips(&l.idle_row, IDLE_CHOICES.len()), &idle_labels, sel, hov, f_small, c);

    // Footer
    fill(hdc, &RECT { left: 0, top: l.footer, right: l.w, bottom: l.footer + 1 }, c.border);
    crate::smooth::rounded(hdc, &l.reset, l.px(8) as f32, if hover == Hit::Reset { c.hover } else { c.bg }, Some(c.outline));
    text(hdc, "Reset to defaults", l.reset, f_small, c.text, DT_CENTER);
    text(hdc, "Changes apply right away", RECT { left: l.reset.right, top: l.footer, right: l.w - pad, bottom: l.footer + l.px(FOOTER) }, f_small, c.muted, DT_RIGHT);
}
