//! Settings panel, shown inside the queue pop-up (⚙ in its header, or tray menu >
//! Settings…): the four shortcuts, start with Windows, capture sound and flash, the
//! queue's memory limit and auto-hide. Every change applies and saves straight
//! away; there's no Save button. The queue window owns the pop-up; this part only
//! lays out, draws and handles clicks for the panel area below its header.
//!
//! Changing a shortcut: click its row, press the new keys. While listening, a short
//! keyboard hook hears keys before any other app (the pop-up never takes focus,
//! and another app's reserved combination would otherwise be swallowed), and all of
//! SnapRelay's own shortcuts are paused.

use std::cell::{Cell, RefCell};
use std::mem::size_of;

use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateSolidBrush, DeleteObject, DrawTextW, FillRect, SelectObject, SetTextColor, DRAW_TEXT_FORMAT,
    DT_CALCRECT, DT_CENTER, DT_LEFT, DT_NOPREFIX, DT_RIGHT, DT_SINGLELINE, DT_VCENTER, HDC, HFONT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
    VIRTUAL_KEY, VK_CONTROL, VK_ESCAPE, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, PostMessageW, SetWindowsHookExW, UnhookWindowsHookEx, HHOOK, KBDLLHOOKSTRUCT, WH_KEYBOARD_LL,
    WM_APP, WM_KEYDOWN, WM_SYSKEYDOWN,
};

use crate::hotkeys;
use crate::settings::{self, Combo, Shortcut, ALT, CTRL, IDLE_CHOICES, LIMITS_MB, SHIFT, WIN};

/// Posted by the keyboard hook to the pop-up: a key went down (wParam = key code).
pub const WM_HEARD_KEY: u32 = WM_APP + 21;

// ---------- colours (0x00BBGGRR), the queue pop-up's palette plus a few extras ----------
struct Colors {
    bg: COLORREF,
    hover: COLORREF,
    border: COLORREF,
    text: COLORREF,
    muted: COLORREF,
    danger: COLORREF,
    success: COLORREF,
    primary: COLORREF,
    cap: COLORREF,
    cap_edge: COLORREF,
    track: COLORREF,
    outline: COLORREF,
}
const LIGHT: Colors = Colors {
    bg: COLORREF(0x00F4_F6F5), hover: COLORREF(0x00E8_ECEA), border: COLORREF(0x00DE_E3E0),
    text: COLORREF(0x0025_281C), muted: COLORREF(0x0066_6B5A), danger: COLORREF(0x003D_48B5),
    success: COLORREF(0x0057_7A2F), primary: COLORREF(0x0084_C3E5), cap: COLORREF(0x00FF_FFFF),
    cap_edge: COLORREF(0x00CD_D1C9), track: COLORREF(0x00CD_D1C9), outline: COLORREF(0x0094_A07A),
};
const DARK: Colors = Colors {
    bg: COLORREF(0x0025_281C), hover: COLORREF(0x002F_3324), border: COLORREF(0x003B_402E),
    text: COLORREF(0x00F4_F6F5), muted: COLORREF(0x00B2_B8A9), danger: COLORREF(0x006E_7AE0),
    success: COLORREF(0x00A0_C87F), primary: COLORREF(0x0084_C3E5), cap: COLORREF(0x003B_402E),
    cap_edge: COLORREF(0x006C_784B), track: COLORREF(0x0045_4A3B), outline: COLORREF(0x006C_784B),
};

/// Something in the panel the mouse is over.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum PanelHit {
    Key(Shortcut),
    StartWithWindows,
    Sound,
    Flash,
    Limit(usize),
    Idle(usize),
    Reset,
}

thread_local! {
    /// The pop-up window, for the keyboard hook to post to.
    static HOST: Cell<Option<HWND>> = const { Cell::new(None) };
    static LISTENING: Cell<Option<Shortcut>> = const { Cell::new(None) };
    /// A one-line result under the shortcuts: (text, is an error).
    static MESSAGE: RefCell<Option<(String, bool)>> = const { RefCell::new(None) };
    static START_WITH_WINDOWS: Cell<bool> = const { Cell::new(false) };
    static HOOK: Cell<Option<HHOOK>> = const { Cell::new(None) };
}

// ---------- layout (96-dpi units, scaled; `top` = where the panel starts) ----------
const PAD: i32 = 16;
const SECTION: i32 = 30;
const ROW: i32 = 42;
const MESSAGE_H: i32 = 24;
const LABEL: i32 = 26;
const CHIPS: i32 = 38;
const FOOTER: i32 = 60;

