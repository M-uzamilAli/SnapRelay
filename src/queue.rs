//! Queue window (Win+Shift+Q): a compact pop-up in the bottom-right corner listing
//! what's queued (numbered 1, 2, 3 by position), with a remove button per
//! screenshot, "Clear" and "Send".
//!
//! It never takes focus (like the on-screen keyboard), so the chat box keeps its
//! cursor and "Send" can paste straight into it. Mouse only.
//!
//! The window doesn't change the queue itself: a click records an Action, and the
//! Application Controller (main loop) carries it out, then calls `sync`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::mem::size_of;
use std::sync::Once;
use std::time::{Duration, Instant};

use windows::core::w;
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW, CreatePen,
    CreateRoundRectRgn, CreateSolidBrush, DeleteDC, DeleteObject, DrawTextW, EndPaint, FillRect,
    GetMonitorInfoW, GetStockObject, InvalidateRect, LineTo, MonitorFromPoint, MoveToEx, RoundRect, SelectObject,
    SetBkMode, SetTextColor, SetWindowRgn, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS,
    DEFAULT_CHARSET, DRAW_TEXT_FORMAT, DT_CENTER, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX,
    DT_SINGLELINE, DT_VCENTER, FW_NORMAL, FW_SEMIBOLD, HBITMAP, HDC, HFONT, MONITORINFO,
    MONITOR_DEFAULTTONEAREST, NULL_BRUSH, NULL_PEN, OUT_DEFAULT_PRECIS, PAINTSTRUCT, PS_SOLID, SRCCOPY,
    TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetClientRect, GetCursorPos, GetWindowRect, LoadCursorW,
    RegisterClassW, KillTimer, SetCursor, SetTimer, SetWindowDisplayAffinity, SetWindowPos, ShowWindow, HMENU,
    HWND_TOPMOST, IDC_ARROW, IDC_HAND, MA_NOACTIVATE, SWP_NOACTIVATE, SWP_SHOWWINDOW, SW_HIDE,
    WDA_EXCLUDEFROMCAPTURE, WM_ERASEBKGND, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE,
    WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_PAINT, WM_SETCURSOR, WM_TIMER, WNDCLASSW, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use crate::buffer::Buffer;
use crate::capture;
use crate::select::make_bitmap;

/// Sent when the mouse leaves the window (after TrackMouseEvent).
const WM_MOUSELEAVE: u32 = 0x02A3;

/// What the user asked for; carried out by the main loop.
#[derive(Clone, Copy)]
pub enum Action {
    Remove(u32),
    ClearAll,
    SendAll,
}

// Sizes in 96-dpi units, scaled to the monitor.
const WIDTH: i32 = 360;
const HEADER: i32 = 52;
const ROW: i32 = 72;
const FOOTER: i32 = 64;
const PAD: i32 = 16;
const EMPTY: i32 = 100;
const THUMB_W: i32 = 96;
const THUMB_H: i32 = 54;
const BUTTON: i32 = 28;
const MAX_ROWS: usize = 6;
/// Close the pop-up after this long without activity (never while the mouse is over it).
const IDLE_SECS: u64 = 8;
const IDLE_TIMER: usize = 7;

// Brand colours (0x00BBGGRR), one set per Windows theme.
// Slate-Forest / Linen surfaces, sage + eucalyptus edges, Warm Champagne for "Send"
// (the relay-to-AI action).
struct Colors {
    bg: COLORREF,
    hover: COLORREF,
    border: COLORREF,
    outline: COLORREF,
    text: COLORREF,
    muted: COLORREF,
    primary: COLORREF,
    primary_hover: COLORREF,
    on_primary: COLORREF,
    danger: COLORREF,
    thumb_bg: COLORREF,
    disabled_fill: COLORREF,
    disabled_text: COLORREF,
}

