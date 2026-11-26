//! Release Controller: finds the focused text box (generic, any app),
//! then pastes each pending screenshot via a clipboard promise + Ctrl+V,
//! moving on as soon as the app pulls the image.

use std::mem::size_of;
use std::thread::sleep;
use std::time::{Duration, Instant};

use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, HWND};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Accessibility::{
    IUIAutomation, IUIAutomationElement, IUIAutomationValuePattern, UIA_CONTROLTYPE_ID,
    UIA_DocumentControlTypeId, UIA_EditControlTypeId, UIA_ValuePatternId,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_KEYUP, VIRTUAL_KEY, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};

use crate::buffer::{Buffer, Status};
use crate::clipboard;

const VK_V: VIRTUAL_KEY = VIRTUAL_KEY(0x56);
/// If the app hasn't pulled the image by then, the paste didn't land.
const READ_TIMEOUT: Duration = Duration::from_secs(3);

/// What Windows told us about the focused element.
pub struct Target {
    pub kind: &'static str,
    pub name: String,
    pub framework: String,
    pub pid: i32,
    pub app: String,
}

/// Ask Windows: what has focus, and is it a box we can paste into?
pub fn focused_text_box(uia: &IUIAutomation) -> Result<Target, String> {
    unsafe {
        let el = uia
            .GetFocusedElement()
            .map_err(|e| format!("Windows couldn't say what has focus ({e})"))?;
        let ct = el.CurrentControlType().unwrap_or_default();
        let target = Target {
            kind: kind_name(ct),
            name: el.CurrentName().map(|b| b.to_string()).unwrap_or_default(),
            framework: el.CurrentFrameworkId().map(|b| b.to_string()).unwrap_or_default(),
            pid: el.CurrentProcessId().unwrap_or(0),
            app: String::new(),
        };
        let target = Target { app: app_name(target.pid as u32), ..target };

        if target.pid as u32 == std::process::id() {
            return Err("focus is on SnapRelay itself".into());
        }
        // Edit = a text box. Document = a larger text area (e.g. Notepad), but web
        // pages are Documents too, so only accept one that says it can be edited.
        let ok = if ct == UIA_EditControlTypeId {
            is_editable(&el).unwrap_or(true)
        } else if ct == UIA_DocumentControlTypeId {
            is_editable(&el).unwrap_or(false)
        } else {
            false
        };
        if ok {
            Ok(target)
        } else {
            Err(format!(
                "focus is on a {} '{}' in {}, which isn't a text box",
                target.kind, target.name, target.app
            ))
        }
    }
}

unsafe fn is_editable(el: &IUIAutomationElement) -> Option<bool> {
    let p: IUIAutomationValuePattern = el.GetCurrentPatternAs(UIA_ValuePatternId).ok()?;
    Some(!p.CurrentIsReadOnly().ok()?.as_bool())
}

fn kind_name(ct: UIA_CONTROLTYPE_ID) -> &'static str {
    if ct == UIA_EditControlTypeId {
        "Edit"
    } else if ct == UIA_DocumentControlTypeId {
        "Document"
    } else {
        "non-text element"
    }
}

fn app_name(pid: u32) -> String {
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return format!("process {pid}");
        };
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let res = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(h);
        match res {
            Ok(()) => String::from_utf16_lossy(&buf[..len as usize])
                .rsplit('\\')
                .next()
                .unwrap_or_default()
                .to_string(),
            Err(_) => format!("process {pid}"),
        }
    }
}

/// The app never sees the hotkey's letter (we swallow it), so when the user lets go
/// it sees a lone Alt press, which most apps (Notepad, Chrome) treat as "open the
/// menu bar", stealing focus from the text box. Tapping an unassigned key while
/// Alt is still held makes it Alt+something instead.
fn mask_lone_alt() {
    let unassigned = VIRTUAL_KEY(0xE8);
    let tap = |up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: unassigned,
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    unsafe {
        SendInput(&[tap(false), tap(true)], size_of::<INPUT>() as i32);
    }
}

/// The release hotkey is still physically held when we get here. If we send
/// Ctrl+V now, the app sees Ctrl+Alt+Shift+V. Wait until the keys are let go.
fn wait_for_modifiers_released() {
    let start = Instant::now();
    let held = || unsafe {
        [VK_CONTROL, VK_SHIFT, VK_MENU, VK_LWIN, VK_RWIN]
            .iter()
            .any(|k| GetAsyncKeyState(k.0 as i32) < 0)
    };
    while held() && start.elapsed() < Duration::from_secs(3) {
        sleep(Duration::from_millis(20));
    }
}

pub fn release(buf: &mut Buffer, uia: &IUIAutomation, owner: HWND) {
    mask_lone_alt(); // must happen first, while the user is still holding the keys
    let pending = buf.pending_ids();
    if pending.is_empty() {
        println!("Nothing to release: the queue has no pending screenshots.");
        return;
    }
    wait_for_modifiers_released();

    let started = Instant::now();
    let target = match focused_text_box(uia) {
        Ok(t) => t,
        Err(why) => {
            println!("Not pasting: {why}.");
            println!("  Click into the chat box, then press release again. {} screenshot(s) still queued.", pending.len());
            return;
        }
    };
    println!(
        "Pasting {} screenshot(s) into: {} '{}' in {} (drawn by {})",
        pending.len(), target.kind, target.name, target.app, target.framework
    );

    for id in pending {
        // Re-check before every paste: if focus moved, stop rather than paste somewhere else.
        match focused_text_box(uia) {
            Ok(t) if t.pid == target.pid => {}
            Ok(t) => {
                println!("  Stopped: focus moved to {}. Remaining screenshots stay queued.", t.app);
                return;
            }
            Err(why) => {
                println!("  Stopped: {why}. Remaining screenshots stay queued.");
                return;
            }
        }
        let shot = buf.get(id).expect("pending id exists");
        if let Err(e) = clipboard::promise(owner, &shot.frame.png) {
            println!("  #{id}: clipboard failed ({e}). Kept in queue.");
            return;
        }
        let sent = Instant::now();
        send_ctrl_v();
        // The signal: the target app pulls the image from us. Any of its processes count
        // (Chrome-based apps read the clipboard from a different process than the one we see).
        let is_target = |pid: u32| app_name(pid).eq_ignore_ascii_case(&target.app);
        match clipboard::wait_for_read(owner, is_target, READ_TIMEOUT) {
            Ok(read) => {
                buf.mark(id, Status::Delivered);
                println!(
                    "  #{id}: taken by {} as {} after {} ms",
                    target.app, read.format, read.at.saturating_duration_since(sent).as_millis()
                );
            }
            Err(others) => {
                println!(
                    "  #{id}: {} didn't take the image within {} s. Kept in queue; stopping here.",
                    target.app, READ_TIMEOUT.as_secs()
                );
                for o in others {
                    println!("     (it was read by {} instead)", app_name(o.pid));
                }
                return;
            }
        }
    }
    println!("Release finished in {} ms.", started.elapsed().as_millis());
}

fn send_ctrl_v() {
    let key = |vk: VIRTUAL_KEY, up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let inputs = [key(VK_CONTROL, false), key(VK_V, false), key(VK_V, true), key(VK_CONTROL, true)];
    unsafe {
        SendInput(&inputs, size_of::<INPUT>() as i32);
    }
}
