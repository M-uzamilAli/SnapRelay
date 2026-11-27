// No console window in the release build (it lives in the tray); `cargo run`
// keeps the console for the developer log.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! SnapRelay prototype: the Application Controller.
//! Registers the hotkeys, waits for them, and hands each one to the right part.

mod buffer;
mod capture;
mod clipboard;
mod counter;
mod feedback;
mod fonts;
mod keys;
mod notice;
mod queue;
mod recording;
mod release;
mod select;
mod smooth;
mod theme;
mod tray;

use std::time::Instant;

use windows::core::w;
use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS, HWND};
use windows::Win32::System::Memory::{GetProcessHeap, HeapCompact, HEAP_FLAGS};
use windows::Win32::System::Threading::{CreateMutexW, GetCurrentProcess, SetProcessWorkingSetSize};
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
const HK_QUEUE: i32 = 4;
const HK_COUNTER: i32 = 5;
const VK_0: u32 = 0x30;
const VK_D: u32 = 0x44;
const VK_Q: u32 = 0x51;
const VK_T: u32 = 0x54;
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
    // Only one SnapRelay at a time: a second copy would fight over the shortcuts.
    let _single = unsafe { CreateMutexW(None, true, w!("Local\\SnapRelay.SingleInstance")) };
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        println!("SnapRelay is already running.");
        return;
    }
    unsafe {
        // Without this, screenshots on a scaled display (125%, 150%) come out the wrong size.
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().expect("COM init");
    }
    let uia: IUIAutomation = unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }
        .expect("Windows accessibility layer (UI Automation) is unavailable");
    recording::init();
    if !fonts::load() {
        println!("Couldn't load the built-in Inter font; Windows will substitute one.");
    }
    let owner = clipboard::create_owner_window();
    counter::set_clipboard_owner(owner);
    tray::create();

    println!("SnapRelay prototype. Shortcuts:");
    // Z, X, D sit together under the left hand: paste, area, whole screen.
    let full_key = register(HK_FULL, "capture the whole screen under the mouse", &[
        (MOD_WIN | MOD_SHIFT, VK_D, "Win+Shift+D"),
        (MOD_CONTROL | MOD_ALT | MOD_SHIFT, VK_D, "Ctrl+Alt+Shift+D"),
    ]);
    let area_key = register(HK_AREA, "capture an area you drag", &[
        (MOD_WIN | MOD_SHIFT, VK_X, "Win+Shift+X"),
        (MOD_CONTROL | MOD_ALT | MOD_SHIFT, VK_X, "Ctrl+Alt+Shift+X"),
    ]);
    // No Alt here on purpose: a released Alt makes many apps jump to their menu bar,
    // which steals focus from the text box we're about to paste into.
    let release_key = register(HK_RELEASE, "paste the ticked screenshots into the focused text box", &[
        (MOD_WIN | MOD_SHIFT, VK_Z, "Win+Shift+Z"),
        (MOD_WIN | MOD_SHIFT, VK_V, "Win+Shift+V"),
    ])
    .unwrap_or("(none)");
    let queue_key = register(HK_QUEUE, "open or close the queue window", &[
        (MOD_WIN | MOD_SHIFT, VK_Q, "Win+Shift+Q"),
        (MOD_CONTROL | MOD_ALT | MOD_SHIFT, VK_Q, "Ctrl+Alt+Shift+Q"),
    ]);
    // Demo helper (recording mode only): start/stop the on-screen counter.
    register(HK_COUNTER, "start / stop the demo counter (recording mode)", &[
        (MOD_CONTROL | MOD_ALT | MOD_SHIFT, VK_T, "Ctrl+Alt+Shift+T"),
        (MOD_CONTROL | MOD_ALT | MOD_SHIFT, VK_0, "Ctrl+Alt+Shift+0"),
    ]);
    queue::set_hint(format!(
        "{} screen  \u{00B7}  {} area",
        full_key.unwrap_or("-"), area_key.unwrap_or("-")
    ));
    println!("Close this window (or Ctrl+C) to quit. Screenshots are kept in memory only.\n");
    // No start-up notice: the tray icon is the only sign SnapRelay is running.

    let mut buf = buffer::Buffer::default();
    let mut msg = MSG::default();
    while unsafe { GetMessageW(&mut msg, HWND::default(), 0, 0) }.0 > 0 {
        // Set when a screenshot came in or went out, so its working memory can go back.
        let mut trim = false;
        if msg.message == WM_HOTKEY {
            trim = true;
            match msg.wParam.0 as i32 {
                HK_FULL => capture(&mut buf, false, full_key.unwrap_or("")),
                HK_AREA => capture(&mut buf, true, area_key.unwrap_or("")),
                HK_RELEASE => {
                    keys::show(release_key, "Paste into chat");
                    release::release(&mut buf, &uia, owner, release_key)
                }
                HK_QUEUE => {
                    keys::show(queue_key.unwrap_or(""), "Queue");
                    queue::toggle(&buf)
                }
                HK_COUNTER => {
                    if recording::is_on() {
                        counter::toggle();
                    } else {
                        notice::show("Turn on Recording mode to use the counter", 2500);
                    }
                }
                _ => {}
            }
        } else {
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        // A click in the queue window is carried out here, where the queue lives.
        if let Some(action) = queue::take_action() {
            match action {
                queue::Action::Toggle(id) => {
                    buf.toggle(id);
                    println!("Ticked: {:?}", buf.selected_ids());
                }
                queue::Action::ToggleAll => {
                    buf.toggle_all();
                    println!("Ticked: {:?}", buf.selected_ids());
                }
                queue::Action::Move(id, to) => {
                    buf.move_to(id, to);
                    println!("Moved #{id} to position {}. Order: {:?}", to + 1, buf.items().iter().map(|s| s.id).collect::<Vec<_>>());
                }
                queue::Action::Remove(id) => {
                    trim = true;
                    buf.remove(id);
                    println!("Removed #{id} from the queue.");
                }
                queue::Action::ClearAll => {
                    trim = true;
                    let (n, _) = buf.summary();
                    buf.clear();
                    println!("Cleared the queue ({n} screenshot(s)).");
                    notice::show(&format!("Cleared {n}"), 1500);
                }
                queue::Action::Send => {
                    trim = true;
                    // The pop-up never took focus, so the chat box still has it.
                    release::release(&mut buf, &uia, owner, release_key);
                    if buf.summary().0 == 0 {
                        queue::hide();
                    }
                }
            }
        }
        if let Some(action) = tray::take_action() {
            match action {
                tray::Action::ToggleQueue => queue::toggle(&buf),
                tray::Action::ToggleRecording => {
                    let on = !recording::is_on();
                    recording::set(on);
                    if !on {
                        counter::hide();
                    }
                    println!("Recording mode {}.", if on { "on" } else { "off" });
                    notice::show(if on { "Recording mode on \u{00B7} shortcuts show on screen" } else { "Recording mode off" }, 2500);
                }
                tray::Action::Quit => break,
            }
        }
        queue::sync(&buf);
        if trim {
            give_back_memory();
        }
        tray::set_tooltip(&match buf.summary().0 {
            0 => "SnapRelay".to_string(),
            n => format!("SnapRelay \u{00B7} {n} queued"),
        });
    }
    tray::remove();
}