const DARK: Colors = Colors {
    bg: COLORREF(0x0025_281C),            // Slate-Forest #1C2825
    hover: COLORREF(0x002F_3324),         // #24332F
    border: COLORREF(0x003B_402E),        // #2E403B
    outline: COLORREF(0x006C_784B),       // Muted Sage #4B786C
    text: COLORREF(0x00F4_F6F5),          // Linen #F5F6F4
    muted: COLORREF(0x00B2_B8A9),         // #A9B8B2
    primary: COLORREF(0x0084_C3E5),       // Warm Champagne #E5C384
    primary_hover: COLORREF(0x00A0_D3ED), // #EDD3A0
    on_primary: COLORREF(0x0025_281C),    // Slate-Forest
    danger: COLORREF(0x006E_7AE0),        // #E07A6E
    thumb_bg: COLORREF(0x001B_1D14),      // #141D1B
    disabled_fill: COLORREF(0x002F_3324),
    disabled_text: COLORREF(0x006B_715E), // #5E716B
};

const LIGHT: Colors = Colors {
    bg: COLORREF(0x00F4_F6F5),            // Linen #F5F6F4
    hover: COLORREF(0x00E8_ECEA),         // Stone #EAECE8
    border: COLORREF(0x00DE_E3E0),        // #E0E3DE
    outline: COLORREF(0x0094_A07A),       // Pale Eucalyptus #7AA094
    text: COLORREF(0x0025_281C),          // Slate-Forest #1C2825
    muted: COLORREF(0x0066_6B5A),         // #5A6B66
    primary: COLORREF(0x0084_C3E5),       // Warm Champagne #E5C384
    primary_hover: COLORREF(0x006E_B6DD), // #DDB66E
    on_primary: COLORREF(0x0025_281C),    // Slate-Forest
    danger: COLORREF(0x003D_48B5),        // #B5483D
    thumb_bg: COLORREF(0x00E8_ECEA),      // Stone
    disabled_fill: COLORREF(0x00EC_F0EE), // #EEF0EC
    disabled_text: COLORREF(0x00A9_ADA3), // #A3ADA9
};

#[derive(Clone, Copy, PartialEq)]
enum Hit {
    Nothing,
    Close,
    Remove(u32),
    Clear,
    Send,
    /// The scrollbar's draggable handle, and the track around it.
    Thumb,
    Track,
}

impl Hit {
    fn is_button(self) -> bool {
        matches!(self, Hit::Close | Hit::Remove(_) | Hit::Clear | Hit::Send)
    }
}

struct Row {
    id: u32,
    /// Capture time, "20:41". The row's number is its position, worked out when drawn.
    time: String,
    thumb: Option<(HBITMAP, i32, i32)>,
}

struct State {
    hwnd: Option<HWND>,
    open: bool,
    dpi: u32,
    rows: Vec<Row>,
    seen_version: u64,
    scroll: usize,
    hover: Hit,
    tracking: bool,
    title: String,
    /// Shortcuts that actually registered, shown when the queue is empty.
    hint: String,
    action: Option<Action>,
    /// Type scale: title, button, meta (time, hint), number.
    fonts: Option<[HFONT; 4]>,
    /// While dragging the scrollbar: where on the handle it was grabbed.
    dragging: Option<i32>,
    last_activity: Instant,
    /// Follows the Windows light/dark setting, read each time the window opens.
    light: bool,
}

impl Default for State {
    fn default() -> Self {
        State {
            hwnd: None, open: false, dpi: 96, rows: Vec::new(), seen_version: u64::MAX, scroll: 0,
            hover: Hit::Nothing, tracking: false, title: String::new(), hint: String::new(), action: None, fonts: None, dragging: None, last_activity: Instant::now(), light: true,
        }
    }
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

// ---------- public API (called by the Application Controller) ----------

pub fn toggle(buf: &Buffer) {
    if is_open() {
        hide();
    } else {
        show(buf);
    }
}

/// The capture shortcuts that are really active (they may be fallbacks).
pub fn set_hint(hint: String) {
    STATE.with(|s| s.borrow_mut().hint = hint);
}

pub fn is_open() -> bool {
    STATE.with(|s| s.try_borrow().map(|s| s.open).unwrap_or(false))
}

/// The button the user clicked, if any. The main loop carries it out.
pub fn take_action() -> Option<Action> {
    STATE.with(|s| s.borrow_mut().action.take())
}

/// Where the window is, so notices can sit above it.
pub fn visible_rect() -> Option<RECT> {
    let hwnd = STATE.with(|s| s.try_borrow().ok().filter(|s| s.open).and_then(|s| s.hwnd))?;
    let mut r = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut r).ok()? };
    Some(r)
}

