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
mod hotkeys;
mod keys;
mod notice;
mod queue;
mod recording;
mod release;
mod select;
mod settings;
mod settings_ui;
mod smooth;
mod theme;
mod tray;

use std::time::Instant;

use settings::Shortcut;

use windows::core::w;
use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS, HWND};
use windows::Win32::System::Memory::{GetProcessHeap, HeapCompact, HEAP_FLAGS};
use windows::Win32::System::Threading::{CreateMutexW, GetCurrentProcess, SetProcessWorkingSetSize};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::Accessibility::{CUIAutomation, IUIAutomation};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT};
use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, GetMessageW, TranslateMessage, MSG, WM_HOTKEY};

/// The demo counter's shortcut (recording mode only). The four main shortcuts are
/// managed by hotkeys.rs, ids 1-4.
const HK_COUNTER: i32 = 5;

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

    settings::load();
    hotkeys::register_all();
    // Demo helper (recording mode only): start/stop the on-screen counter.
    for vk in [0x54u32, 0x30] {
        // Ctrl+Alt+Shift+T, else Ctrl+Alt+Shift+0
        if unsafe { RegisterHotKey(HWND::default(), HK_COUNTER, MOD_CONTROL | MOD_ALT | MOD_SHIFT | MOD_NOREPEAT, vk) }.is_ok() {
            break;
        }
    }
    println!("Close this window (or Ctrl+C) to quit. Screenshots are kept in memory only.\n");
    // No start-up notice: the tray icon is the only sign SnapRelay is running.

    let mut buf = buffer::Buffer::default();
    let mut msg = MSG::default();
    while unsafe { GetMessageW(&mut msg, HWND::default(), 0, 0) }.0 > 0 {
        // Set when a screenshot came in or went out, so its working memory can go back.
        let mut trim = false;
        if msg.message == WM_HOTKEY {
            trim = true;
            match hotkeys::from_id(msg.wParam.0 as i32) {
                Some(Shortcut::Screen) => capture(&mut buf, false),
                Some(Shortcut::Area) => capture(&mut buf, true),
                Some(Shortcut::Paste) => {
                    let key = hotkeys::label(Shortcut::Paste);
                    keys::show(&key, "Paste into chat");
                    release::release(&mut buf, &uia, owner, &key)
                }
                Some(Shortcut::Queue) => {
                    keys::show(&hotkeys::label(Shortcut::Queue), "Queue");
                    queue::toggle(&buf)
                }
                None if msg.wParam.0 as i32 == HK_COUNTER => {
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
                    release::release(&mut buf, &uia, owner, &hotkeys::label(Shortcut::Paste));
                    if buf.summary().0 == 0 {
                        queue::hide();
                    }
                }
            }
        }
        if let Some(action) = tray::take_action() {
            match action {
                tray::Action::ToggleQueue => queue::toggle(&buf),
                tray::Action::OpenSettings => settings_ui::open(),
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
fn capture(buf: &mut buffer::Buffer, pick_area: bool) {
    let t = Instant::now();
    let which = if pick_area { Shortcut::Area } else { Shortcut::Screen };
    let shortcut = hotkeys::label(which);
    let prefs = settings::get();
    // Queue memory limit (Settings): stop here rather than drop anything already queued.
    if buf.summary().1 >= prefs.limit_mb as usize * 1024 * 1024 {
        keys::show(&shortcut, "Queue full");
        notice::show(&format!("Queue full ({} MB) \u{00B7} send or clear first", prefs.limit_mb), 3000);
        return;
    }
    // SnapRelay's own windows never go into the screenshot, even in recording mode.
    let grabbed = recording::hidden(capture::grab_monitor_under_cursor);
    // Recording mode: show the shortcut now, after the copy, so it isn't in the picture.
    keys::show(&shortcut, if pick_area { "Capture area" } else { "Capture screen" });
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
        match select::select_area(raw.area, w, h, px, hotkeys::id(Shortcut::Area), &shortcut) {
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
    // (Both can be turned off in Settings.)
    if prefs.sound {
        feedback::click();
    }
    if prefs.flash {
        feedback::flash(raw.area);
    }
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