/// Hand memory we've finished with back to Windows: the big temporary screenshot
/// blocks, and pages of code we only needed for a moment. Windows gives pages back
/// instantly if we need them again; this just stops us looking heavier than we are.
fn give_back_memory() {
    unsafe {
        if let Ok(heap) = GetProcessHeap() {
            let _ = HeapCompact(heap, HEAP_FLAGS(0));
        }
        let _ = SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
    }
}

/// Capture Engine hand-off: copy the screen, optionally let the user pick an area,
/// give feedback, compress, and store in the Context Buffer.
fn capture(buf: &mut buffer::Buffer, pick_area: bool, shortcut: &str) {
    let t = Instant::now();
    // SnapRelay's own windows never go into the screenshot, even in recording mode.
    let grabbed = recording::hidden(capture::grab_monitor_under_cursor);
    // Recording mode: show the shortcut now, after the copy, so it isn't in the picture.
    keys::show(shortcut, if pick_area { "Capture area" } else { "Capture screen" });
    let raw = match grabbed {
        Ok(raw) => raw,
        Err(e) => {
            println!("Capture failed: {e}");
            notice::show("Capture failed", 3000);
            return;
        }
    };
    let raw = if pick_area {
        let (w, h, px) = raw.pixels();
        match select::select_area(raw.area, w, h, px, HK_AREA, shortcut) {
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
    let preview_box = queue::preview_size(raw.dpi());
    match capture::compress(raw, preview_box) {
        Ok(frame) => {
            let (w, h, kb) = (frame.width, frame.height, frame.png.len() / 1024);
            let id = buf.add(frame);
            counter::screenshot_taken();
            let (pending, bytes) = buf.summary();
            println!(
                "Captured #{id}: {w}x{h}, {kb} KB in {} ms. Queue: {pending} pending, {} KB in memory.",
                t.elapsed().as_millis(), bytes / 1024
            );
            notice::show(&format!("Captured \u{00B7} {pending} queued"), 1500);
        }
        Err(e) => {
            println!("Capture failed: {e}");
            notice::show("Capture failed", 3000);
        }
    }
}