/// Redraw if the queue changed since we last looked.
pub fn sync(buf: &Buffer) {
    let changed = STATE.with(|s| {
        let s = s.borrow();
        s.open && s.seen_version != buf.version()
    });
    if changed {
        // A new capture or removal counts as activity.
        STATE.with(|s| s.borrow_mut().last_activity = Instant::now());
        rebuild(buf);
        place(false);
    }
}

pub fn show(buf: &Buffer) {
    unsafe {
        let hwnd = window();
        // Size for the monitor the mouse is on.
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let (mut dpi, mut _y) = (96u32, 96u32);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dpi, &mut _y);
        // Windows 11's interface font: "Display" cut for the title, "Text" cut for the rest.
        let fonts = [
            font(dpi, 16, FW_SEMIBOLD.0, w!("Segoe UI Variable Display")), // title
            font(dpi, 14, FW_SEMIBOLD.0, w!("Segoe UI Variable Text")),    // buttons
            font(dpi, 13, FW_NORMAL.0, w!("Segoe UI Variable Text")),      // time, hint
            font(dpi, 12, FW_SEMIBOLD.0, w!("Segoe UI Variable Text")),    // row number
        ];
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            s.hwnd = Some(hwnd);
            s.dpi = dpi;
            s.fonts = Some(fonts);
            s.scroll = 0;
            s.hover = Hit::Nothing;
            s.dragging = None;
            s.open = true;
            s.last_activity = Instant::now();
            s.light = crate::theme::is_light();
        });
    }
    rebuild(buf);
    place(true);
    if let Some(h) = STATE.with(|s| s.borrow().hwnd) {
        unsafe { SetTimer(h, IDLE_TIMER, 1000, None) };
    }
}

pub fn hide() {
    let (hwnd, thumbs, fonts) = STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.open = false;
        let thumbs: Vec<HBITMAP> = s.rows.drain(..).filter_map(|r| r.thumb.map(|t| t.0)).collect();
        (s.hwnd, thumbs, s.fonts.take())
    });
    unsafe {
        if let Some(h) = hwnd {
            let _ = KillTimer(h, IDLE_TIMER);
            let _ = ShowWindow(h, SW_HIDE);
        }
        // Thumbnails and fonts only live while the window is open.
        for b in thumbs {
            let _ = DeleteObject(b);
        }
        for f in fonts.into_iter().flatten() {
            let _ = DeleteObject(f);
        }
    }
}

// ---------- building and placing ----------

/// Refresh rows from the queue. Thumbnails are reused by id; new ones are made now.
fn rebuild(buf: &Buffer) {
    let (dpi, mut old) = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let old: HashMap<u32, (HBITMAP, i32, i32)> =
            s.rows.drain(..).filter_map(|r| r.thumb.map(|t| (r.id, t))).collect();
        (s.dpi, old)
    });
    let p = |v: i32| v * dpi as i32 / 96;
    let rows: Vec<Row> = buf
        .items()
        .iter()
        .map(|shot| {
            let thumb = old.remove(&shot.id).or_else(|| {
                let (tw, th, px) = capture::thumbnail(&shot.frame.png, p(THUMB_W) as u32, p(THUMB_H) as u32).ok()?;
                unsafe { make_bitmap(tw, th, &px) }.map(|b| (b, tw as i32, th as i32))
            });
            Row {
                id: shot.id,
                time: shot.time.clone(),
                thumb,
            }
        })
        .collect();
    for (_, (b, _, _)) in old {
        unsafe {
            let _ = DeleteObject(b);
        }
    }
    let count = buf.summary().0;
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.title = match count {
            0 => "Nothing queued".into(),
            1 => "1 screenshot".into(),
            n => format!("{n} screenshots"),
        };
        s.scroll = s.scroll.min(count.saturating_sub(MAX_ROWS));
        s.rows = rows;
        s.seen_version = buf.version();
    });
}

