//! Clipboard side of the Release Controller.
//!
//! Instead of putting the full image on the clipboard, we put a *promise*
//! ("SnapRelay has an image"). When an app pastes, Windows asks us for the
//! image right then. That request is our signal that the paste was read,
//! so we can move on immediately instead of guessing with a fixed wait.

use std::cell::RefCell;
use std::mem::size_of;
use std::sync::{Once, OnceLock};
use std::thread::sleep;
use std::time::{Duration, Instant};

use windows::core::w;
use windows::Win32::Foundation::{HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{BITMAPINFOHEADER, BI_RGB};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetOpenClipboardWindow, OpenClipboard,
    RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetWindowThreadProcessId, PeekMessageW,
    RegisterClassW, TranslateMessage, HMENU, HWND_MESSAGE, MSG, PM_REMOVE, WINDOW_EX_STYLE,
    WINDOW_STYLE, WM_RENDERALLFORMATS, WM_RENDERFORMAT, WNDCLASSW,
};

use crate::capture::decode_to_bgra_bottom_up;

const CF_DIB: u32 = 8;

fn png_format() -> u32 {
    static F: OnceLock<u32> = OnceLock::new();
    *F.get_or_init(|| unsafe { RegisterClipboardFormatW(w!("PNG")) })
}

/// Marker that keeps our screenshots out of Win+V clipboard history.
fn no_history_format() -> u32 {
    static F: OnceLock<u32> = OnceLock::new();
    *F.get_or_init(|| unsafe { RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing")) })
}

/// One time an app pulled the promised image.
#[derive(Clone)]
pub struct Read {
    pub pid: u32,
    pub format: &'static str,
    pub at: Instant,
}

#[derive(Default)]
struct Promise {
    png: Option<Vec<u8>>,
    reads: Vec<Read>,
}

thread_local! {
    // The image currently promised on the clipboard, and who has pulled it.
    static PROMISE: RefCell<Promise> = RefCell::new(Promise::default());
}

/// An invisible window that owns the clipboard promise; Windows sends it the
/// "someone wants the image now" message.
pub fn create_owner_window() -> HWND {
    static REGISTER: Once = Once::new();
    unsafe {
        let hinst = GetModuleHandleW(None).map(|m| HINSTANCE(m.0)).unwrap_or_default();
        REGISTER.call_once(|| {
            RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(owner_proc),
                hInstance: hinst,
                lpszClassName: w!("SnapRelayClipboard"),
                ..Default::default()
            });
        });
        CreateWindowExW(
            WINDOW_EX_STYLE(0), w!("SnapRelayClipboard"), w!("SnapRelay"), WINDOW_STYLE(0),
            0, 0, 0, 0, HWND_MESSAGE, HMENU::default(), hinst, None,
        )
        .expect("couldn't create the clipboard owner window")
    }
}

extern "system" fn owner_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            // An app is pasting: Windows wants the real data for this format, now.
            WM_RENDERFORMAT => {
                deliver(wp.0 as u32);
                LRESULT(0)
            }
            // We're shutting down while still holding a promise: fill it in fully.
            WM_RENDERALLFORMATS => {
                if OpenClipboard(hwnd).is_ok() {
                    deliver(png_format());
                    deliver(CF_DIB);
                    let _ = CloseClipboard();
                }
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

/// Hand the promised image to Windows in the format that was asked for,
/// and note who asked.
unsafe fn deliver(format: u32) {
    let Some(png) = PROMISE.with(|p| p.borrow().png.clone()) else { return };
    let (bytes, label) = if format == png_format() {
        (png, "PNG")
    } else if format == CF_DIB {
        match make_dib(&png) {
            Ok(dib) => (dib, "bitmap"),
            Err(_) => return,
        }
    } else {
        return;
    };
    let _ = set_data(format, &bytes);

    let mut pid = 0u32;
    if let Ok(reader) = GetOpenClipboardWindow() {
        GetWindowThreadProcessId(reader, Some(&mut pid as *mut u32));
    }
    PROMISE.with(|p| p.borrow_mut().reads.push(Read { pid, format: label, at: Instant::now() }));
}

/// Replace whatever is on the clipboard with a promise of this image.
pub fn promise(owner: HWND, png: &[u8]) -> Result<(), String> {
    PROMISE.with(|p| {
        let mut p = p.borrow_mut();
        p.png = Some(png.to_vec());
        p.reads.clear();
    });
    unsafe {
        open_with_retry(owner)?;
        let result = (|| {
            EmptyClipboard().map_err(|e| e.to_string())?;
            // No data (null handle) = "ask me when you need it".
            let _ = SetClipboardData(png_format(), HANDLE::default());
            let _ = SetClipboardData(CF_DIB, HANDLE::default());
            set_data(no_history_format(), &[0, 0, 0, 0])?;
            Ok(())
        })();
        let _ = CloseClipboard();
        result
    }
}

/// Wait until an app from `target_app` pulls the image (the signal), or give up.
/// On timeout, returns whoever else pulled it, if anyone.
pub fn wait_for_read(owner: HWND, is_target: impl Fn(u32) -> bool, timeout: Duration) -> Result<Read, Vec<Read>> {
    let start = Instant::now();
    loop {
        pump(owner);
        let reads = PROMISE.with(|p| p.borrow().reads.clone());
        if let Some(r) = reads.iter().find(|r| r.pid == 0 || is_target(r.pid)) {
            return Ok(r.clone());
        }
        if start.elapsed() > timeout {
            return Err(reads);
        }
        sleep(Duration::from_millis(2));
    }
}

/// While waiting, keep answering Windows. Only this window's messages are taken,
/// so hotkey presses stay queued for the main loop. (Windows' "give me the image"
/// requests are delivered during any PeekMessage call.)
pub fn pump(owner: HWND) {
    unsafe {
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, owner, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// The app that just pasted may still have the clipboard open; it closes it when
/// done reading. Keep answering Windows while we wait our turn.
unsafe fn open_with_retry(owner: HWND) -> Result<(), String> {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(2) {
        if OpenClipboard(owner).is_ok() {
            return Ok(());
        }
        pump(owner);
        sleep(Duration::from_millis(5));
    }
    Err("clipboard stayed busy for 2 s".into())
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

/// Bitmap form for apps that don't take PNG. Only built if an app asks for it.
fn make_dib(png: &[u8]) -> Result<Vec<u8>, String> {
    let (w, h, bgra) = decode_to_bgra_bottom_up(png)?;
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
    Ok(dib)
}