struct Layout {
    dpi: i32,
    sections: [i32; 3],
    keys: [RECT; 4],
    message: RECT,
    toggles: [RECT; 3],
    limit_label: RECT,
    limit_chips: Vec<RECT>,
    idle_label: RECT,
    idle_chips: Vec<RECT>,
    footer: i32,
    reset: RECT,
}

impl Layout {
    fn px(&self, v: i32) -> i32 {
        v * self.dpi / 96
    }
    fn switch(&self, row: &RECT) -> RECT {
        let (w, h) = (self.px(38), self.px(22));
        let top = row.top + (row.bottom - row.top - h) / 2;
        RECT { left: row.right - self.px(PAD) - w, top, right: row.right - self.px(PAD), bottom: top + h }
    }
}

/// Height of the panel at this scale (the pop-up adds its header on top).
pub fn height(dpi: u32) -> i32 {
    let p = |v: i32| v * dpi as i32 / 96;
    p(3 * SECTION + 4 * ROW + MESSAGE_H + 3 * ROW + 2 * (LABEL + CHIPS) + FOOTER)
}

fn layout(dpi: u32, top: i32, w: i32) -> Layout {
    let d = dpi as i32;
    let p = |v: i32| v * d / 96;
    let row = |t: i32| RECT { left: 0, top: t, right: w, bottom: t + p(ROW) };
    let chips = |t: i32, n: usize, cw: i32| -> Vec<RECT> {
        let (h, gap) = (p(28), p(8));
        let t = t + (p(CHIPS) - h) / 2;
        (0..n).map(|i| {
            let l = p(PAD) + i as i32 * (p(cw) + gap);
            RECT { left: l, top: t, right: l + p(cw), bottom: t + h }
        }).collect()
    };
    let mut y = top;
    let s0 = y;
    y += p(SECTION);
    let keys = [0, 1, 2, 3].map(|i| row(y + i * p(ROW)));
    y += 4 * p(ROW);
    let message = RECT { left: p(PAD), top: y, right: w - p(PAD), bottom: y + p(MESSAGE_H) };
    y += p(MESSAGE_H);
    let s1 = y;
    y += p(SECTION);
    let toggles = [0, 1, 2].map(|i| row(y + i * p(ROW)));
    y += 3 * p(ROW);
    let s2 = y;
    y += p(SECTION);
    let limit_label = RECT { left: p(PAD), top: y, right: w - p(PAD), bottom: y + p(LABEL) };
    let limit_chips = chips(y + p(LABEL), LIMITS_MB.len(), 76);
    y += p(LABEL + CHIPS);
    let idle_label = RECT { left: p(PAD), top: y, right: w - p(PAD), bottom: y + p(LABEL) };
    let idle_chips = chips(y + p(LABEL), IDLE_CHOICES.len(), 64);
    y += p(LABEL + CHIPS);
    let footer = y;
    let b = p(34);
    let reset = RECT { left: p(PAD), top: footer + (p(FOOTER) - b) / 2, right: p(PAD) + p(136), bottom: footer + (p(FOOTER) + b) / 2 };
    Layout { dpi: d, sections: [s0, s1, s2], keys, message, toggles, limit_label, limit_chips, idle_label, idle_chips, footer, reset }
}

