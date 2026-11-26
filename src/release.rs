//! Release Controller: finds the focused text box (generic, any app),
//! then pastes each pending screenshot via a clipboard promise + Ctrl+V,
//! moving on as soon as the app pulls the image.

use std::collections::HashMap;
use std::mem::size_of;
use std::thread::sleep;
use std::time::{Duration, Instant};

use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, HWND};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Accessibility::{
    IUIAutomation, IUIAutomationElement, IUIAutomationValuePattern,
    UIA_CONTROLTYPE_ID, UIA_CustomControlTypeId, UIA_DocumentControlTypeId,
    UIA_EditControlTypeId, UIA_GroupControlTypeId, UIA_PaneControlTypeId,
    UIA_ValuePatternId,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_KEYUP, VIRTUAL_KEY, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};

use crate::buffer::Buffer;
use crate::clipboard;
use crate::notice;

const VK_V: VIRTUAL_KEY = VIRTUAL_KEY(0x56);
/// How long one Ctrl+V gets before we consider it missed and press again.
const READ_WAIT: Duration = Duration::from_millis(1200);
const PASTE_TRIES: u32 = 3;
/// Quiet time after a new clipboard item, so sync tools grab it before the app pastes.
const SETTLE: Duration = Duration::from_millis(60);

/// What Windows told us about the focused element.
pub struct Target {
    pub kind: &'static str,
    pub name: String,
    pub framework: String,
    pub pid: i32,
    pub app: String,
    /// false = Windows can't see inside this app (e.g. WhatsApp's embedded browser),
    /// so we try the paste and let the read signal decide.
    pub sure: bool,
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
            sure: true,
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
        } else if is_opaque(ct, &target.name) {
            // Windows can't tell what this is. It might well be a text box (WhatsApp's
            // message box shows up like this): try, and the read signal tells us if it worked.
            Ok(Target { kind: "unknown area", sure: false, ..target })
        } else {
            let class = el.CurrentClassName().map(|b| b.to_string()).unwrap_or_default();
            Err(format!(
                "focus is on a {} '{}' in {} (type id {}, class '{}'), which isn't a text box",
                target.kind, target.name, target.app, ct.0, class
            ))
        }
    }
}