/// Size the window for its rows. On first show, put it in the bottom-right corner
/// above the taskbar; afterwards keep its bottom-right corner where it is.
fn place(first: bool) {
    let Some((hwnd, l)) = STATE.with(|s| {
        let s = s.borrow();
        s.hwnd.map(|h| (h, layout(&s)))
    }) else {
        return;
    };
    unsafe {
        let (right, bottom) = if first {
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
            let _ = GetMonitorInfoW(mon, &mut info);
            (info.rcWork.right - l.pad - 4, info.rcWork.bottom - l.pad - 4)
        } else {
            let mut r = RECT::default();
            let _ = GetWindowRect(hwnd, &mut r);
            (r.right, r.bottom)
        };
        let _ = SetWindowPos(hwnd, HWND_TOPMOST, right - l.w, bottom - l.h, l.w, l.h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
        SetWindowRgn(hwnd, CreateRoundRectRgn(0, 0, l.w + 1, l.h + 1, l.pad, l.pad), true);
        let _ = InvalidateRect(hwnd, None, false);
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
            lpfnWndProc: Some(queue_proc),
            hInstance: hinst,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: w!("SnapRelayQueue"),
            ..Default::default()
        });
    });
    let hwnd = CreateWindowExW(
        WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
        w!("SnapRelayQueue"), w!("SnapRelay queue"), WS_POPUP,
        0, 0, 10, 10, HWND::default(), HMENU::default(), hinst, None,
    )
    .expect("couldn't create the queue window");
    // Keep SnapRelay's own window out of the screenshots it takes.
    // (Set SNAPRELAY_VISIBLE_IN_SCREENSHOTS=1 to turn this off, e.g. to take screenshots for docs/tests.)
    if std::env::var_os("SNAPRELAY_VISIBLE_IN_SCREENSHOTS").is_none() {
        let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
    }
    STATE.with(|s| s.borrow_mut().hwnd = Some(hwnd));
    hwnd
}

unsafe fn font(dpi: u32, size: i32, weight: u32, face: windows::core::PCWSTR) -> HFONT {
    CreateFontW(
        -(size * dpi as i32 / 96), 0, 0, 0, weight as i32, 0, 0, 0,
        DEFAULT_CHARSET.0 as u32, OUT_DEFAULT_PRECIS.0 as u32, CLIP_DEFAULT_PRECIS.0 as u32,
        CLEARTYPE_QUALITY.0 as u32, 0, face,
    )
}

// ---------- layout and hit-testing ----------

struct Layout {
    w: i32,
    h: i32,
    pad: i32,
    header: i32,
    row: i32,
    body: i32,
    visible: usize,
    close: RECT,
    clear: RECT,
    send: RECT,
    p: i32, // 1 unit at this scale (for small offsets)
    dpi: i32,
}

