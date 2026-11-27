//! Recording mode (tray menu switch, off by default), for making demo videos.
//!
//! Normally SnapRelay's own windows (queue, notices) are hidden from every screen
//! capture, so they never end up in your screenshots. A screen recorder is a
//! screen capture too, so in recording mode they're made visible, and the
//! shortcuts you press are shown at the bottom of the screen (keys.rs).
//! SnapRelay's own screenshots still leave them out: they're hidden from capture
//! for the instant the screen is copied.

use std::cell::{Cell, RefCell};
use std::thread::sleep;
use std::time::Duration;

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WDA_NONE, WINDOW_DISPLAY_AFFINITY,
};

thread_local! {
    static ON: Cell<bool> = const { Cell::new(false) };
    /// SnapRelay's own on-screen windows.
    static WINDOWS: RefCell<Vec<HWND>> = const { RefCell::new(Vec::new()) };
}

/// Start in recording mode if SNAPRELAY_VISIBLE_IN_SCREENSHOTS is set (for tests and docs).
pub fn init() {
    if std::env::var_os("SNAPRELAY_VISIBLE_IN_SCREENSHOTS").is_some() {
        ON.with(|o| o.set(true));
    }
}

pub fn is_on() -> bool {
    ON.with(|o| o.get())
}

pub fn set(on: bool) {
    ON.with(|o| o.set(on));
    apply_all(affinity());
}

/// One of SnapRelay's windows: hide it from captures unless recording.
pub fn register(hwnd: HWND) {
    WINDOWS.with(|w| w.borrow_mut().push(hwnd));
    unsafe {
        let _ = SetWindowDisplayAffinity(hwnd, affinity());
    }
}

/// Run a screen copy with SnapRelay's windows left out, even in recording mode.
pub fn hidden<T>(copy: impl FnOnce() -> T) -> T {
    if !is_on() {
        return copy();
    }
    apply_all(WDA_EXCLUDEFROMCAPTURE);
    sleep(Duration::from_millis(50)); // give the screen one frame to update
    let result = copy();
    apply_all(WDA_NONE);
    result
}

fn affinity() -> WINDOW_DISPLAY_AFFINITY {
    if is_on() { WDA_NONE } else { WDA_EXCLUDEFROMCAPTURE }
}

fn apply_all(a: WINDOW_DISPLAY_AFFINITY) {
    WINDOWS.with(|w| {
        for &h in w.borrow().iter() {
            unsafe {
                let _ = SetWindowDisplayAffinity(h, a);
            }
        }
    });
}
