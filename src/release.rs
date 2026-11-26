//! Release Controller: finds the focused text box (generic, any app),
//! then pastes each pending screenshot via clipboard + Ctrl+V.

use std::mem::size_of;
use std::thread::sleep;
use std::time::{Duration, Instant};

use windows::core::{w, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
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
use windows::Win32::Graphics::Gdi::{BITMAPINFOHEADER, BI_RGB};

use crate::buffer::{Buffer, Status};
use crate::capture::decode_to_bgra_bottom_up;

const CF_DIB: u32 = 8;
const VK_V: VIRTUAL_KEY = VIRTUAL_KEY(0x56);
/// Time we give the app to read the clipboard before we replace it with the next image.
const GAP_BETWEEN_PASTES: Duration = Duration::from_millis(700);

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
        if let Err(e) = put_image_on_clipboard(owner, &shot.frame.png) {
            println!("  #{id}: clipboard failed ({e}). Kept in queue.");
            return;
        }
        send_ctrl_v();
        sleep(GAP_BETWEEN_PASTES);
        buf.mark(id, Status::Pasted);
        println!("  #{id}: pasted (Ctrl+V sent; not yet verified that it attached)");
    }
    println!("Release finished.");
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

/// Puts the image on the clipboard in two forms (PNG for apps that prefer it,
/// DIB for everything else) and marks it to stay out of Win+V clipboard history.
fn put_image_on_clipboard(owner: HWND, png_bytes: &[u8]) -> Result<(), String> {
    let (w, h, bgra) = decode_to_bgra_bottom_up(png_bytes)?;
    let header = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: w as i32,
        biHeight: h as i32, // positive = rows bottom-up
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,
        biSizeImage: bgra.len() as u32,
        ..Default::default()
    };
    let header_bytes = unsafe {
        std::slice::from_raw_parts((&header as *const BITMAPINFOHEADER).cast::<u8>(), size_of::<BITMAPINFOHEADER>())
    };
    let mut dib = Vec::with_capacity(header_bytes.len() + bgra.len());
    dib.extend_from_slice(header_bytes);
    dib.extend_from_slice(&bgra);

    unsafe {
        let png_fmt = RegisterClipboardFormatW(w!("PNG"));
        let no_history = RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing"));

        let mut opened = false;
        for _ in 0..10 {
            if OpenClipboard(owner).is_ok() {
                opened = true;
                break;
            }
            sleep(Duration::from_millis(30)); // another app may be holding it
        }
        if !opened {
            return Err("clipboard is busy".into());
        }
        let result = (|| {
            EmptyClipboard().map_err(|e| e.to_string())?;
            set_data(png_fmt, png_bytes)?;
            set_data(CF_DIB, &dib)?;
            set_data(no_history, &[0, 0, 0, 0])?;
            Ok(())
        })();
        let _ = CloseClipboard();
        result
    }
}

unsafe fn set_data(format: u32, bytes: &[u8]) -> Result<(), String> {
    let mem = GlobalAlloc(GMEM_MOVEABLE, bytes.len()).map_err(|e| e.to_string())?;
    let dst = GlobalLock(mem);
    if dst.is_null() {
        return Err("couldn't lock clipboard memory".into());
    }
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), dst.cast::<u8>(), bytes.len());
    let _ = GlobalUnlock(mem); // reports an "error" when the lock count reaches zero; that's normal
    SetClipboardData(format, HANDLE(mem.0)).map_err(|e| e.to_string())?;
    Ok(())
}