impl Layout {
    fn px(&self, v: i32) -> i32 {
        v * self.dpi / 96
    }
    fn row_rect(&self, j: usize) -> RECT {
        let top = self.header + j as i32 * self.row;
        RECT { left: 0, top, right: self.w, bottom: top + self.row }
    }
    /// Slim scrollbar along the right edge of the list.
    fn track(&self) -> RECT {
        RECT {
            left: self.w - self.px(11),
            top: self.header + self.px(8),
            right: self.w - self.px(5),
            bottom: self.header + self.body - self.px(8),
        }
    }
    /// The handle: as tall as the visible share of the list, at the scroll position.
    fn thumb(&self, total: usize, scroll: usize) -> RECT {
        let t = self.track();
        let track_h = t.bottom - t.top;
        let h = ((track_h as f32) * MAX_ROWS as f32 / total.max(1) as f32) as i32;
        let h = h.max(self.px(28)).min(track_h);
        let max = total.saturating_sub(MAX_ROWS);
        let top = if max == 0 { t.top } else { t.top + ((track_h - h) as f32 * scroll as f32 / max as f32).round() as i32 };
        RECT { left: t.left, top, right: t.right, bottom: top + h }
    }
    /// Scroll position for a handle whose top edge is at `top`.
    fn scroll_for(&self, total: usize, top: i32) -> usize {
        let t = self.track();
        let h = self.thumb(total, 0).bottom - t.top;
        let room = (t.bottom - t.top - h).max(1);
        let max = total.saturating_sub(MAX_ROWS);
        (((top - t.top) as f32 / room as f32) * max as f32).round().clamp(0.0, max as f32) as usize
    }
    fn remove_rect(&self, j: usize) -> RECT {
        let r = self.row_rect(j);
        let b = self.px(BUTTON);
        let top = r.top + (self.row - b) / 2;
        RECT { left: self.w - self.pad - b, top, right: self.w - self.pad, bottom: top + b }
    }
}

fn layout(s: &State) -> Layout {
    let dpi = s.dpi as i32;
    let p = |v: i32| v * dpi / 96;
    let visible = s.rows.len().min(MAX_ROWS);
    let body = if s.rows.is_empty() { p(EMPTY) } else { visible as i32 * p(ROW) };
    let (w, pad) = (p(WIDTH), p(PAD));
    let h = p(HEADER) + body + p(FOOTER);
    let fy = p(HEADER) + body + p(14);
    Layout {
        w, h, pad, header: p(HEADER), row: p(ROW), body, visible,
        close: RECT { left: w - pad - p(BUTTON), top: (p(HEADER) - p(BUTTON)) / 2, right: w - pad, bottom: (p(HEADER) + p(BUTTON)) / 2 },
        clear: RECT { left: pad, top: fy, right: pad + p(88), bottom: fy + p(36) },
        send: RECT { left: w - pad - p(120), top: fy, right: w - pad, bottom: fy + p(36) },
        p: p(1).max(1),
        dpi,
    }
}

fn inside(r: &RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

fn hit(s: &State, x: i32, y: i32) -> Hit {
    let l = layout(s);
    if inside(&l.close, x, y) {
        return Hit::Close;
    }
    if s.rows.len() > MAX_ROWS {
        let t = l.track();
        // A generous target, wider than the drawn bar.
        let zone = RECT { left: t.left - l.px(5), top: t.top, right: l.w, bottom: t.bottom };
        if inside(&zone, x, y) {
            let th = l.thumb(s.rows.len(), s.scroll);
            return if y >= th.top && y < th.bottom { Hit::Thumb } else { Hit::Track };
        }
    }
    for j in 0..l.visible {
        if inside(&l.remove_rect(j), x, y) {
            return Hit::Remove(s.rows[s.scroll + j].id);
        }
    }
    if !s.rows.is_empty() {
        if inside(&l.clear, x, y) {
            return Hit::Clear;
        }
        if inside(&l.send, x, y) {
            return Hit::Send;
        }
    }
    Hit::Nothing
}

fn point(lp: LPARAM) -> (i32, i32) {
    ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32)
}

// ---------- window messages ----------

