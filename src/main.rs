//! SnapRelay prototype: the Application Controller.
//! Registers the hotkeys, waits for them, and hands each one to the right part.

mod buffer;
mod capture;
mod feedback;
mod release;

use std::time::Instant;

use windows::core::w;
use windows::Win32::Foundation::{HINSTANCE, HWND};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::Accessibility::{CUIAutomation, IUIAutomation};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, GetMessageW, HMENU, HWND_MESSAGE, MSG, WINDOW_EX_STYLE, WINDOW_STYLE, WM_HOTKEY,
};

const HK_CAPTURE: i32 = 1;
const HK_RELEASE: i32 = 2;
const VK_Z: u32 = 0x5A;
const VK_V: u32 = 0x56;
const VK_X: u32 = 0x58;

fn register(id: i32, mods: HOT_KEY_MODIFIERS, vk: u32) -> bool {
    unsafe { RegisterHotKey(HWND::default(), id, mods | MOD_NOREPEAT, vk).is_ok() }
}

/// Try the preferred hotkey; if Windows or another app already owns it, try the fallback.
fn register_with_fallback(id: i32, what: &str, tries: &[(HOT_KEY_MODIFIERS, u32, &str)]) {
    for (mods, vk, label) in tries {
        if register(id, *mods, *vk) {
            println!("  {label:<18} {what}");
            return;
        }
        println!("  {label:<18} taken by Windows or another app, trying the next option");
    }
    println!("  (no free shortcut for '{what}', this action is unavailable)");
}

fn main() {
    unsafe {
        // Without this, screenshots on a scaled display (125%, 150%) come out the wrong size.
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().expect("COM init");
    }
    let uia: IUIAutomation = unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }
        .expect("Windows accessibility layer (UI Automation) is unavailable");
    // An invisible window that owns our clipboard data.
    let owner = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0), w!("STATIC"), w!("SnapRelay"), WINDOW_STYLE(0),
            0, 0, 0, 0, HWND_MESSAGE, HMENU::default(), HINSTANCE::default(), None,
        )
    }
    .expect("couldn't create the clipboard owner window");

    println!("SnapRelay prototype. Shortcuts:");
    register_with_fallback(HK_CAPTURE, "capture the screen under the mouse", &[
        (MOD_WIN | MOD_SHIFT, VK_Z, "Win+Shift+Z"),
        (MOD_CONTROL | MOD_ALT | MOD_SHIFT, VK_Z, "Ctrl+Alt+Shift+Z"),
    ]);
    // No Alt here on purpose: a released Alt makes many apps jump to their menu bar,
    // which steals focus from the text box we're about to paste into.
    register_with_fallback(HK_RELEASE, "paste the queue into the focused text box", &[
        (MOD_WIN | MOD_SHIFT, VK_X, "Win+Shift+X"),
        (MOD_CONTROL | MOD_ALT | MOD_SHIFT, VK_V, "Ctrl+Alt+Shift+V"),
    ]);
    println!("Close this window (or Ctrl+C) to quit. Screenshots are kept in memory only.\n");

    let mut buf = buffer::Buffer::default();
    let mut msg = MSG::default();
    while unsafe { GetMessageW(&mut msg, HWND::default(), 0, 0) }.0 > 0 {
        if msg.message != WM_HOTKEY {
            continue;
        }
        match msg.wParam.0 as i32 {
            HK_CAPTURE => {
                let t = Instant::now();
                let result = capture::grab_monitor_under_cursor().and_then(|raw| {
                    // Feedback right after the screen copy, before the slow compression,
                    // so it feels instant and the flash never appears in the screenshot.
                    feedback::click();
                    feedback::flash(raw.area);
                    capture::compress(raw)
                });
                match result {
                    Ok(frame) => {
                        let (w, h, kb) = (frame.width, frame.height, frame.png.len() / 1024);
                        let id = buf.add(frame);
                        let (pending, _, bytes) = buf.summary();
                        println!(
                            "Captured #{id}: {w}x{h}, {kb} KB in {} ms. Queue: {pending} pending, {} KB in memory.",
                            t.elapsed().as_millis(), bytes / 1024
                        );
                    }
                    Err(e) => println!("Capture failed: {e}"),
                }
            }
            HK_RELEASE => release::release(&mut buf, &uia, owner),
            _ => {}
        }
    }
}