fn inside(r: &RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

/// What's under the mouse at (x, y) in the pop-up.
pub fn hit(dpi: u32, top: i32, w: i32, x: i32, y: i32) -> Option<PanelHit> {
    let l = layout(dpi, top, w);
    if let Some(i) = l.keys.iter().position(|r| inside(r, x, y)) {
        return Some(PanelHit::Key(Shortcut::ALL[i]));
    }
    if let Some(i) = l.toggles.iter().position(|r| inside(r, x, y)) {
        return Some([PanelHit::StartWithWindows, PanelHit::Sound, PanelHit::Flash][i]);
    }
    if let Some(i) = l.limit_chips.iter().position(|r| inside(r, x, y)) {
        return Some(PanelHit::Limit(i));
    }
    if let Some(i) = l.idle_chips.iter().position(|r| inside(r, x, y)) {
        return Some(PanelHit::Idle(i));
    }
    inside(&l.reset, x, y).then_some(PanelHit::Reset)
}

// ---------- entering, leaving, clicks ----------

/// The panel is being shown in `host` (the queue pop-up).
pub fn enter(host: HWND) {
    HOST.with(|h| h.set(Some(host)));
    START_WITH_WINDOWS.with(|s| s.set(settings::starts_with_windows()));
    MESSAGE.with(|m| *m.borrow_mut() = None);
}

/// The panel is going away (back to the queue, or the pop-up closed).
pub fn leave() {
    stop_listening(None);
}

pub fn click(h: PanelHit) {
    match h {
        PanelHit::Key(sc) => {
            // Clicking the row that's listening cancels; another row switches to it.
            let current = LISTENING.with(|l| l.get());
            stop_listening(None);
            if current != Some(sc) {
                start_listening(sc);
            }
        }
        PanelHit::StartWithWindows => {
            let on = !START_WITH_WINDOWS.with(|s| s.get());
            let result = settings::set_start_with_windows(on);
            START_WITH_WINDOWS.with(|s| s.set(settings::starts_with_windows()));
            if let Err(e) = result {
                MESSAGE.with(|m| *m.borrow_mut() = Some((format!("Couldn't change start-up: {e}"), true)));
            }
        }
        PanelHit::Sound => settings::update(|s| s.sound = !s.sound),
        PanelHit::Flash => settings::update(|s| s.flash = !s.flash),
        PanelHit::Limit(i) => settings::update(|s| s.limit_mb = LIMITS_MB[i]),
        PanelHit::Idle(i) => settings::update(|s| s.idle_secs = IDLE_CHOICES[i]),
        PanelHit::Reset => {
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
            MESSAGE.with(|m| *m.borrow_mut() = Some(("Back to the defaults".into(), false)));
        }
    }
}

fn start_listening(sc: Shortcut) {
    hotkeys::suspend();
    unsafe {
        let hinst = GetModuleHandleW(None).map(|m| HINSTANCE(m.0)).unwrap_or_default();
        if let Ok(h) = SetWindowsHookExW(WH_KEYBOARD_LL, Some(heard_key_hook), hinst, 0) {
            HOOK.with(|x| x.set(Some(h)));
        }
    }
    LISTENING.with(|l| l.set(Some(sc)));
    MESSAGE.with(|m| *m.borrow_mut() = Some(("Press the new keys \u{00B7} Esc to cancel".into(), false)));
}

/// Stop listening and put the shortcuts back; `message` is the outcome.
fn stop_listening(message: Option<(String, bool)>) {
    if let Some(h) = HOOK.with(|x| x.take()) {
        unsafe {
            let _ = UnhookWindowsHookEx(h);
        }
    }
    if LISTENING.with(|l| l.take()).is_some() {
        hotkeys::resume();
        MESSAGE.with(|m| *m.borrow_mut() = message);
    }
}

/// The pop-up got WM_HEARD_KEY: a non-modifier key went down while listening.
pub fn heard_key(vk: u32) {
    let Some(sc) = LISTENING.with(|l| l.get()) else { return };
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

/// Is the key held right now? (Physical state: the hook runs before any window.)
fn down(vk: VIRTUAL_KEY) -> bool {
    unsafe { GetAsyncKeyState(vk.0 as i32) < 0 }
}

fn is_modifier(vk: u32) -> bool {
    matches!(vk, 0x10 | 0x11 | 0x12 | 0x5B | 0x5C | 0xA0..=0xA5)
}

/// The keyboard hook (only while listening). Modifiers pass through; the first
/// other key is kept from every app (so another app's shortcut doesn't fire) and
/// handed to the pop-up.
extern "system" fn heard_key_hook(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if code >= 0 && (wp.0 as u32 == WM_KEYDOWN || wp.0 as u32 == WM_SYSKEYDOWN) {
            let k = &*(lp.0 as *const KBDLLHOOKSTRUCT);
            // 0xE8 is our own harmless key (below); let it through.
            if k.vkCode != 0xE8 && !is_modifier(k.vkCode) {
                if let Some(h) = HOST.with(|h| h.get()) {
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

// ---------- drawing ----------

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
unsafe fn caps(hdc: HDC, l: &Layout, row: &RECT, label: &str, small: HFONT, c: &Colors) {
    let h = l.px(24);
    let top = row.top + (row.bottom - row.top - h) / 2;
    let mut x = row.right - l.px(PAD);
    let parts: Vec<&str> = label.split('+').collect();
    for (i, part) in parts.iter().enumerate().rev() {
        let w = width(hdc, part, small) + l.px(14);
        let r = RECT { left: x - w, top, right: x, bottom: top + h };
        crate::smooth::rounded(hdc, &r, l.px(6) as f32, c.cap, Some(c.cap_edge));
        text(hdc, part, r, small, c.text, DT_CENTER);
        x -= w;
        if i > 0 {
            text(hdc, "+", RECT { left: x - l.px(13), top, right: x, bottom: top + h }, small, c.muted, DT_CENTER);
            x -= l.px(13);
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
            crate::smooth::rounded(hdc, r, l.px(7) as f32, if hover == Some(i) { c.hover } else { c.bg }, Some(c.border));
            text(hdc, &labels[i], *r, small, c.text, DT_CENTER);
        }
    }
}

/// Draw the panel from `top` down, `w` wide. Fonts: body, small.
pub unsafe fn render(hdc: HDC, dpi: u32, top: i32, w: i32, light: bool, [f_body, f_small]: [HFONT; 2], hover: Option<PanelHit>) {
    let l = layout(dpi, top, w);
    let c = if light { &LIGHT } else { &DARK };
    let st = settings::get();
    let pad = l.px(PAD);
    let listening = LISTENING.with(|x| x.get());

    for (y, name) in l.sections.iter().zip(["SHORTCUTS", "GENERAL", "QUEUE"]) {
        text(hdc, name, RECT { left: pad, top: *y + l.px(8), right: w, bottom: *y + l.px(SECTION) }, f_small, c.muted, DT_LEFT);
    }

    for (i, sc) in Shortcut::ALL.into_iter().enumerate() {
        let r = l.keys[i];
        if listening == Some(sc) || hover == Some(PanelHit::Key(sc)) {
            fill(hdc, &r, c.hover);
        }
        text(hdc, sc.title(), RECT { left: pad, ..r }, f_body, c.text, DT_LEFT);
        if listening == Some(sc) {
            let label = "Press keys\u{2026}";
            let cw = width(hdc, label, f_small) + l.px(18);
            let h = l.px(24);
            let t = r.top + (l.px(ROW) - h) / 2;
            let cap = RECT { left: r.right - pad - cw, top: t, right: r.right - pad, bottom: t + h };
            crate::smooth::rounded(hdc, &cap, l.px(6) as f32, c.cap, Some(c.primary));
            text(hdc, label, cap, f_small, c.text, DT_CENTER);
        } else if hotkeys::is_active(sc) {
            caps(hdc, &l, &r, &hotkeys::label(sc), f_small, c);
        } else {
            text(hdc, "Not active \u{00B7} click to set", RECT { right: r.right - pad, ..r }, f_small, c.danger, DT_RIGHT);
        }
    }
    if let Some((m, is_error)) = MESSAGE.with(|m| m.borrow().clone()) {
        text(hdc, &m, l.message, f_small, if is_error { c.danger } else { c.success }, DT_RIGHT);
    }

    let on_start = START_WITH_WINDOWS.with(|s| s.get());
    let toggles = [
        ("Start with Windows", on_start, PanelHit::StartWithWindows),
        ("Capture sound", st.sound, PanelHit::Sound),
        ("Capture flash", st.flash, PanelHit::Flash),
    ];
    for (r, (name, on, h)) in l.toggles.iter().zip(toggles) {
        if hover == Some(h) {
            fill(hdc, r, c.hover);
        }
        text(hdc, name, RECT { left: pad, ..*r }, f_body, c.text, DT_LEFT);
        switch(hdc, &l, &l.switch(r), on, c);
    }

    text(hdc, "Memory limit", l.limit_label, f_body, c.text, DT_LEFT);
    let labels: Vec<String> = LIMITS_MB.iter().map(|m| format!("{m} MB")).collect();
    let sel = LIMITS_MB.iter().position(|m| *m == st.limit_mb).unwrap_or(1);
    let hov = match hover { Some(PanelHit::Limit(i)) => Some(i), _ => None };
    chips(hdc, &l, &l.limit_chips, &labels, sel, hov, f_small, c);

    text(hdc, "Hide queue when idle", l.idle_label, f_body, c.text, DT_LEFT);
    let labels: Vec<String> = IDLE_CHOICES.iter().map(|s| if *s == 0 { "Never".into() } else { format!("{s} s") }).collect();
    let sel = IDLE_CHOICES.iter().position(|x| *x == st.idle_secs).unwrap_or(1);
    let hov = match hover { Some(PanelHit::Idle(i)) => Some(i), _ => None };
    chips(hdc, &l, &l.idle_chips, &labels, sel, hov, f_small, c);

    fill(hdc, &RECT { left: 0, top: l.footer, right: w, bottom: l.footer + l.px(1).max(1) }, c.border);
    crate::smooth::rounded(hdc, &l.reset, l.px(8) as f32, if hover == Some(PanelHit::Reset) { c.hover } else { c.bg }, Some(c.outline));
    text(hdc, "Reset to defaults", l.reset, f_small, c.text, DT_CENTER);
    text(hdc, "Changes apply right away", RECT { left: l.reset.right, top: l.footer, right: w - pad, bottom: l.footer + l.px(FOOTER) }, f_small, c.muted, DT_RIGHT);
}