extern "system" fn queue_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            // Clicking must never make this the active window (the chat box keeps focus).
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => {
                paint(hwnd);
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let (x, y) = point(lp);
                let (changed, start_tracking) = STATE.with(|s| {
                    let Ok(mut s) = s.try_borrow_mut() else { return (false, false) };
                    if let Some(grab) = s.dragging {
                        let l = layout(&s);
                        let next = l.scroll_for(s.rows.len(), y - grab);
                        let changed = next != s.scroll;
                        s.scroll = next;
                        s.last_activity = Instant::now();
                        return (changed, false);
                    }
                    let h = hit(&s, x, y);
                    let changed = h != s.hover;
                    s.hover = h;
                    s.last_activity = Instant::now();
                    let start = !s.tracking;
                    s.tracking = true;
                    (changed, start)
                });
                if start_tracking {
                    let mut t = TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    let _ = TrackMouseEvent(&mut t);
                }
                if changed {
                    let _ = InvalidateRect(hwnd, None, false);
                }
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                STATE.with(|s| {
                    if let Ok(mut s) = s.try_borrow_mut() {
                        s.hover = Hit::Nothing;
                        s.tracking = false;
                    }
                });
                let _ = InvalidateRect(hwnd, None, false);
                LRESULT(0)
            }
            WM_SETCURSOR => {
                let on_button = STATE.with(|s| s.try_borrow().map(|s| s.hover.is_button()).unwrap_or(false));
                if on_button {
                    SetCursor(LoadCursorW(None, IDC_HAND).unwrap_or_default());
                    LRESULT(1)
                } else {
                    DefWindowProcW(hwnd, msg, wp, lp)
                }
            }
            WM_LBUTTONDOWN => {
                let (x, y) = point(lp);
                let grabbed = STATE.with(|s| {
                    let Ok(mut s) = s.try_borrow_mut() else { return false };
                    let l = layout(&s);
                    let th = l.thumb(s.rows.len(), s.scroll);
                    match hit(&s, x, y) {
                        Hit::Thumb => {
                            s.dragging = Some(y - th.top);
                            true
                        }
                        Hit::Track => {
                            // Jump a page towards the click.
                            let max = s.rows.len().saturating_sub(MAX_ROWS);
                            s.scroll = if y < th.top { s.scroll.saturating_sub(MAX_ROWS) } else { (s.scroll + MAX_ROWS).min(max) };
                            false
                        }
                        _ => false,
                    }
                });
                if grabbed {
                    SetCapture(hwnd);
                }
                let _ = InvalidateRect(hwnd, None, false);
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let (x, y) = point(lp);
                let was_dragging = STATE.with(|s| s.borrow_mut().dragging.take().is_some());
                if was_dragging {
                    let _ = ReleaseCapture();
                    let _ = InvalidateRect(hwnd, None, false);
                    return LRESULT(0);
                }
                let clicked = STATE.with(|s| s.try_borrow().map(|s| hit(&s, x, y)).unwrap_or(Hit::Nothing));
                match clicked {
                    Hit::Close => hide(),
                    Hit::Remove(id) => STATE.with(|s| s.borrow_mut().action = Some(Action::Remove(id))),
                    Hit::Clear => STATE.with(|s| s.borrow_mut().action = Some(Action::ClearAll)),
                    Hit::Send => STATE.with(|s| s.borrow_mut().action = Some(Action::SendAll)),
                    Hit::Thumb | Hit::Track | Hit::Nothing => {}
                }
                LRESULT(0)
            }
            WM_MOUSEWHEEL => {
                let delta = ((wp.0 >> 16) & 0xFFFF) as i16;
                STATE.with(|s| {
                    if let Ok(mut s) = s.try_borrow_mut() {
                        s.last_activity = Instant::now();
                        let max = s.rows.len().saturating_sub(MAX_ROWS);
                        s.scroll = if delta > 0 { s.scroll.saturating_sub(1) } else { (s.scroll + 1).min(max) };
                    }
                });
                let _ = InvalidateRect(hwnd, None, false);
                LRESULT(0)
            }
            WM_TIMER if wp.0 == IDLE_TIMER => {
                let idle = STATE.with(|s| {
                    let Ok(mut s) = s.try_borrow_mut() else { return false };
                    if s.tracking {
                        // The mouse is over the pop-up: the user is looking at it.
                        s.last_activity = Instant::now();
                        return false;
                    }
                    s.last_activity.elapsed() >= Duration::from_secs(IDLE_SECS)
                });
                if idle {
                    hide();
                }
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

// ---------- drawing ----------

unsafe fn fill(hdc: HDC, r: &RECT, c: COLORREF) {
    let b = CreateSolidBrush(c);
    FillRect(hdc, r, b);
    let _ = DeleteObject(b);
}

/// A 1px rounded border with no fill.
unsafe fn outline(hdc: HDC, r: &RECT, c: COLORREF, radius: i32) {
    let pen = CreatePen(PS_SOLID, 1, c);
    let old_p = SelectObject(hdc, pen);
    let old_b = SelectObject(hdc, GetStockObject(NULL_BRUSH));
    let _ = RoundRect(hdc, r.left, r.top, r.right, r.bottom, radius, radius);
    SelectObject(hdc, old_p);
    SelectObject(hdc, old_b);
    let _ = DeleteObject(pen);
}

unsafe fn round(hdc: HDC, r: &RECT, fill_c: COLORREF, border: Option<COLORREF>, radius: i32) {
    let brush = CreateSolidBrush(fill_c);
    let pen = border.map(|c| CreatePen(PS_SOLID, 1, c));
    let old_b = SelectObject(hdc, brush);
    let old_p = match pen {
        Some(p) => SelectObject(hdc, p),
        None => SelectObject(hdc, GetStockObject(NULL_PEN)),
    };
    let _ = RoundRect(hdc, r.left, r.top, r.right, r.bottom, radius, radius);
    SelectObject(hdc, old_b);
    SelectObject(hdc, old_p);
    let _ = DeleteObject(brush);
    if let Some(p) = pen {
        let _ = DeleteObject(p);
    }
}

/// A crisp "x" drawn with lines, centred in `r` (font glyphs look fuzzy at this size).
unsafe fn draw_x(hdc: HDC, r: &RECT, c: COLORREF, size: i32, thickness: i32) {
    let (cx, cy, half) = ((r.left + r.right) / 2, (r.top + r.bottom) / 2, size / 2);
    let pen = CreatePen(PS_SOLID, thickness.max(1), c);
    let old = SelectObject(hdc, pen);
    let _ = MoveToEx(hdc, cx - half, cy - half, None);
    let _ = LineTo(hdc, cx + half, cy + half);
    let _ = MoveToEx(hdc, cx + half, cy - half, None);
    let _ = LineTo(hdc, cx - half, cy + half);
    SelectObject(hdc, old);
    let _ = DeleteObject(pen);
}

unsafe fn text(hdc: HDC, s: &str, r: RECT, font: HFONT, c: COLORREF, flags: DRAW_TEXT_FORMAT) {
    let old = SelectObject(hdc, font);
    SetTextColor(hdc, c);
    let mut wide: Vec<u16> = s.encode_utf16().collect();
    let mut r = r;
    DrawTextW(hdc, &mut wide, &mut r, flags | DT_NOPREFIX | DT_SINGLELINE | DT_VCENTER);
    SelectObject(hdc, old);
}

unsafe fn paint(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    STATE.with(|s| {
        let Ok(s) = s.try_borrow() else { return };
        let Some([f_title, f_button, f_meta, f_number]) = s.fonts else { return };
        let mut rc = RECT::default();
        let _ = GetClientRect(hwnd, &mut rc);
        let l = layout(&s);
        let c = if s.light { &LIGHT } else { &DARK };
        // Draw off-screen, then copy once: no flicker.
        let mem = CreateCompatibleDC(hdc);
        let bmp = CreateCompatibleBitmap(hdc, rc.right, rc.bottom);
        let old_bmp = SelectObject(mem, bmp);
        SetBkMode(mem, TRANSPARENT);
        fill(mem, &rc, c.bg);

        // Header
        let (pad, w) = (l.pad, l.w);
        text(mem, &s.title, RECT { left: pad, top: 0, right: l.close.left - l.px(8), bottom: l.header }, f_title, c.text, DT_LEFT | DT_END_ELLIPSIS);
        if s.hover == Hit::Close {
            round(mem, &l.close, c.hover, None, l.px(8));
        }
        draw_x(mem, &l.close, if s.hover == Hit::Close { c.text } else { c.muted }, l.px(10), (l.px(3) / 2).max(2));
        fill(mem, &RECT { left: 0, top: l.header - l.p, right: w, bottom: l.header }, c.border);

        // Rows (or the empty state)
        if s.rows.is_empty() {
            let mid = l.header + l.body / 2;
            text(mem, &s.hint, RECT { left: pad, top: mid - l.px(12), right: w - pad, bottom: mid + l.px(12) }, f_meta, c.muted, DT_CENTER | DT_END_ELLIPSIS);
        }
        for j in 0..l.visible {
            let row = &s.rows[s.scroll + j];
            let rr = l.row_rect(j);
            let remove_hover = s.hover == Hit::Remove(row.id);
            if remove_hover {
                fill(mem, &rr, c.hover);
            }
            // Thumbnail, centred in its box
            let num = RECT { left: pad, top: rr.top, right: pad + l.px(18), bottom: rr.bottom };
            text(mem, &(s.scroll + j + 1).to_string(), num, f_number, c.muted, DT_LEFT);
            let tl = num.right + l.px(8);
            let tb = RECT { left: tl, top: rr.top + (l.row - l.px(THUMB_H)) / 2, right: tl + l.px(THUMB_W), bottom: rr.top + (l.row + l.px(THUMB_H)) / 2 };
            fill(mem, &tb, c.thumb_bg);
            if let Some((tbmp, tw, th)) = row.thumb {
                let src = CreateCompatibleDC(mem);
                let old = SelectObject(src, tbmp);
                let x = tb.left + (tb.right - tb.left - tw) / 2;
                let y = tb.top + (tb.bottom - tb.top - th) / 2;
                let _ = BitBlt(mem, x, y, tw, th, src, 0, 0, SRCCOPY);
                SelectObject(src, old);
                let _ = DeleteDC(src);
            }
            let tx = tb.right + l.px(12);
            let rb = l.remove_rect(j);
            text(mem, &row.time, RECT { left: tx, top: rr.top, right: rb.left - l.px(8), bottom: rr.bottom }, f_meta, c.muted, DT_LEFT);
            if remove_hover {
                round(mem, &rb, c.bg, None, l.px(8));
            }
            draw_x(mem, &rb, if remove_hover { c.danger } else { c.muted }, l.px(9), (l.px(3) / 2).max(2));
        }

        // Scrollbar, only when there's more than fits
        if s.rows.len() > MAX_ROWS {
            let t = l.track();
            let radius = t.right - t.left;
            round(mem, &t, c.border, None, radius);
            let active = s.dragging.is_some() || matches!(s.hover, Hit::Thumb | Hit::Track);
            round(mem, &l.thumb(s.rows.len(), s.scroll), if active { c.muted } else { c.outline }, None, radius);
        }

        // Footer
        let ft = l.header + l.body;
        fill(mem, &RECT { left: 0, top: ft, right: w, bottom: ft + l.p }, c.border);
        let enabled = !s.rows.is_empty();
        let clear_fill = if enabled && s.hover == Hit::Clear { c.hover } else { c.bg };
        round(mem, &l.clear, clear_fill, Some(c.outline), l.px(10));
        text(mem, "Clear", l.clear, f_button, if enabled { c.text } else { c.disabled_text }, DT_CENTER);
        let send_fill = if !enabled { c.disabled_fill } else if s.hover == Hit::Send { c.primary_hover } else { c.primary };
        round(mem, &l.send, send_fill, None, l.px(10));
        let label = "Send";
        text(mem, label, l.send, f_button, if enabled { c.on_primary } else { c.disabled_text }, DT_CENTER);

        // A light pop-up needs an edge to stand out on white pages.
        outline(mem, &rc, c.outline, l.pad);
        let _ = BitBlt(hdc, 0, 0, rc.right, rc.bottom, mem, 0, 0, SRCCOPY);
        SelectObject(mem, old_bmp);
        let _ = DeleteObject(bmp);
        let _ = DeleteDC(mem);
    });
    let _ = EndPaint(hwnd, &ps);
}
