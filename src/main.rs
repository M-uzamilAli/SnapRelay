//! SnapRelay prototype: the Application Controller.
//! Registers the hotkeys, waits for them, and hands each one to the right part.

mod buffer;
mod capture;
mod clipboard;
mod feedback;
mod notice;
mod release;
mod select;

use std::time::Instant;

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::Accessibility::{CUIAutomation, IUIAutomation};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN,
};
use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, GetMessageW, TranslateMessage, MSG, WM_HOTKEY};

const HK_FULL: i32 = 1;
const HK_AREA: i32 = 2;
const HK_RELEASE: i32 = 3;
const VK_A: u32 = 0x41;
const VK_V: u32 = 0x56;
const VK_X: u32 = 0x58;
const VK_Z: u32 = 0x5A;

/// Try the preferred hotkey; if Windows or another app already owns it, try the next.
/// Returns the label of the one that worked.
fn register(id: i32, what: &str, tries: &[(HOT_KEY_MODIFIERS, u32, &'static str)]) -> Option<&'static str> {
    for (i, (mods, vk, label)) in tries.iter().enumerate() {
        if unsafe { RegisterHotKey(HWND::default(), id, *mods | MOD_NOREPEAT, *vk) }.is_ok() {
            let note = if i > 0 { "   (fallback)" } else { "" };
            println!("  {label:<18} {what}{note}");
            return Some(label);
        }
        println!("  {label:<18} taken by Windows or another app");
    }
    println!("  (no free shortcut for '{what}', this action is unavailable)");
    None
}

fn main() {
    unsafe {
        // Without this, screenshots on a scaled display (125%, 150%) come out the wrong size.
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().expect("COM init");
    }
    let uia: IUIAutomation = unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }
        .expect("Windows accessibility layer (UI Automation) is unavailable");
    let owner = clipboard::create_owner_window();

    println!("SnapRelay prototype. Shortcuts:");
    let area_key = register(HK_AREA, "capture an area you drag", &[
        (MOD_WIN | MOD_SHIFT, VK_A, "Win+Shift+A"),
        (MOD_CONTROL | MOD_ALT | MOD_SHIFT, VK_A, "Ctrl+Alt+Shift+A"),
    ]);
    let full_key = register(HK_FULL, "capture the whole screen under the mouse", &[
        (MOD_WIN | MOD_SHIFT, VK_Z, "Win+Shift+Z"),
        (MOD_CONTROL | MOD_ALT | MOD_SHIFT, VK_Z, "Ctrl+Alt+Shift+Z"),
    ]);
    // No Alt here on purpose: a released Alt makes many apps jump to their menu bar,
    // which steals focus from the text box we're about to paste into.
    let release_key = register(HK_RELEASE, "paste the queue into the focused text box", &[
        (MOD_WIN | MOD_SHIFT, VK_V, "Win+Shift+V"),
        (MOD_WIN | MOD_SHIFT, VK_X, "Win+Shift+X"),
    ])
    .unwrap_or("(none)");
    println!("Close this window (or Ctrl+C) to quit. Screenshots are kept in memory only.\n");
    notice::show(
        &format!(
            "SnapRelay ready\n{} area  \u{00B7}  {} full screen  \u{00B7}  {} paste",
            area_key.unwrap_or("-"), full_key.unwrap_or("-"), release_key
        ),
        4000,
    );

    let mut buf = buffer::Buffer::default();
    let mut msg = MSG::default();
    while unsafe { GetMessageW(&mut msg, HWND::default(), 0, 0) }.0 > 0 {
        if msg.message != WM_HOTKEY {
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            continue;
        }
        match msg.wParam.0 as i32 {
            HK_FULL => capture(&mut buf, false),
            HK_AREA => capture(&mut buf, true),
            HK_RELEASE => release::release(&mut buf, &uia, owner, release_key),
            _ => {}
        }
    }
}

/// Capture Engine hand-off: copy the screen, optionally let the user pick an area,
/// give feedback, compress, and store in the Context Buffer.
fn capture(buf: &mut buffer::Buffer, pick_area: bool) {
    let t = Instant::now();
    let raw = match capture::grab_monitor_under_cursor() {
        Ok(raw) => raw,
        Err(e) => {
            println!("Capture failed: {e}");
            notice::show("Capture failed", 3000);
            return;
        }
    };
    let raw = if pick_area {
        let (w, h, px) = raw.pixels();
        match select::select_area(raw.area, w, h, px) {
            Some(r) => capture::crop(raw, r),
            None => {
                println!("Area capture cancelled.");
                notice::show("Area capture cancelled", 1500);
                return;
            }
        }
    } else {
        raw
    };
    // Feedback before the slow compression, so it feels instant
    // (and the flash never appears in the screenshot).
    feedback::click();
    feedback::flash(raw.area);
    match capture::compress(raw) {
        Ok(frame) => {
            let (w, h, kb) = (frame.width, frame.height, frame.png.len() / 1024);
            let id = buf.add(frame);
            let (pending, _, bytes) = buf.summary();
            println!(
                "Captured #{id}: {w}x{h}, {kb} KB in {} ms. Queue: {pending} pending, {} KB in memory.",
                t.elapsed().as_millis(), bytes / 1024
            );
            notice::show(&format!("Captured \u{00B7} {pending} in queue"), 1800);
        }
        Err(e) => {
            println!("Capture failed: {e}");
            notice::show("Capture failed", 3000);
        }
    }
}