/// An unnamed pane/group/custom element: an app drawing its own content without
/// telling Windows what's there (e.g. WhatsApp's embedded browser). Named elements
/// and clearly non-text controls (buttons, lists, menus) are still refused.
fn is_opaque(ct: UIA_CONTROLTYPE_ID, name: &str) -> bool {
    let container = ct == UIA_PaneControlTypeId || ct == UIA_GroupControlTypeId || ct == UIA_CustomControlTypeId;
    container && name.trim().is_empty()
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

/// Programs that read every clipboard change on their own (sync tools). Their reads
/// never mean "the paste happened", so they don't block a retry.
const BACKGROUND_READERS: [&str; 2] = ["msrdc.exe", "PowerToys.MouseWithoutBordersHelper.exe"];

pub fn release(buf: &mut Buffer, uia: &IUIAutomation, owner: HWND, release_key: &str) {
    mask_lone_alt(); // must happen first, while the user is still holding the keys
    let pending = buf.pending_ids();
    if pending.is_empty() {
        println!("Nothing to release: the queue has no pending screenshots.");
        notice::show("Nothing queued", 2000);
        return;
    }
    wait_for_modifiers_released();

    let started = Instant::now();
    let total = pending.len();
    let target = match focused_text_box(uia) {
        Ok(t) => t,
        Err(why) => {
            println!("Not pasting: {why}.");
            println!("  Click into the chat box, then press release again. {total} screenshot(s) still queued.");
            notice::show("Click into the chat box first", 3000);
            return;
        }
    };
    println!(
        "Pasting {total} screenshot(s) into: {} '{}' in {} (drawn by {})",
        target.kind, target.name, target.app, target.framework
    );
    // The app counts if it reads under its own name, or through a helper process it
    // started (Chrome-based apps and embedded browsers like WhatsApp's read the
    // clipboard from a different process than the one that owns the window).
    let parents = parent_map();
    let is_target = |pid: u32| {
        app_name(pid).eq_ignore_ascii_case(&target.app) || descends_from(pid, target.pid as u32, &parents)
    };
    if !target.sure {
        println!("  Windows can't see inside {}; trying the paste and checking whether it takes the image.", target.app);
    }

    let mut delivered = 0;
    for (n, id) in pending.into_iter().enumerate() {
        // Re-check before every paste: if focus moved, stop rather than paste somewhere else.
        let still_there = match focused_text_box(uia) {
            Ok(t) if t.pid == target.pid => Ok(()),
            Ok(t) => Err(format!("focus moved to {}", t.app)),
            Err(why) => Err(why),
        };
        if let Err(why) = still_there {
            println!("  Stopped: {why}. Remaining screenshots stay queued.");
            break;
        }
        notice::show(&format!("Sending {} of {total}\u{2026}", n + 1), 5000);
        let shot = buf.get(id).expect("pending id exists");
        if let Err(e) = clipboard::promise(owner, &shot.frame.png) {
            println!("  #{id}: clipboard failed ({e}). Kept in queue.");
            break;
        }

        let mut taken = None;
        for attempt in 1..=PASTE_TRIES {
            // Sync tools grab every new clipboard item; if one still holds the clipboard
            // open when the app tries to paste, the app quietly gets nothing.
            clipboard::settle(owner, SETTLE);
            if let Some((pid, waited)) = clipboard::wait_until_unlocked(owner, Duration::from_secs(1)) {
                println!("     clipboard was held by {} for {} ms before the paste", app_name(pid), waited.as_millis());
            }
            let sent = Instant::now();
            send_ctrl_v();
            match clipboard::wait_for_read(owner, &is_target, sent, READ_WAIT) {
                Ok(read) => {
                    taken = Some((read, sent, attempt));
                    break;
                }
                Err(reads) => {
                    // Retrying is safe only if nothing but background sync tools read it
                    // after our Ctrl+V; otherwise we can't be sure it wasn't pasted.
                    let unknown: Vec<String> = reads
                        .iter()
                        .filter(|r| r.at >= sent)
                        .map(|r| app_name(r.pid))
                        .filter(|name| !BACKGROUND_READERS.iter().any(|b| b.eq_ignore_ascii_case(name)))
                        .collect();
                    if !unknown.is_empty() {
                        println!("     read by {} instead; not retrying in case it was pasted there", unknown.join(", "));
                        break;
                    }
                    if attempt < PASTE_TRIES {
                        println!("     try {attempt}: {} hasn't taken it yet, pressing Ctrl+V again", target.app);
                    }
                }
            }
        }
        let readers = readers_list();
        match taken {
            Some((read, sent, attempt)) => {
                buf.remove(id); // delivered: free its memory right away
                delivered += 1;
                let tries = if attempt > 1 { format!(" on try {attempt}") } else { String::new() };
                let who = if read.pid == 0 { "an unnamed reader".to_string() } else { app_name(read.pid) };
                println!(
                    "  #{id}: taken by {who} as {} {} ms after Ctrl+V{tries}   (all readers: {readers})",
                    read.format, read.at.saturating_duration_since(sent).as_millis()
                );
            }
            None => {
                println!("  #{id}: not taken by {}. Kept in queue; stopping here.   (readers: {readers})", target.app);
                break;
            }
        }
    }

    let kept = total - delivered;
    let (left, bytes) = buf.summary();
    println!(
        "Release finished in {} ms: {delivered} of {total} delivered. Queue: {left} left, {} KB in memory.",
        started.elapsed().as_millis(), bytes / 1024
    );
    if kept == 0 {
        notice::show(&format!("\u{2713} Sent {delivered}"), 2500);
    } else {
        notice::show(
            &format!("Sent {delivered} of {total}. {kept} still queued \u{00B7} {release_key} to retry"),
            6000,
        );
    }
}

/// child pid -> parent pid, for every running process.
fn parent_map() -> HashMap<u32, u32> {
    let mut map = HashMap::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return map };
        let mut e = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        if Process32FirstW(snap, &mut e).is_ok() {
            loop {
                map.insert(e.th32ProcessID, e.th32ParentProcessID);
                if Process32NextW(snap, &mut e).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    map
}

/// Was `pid` started (directly or a few levels down) by `ancestor`?
fn descends_from(pid: u32, ancestor: u32, parents: &HashMap<u32, u32>) -> bool {
    let mut p = pid;
    for _ in 0..8 {
        match parents.get(&p) {
            Some(&parent) if parent == ancestor => return true,
            Some(&parent) if parent != 0 && parent != p => p = parent,
            _ => return false,
        }
    }
    false
}

/// Everyone who pulled the current image, for the log ("chrome.exe, msrdc.exe").
fn readers_list() -> String {
    let mut names: Vec<String> = clipboard::reads()
        .iter()
        .map(|r| if r.pid == 0 { "unnamed reader".to_string() } else { app_name(r.pid) })
        .collect();
    names.sort();
    names.dedup();
    if names.is_empty() { "none".into() } else { names.join(", ") }
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
