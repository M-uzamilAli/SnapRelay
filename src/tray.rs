//! Tray icon by the clock. Left-click opens or closes the queue window;
//! right-click shows a small menu (Show queue, Recording mode, Quit). The tooltip shows how many
//! screenshots are queued. Like the queue window, a click only records an Action;
//! the main loop carries it out.

use std::cell::{Cell, RefCell};
use std::mem::size_of;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, GetCursorPos,
    GetSystemMetrics, LoadImageW, PostMessageW, RegisterClassW, RegisterWindowMessageW,
    SetForegroundWindow, TrackPopupMenu, HICON, HMENU, IMAGE_ICON, LR_DEFAULTCOLOR, MF_CHECKED, MF_SEPARATOR, MF_UNCHECKED,
    MF_STRING, SM_CXSMICON, SM_CYSMICON, TPM_BOTTOMALIGN, TPM_RETURNCMD, TPM_RIGHTALIGN,
    TPM_RIGHTBUTTON, WINDOW_EX_STYLE, WM_APP, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP, WNDCLASSW,
    WS_POPUP,
};

#[derive(Clone, Copy)]
pub enum Action {
    ToggleQueue,
    /// Recording mode on/off (see recording.rs).
    ToggleRecording,
    Quit,
}

const WM_TRAY: u32 = WM_APP + 1;
const MENU_SHOW: usize = 1;
const MENU_QUIT: usize = 2;
const MENU_RECORDING: usize = 3;
// Resource ids in assets/snaprelay.rc
const ICON_DARK: u16 = 1;
const ICON_LIGHT: u16 = 2;

thread_local! {
    static WINDOW: Cell<Option<HWND>> = const { Cell::new(None) };
    static ACTION: Cell<Option<Action>> = const { Cell::new(None) };
    static TIP: RefCell<String> = const { RefCell::new(String::new()) };
    /// Explorer broadcasts this after it restarts; we then add our icon again.
    static TASKBAR_CREATED: Cell<u32> = const { Cell::new(0) };
}

pub fn create() {
    unsafe {
        let hinst = GetModuleHandleW(None).map(|m| HINSTANCE(m.0)).unwrap_or_default();
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(tray_proc),
            hInstance: hinst,
            lpszClassName: w!("SnapRelayTray"),
            ..Default::default()
        });
        // A hidden normal window (not message-only), so it hears "TaskbarCreated".
        let Ok(hwnd) = CreateWindowExW(
            WINDOW_EX_STYLE(0), w!("SnapRelayTray"), w!("SnapRelay"), WS_POPUP,
            0, 0, 0, 0, HWND::default(), HMENU::default(), hinst, None,
        ) else {
            return;
        };
        WINDOW.with(|w| w.set(Some(hwnd)));
        TASKBAR_CREATED.with(|t| t.set(RegisterWindowMessageW(w!("TaskbarCreated"))));
        TIP.with(|t| *t.borrow_mut() = "SnapRelay".into());
        if notify(NIM_ADD) {
            println!("Tray icon added (Windows may hide it under the ^ arrow next to the clock).");
        } else {
            println!("Couldn't add the tray icon.");
        }
    }
}

pub fn remove() {
    notify(NIM_DELETE);
}

pub fn take_action() -> Option<Action> {
    ACTION.with(|a| a.take())
}

/// Update the hover text (only touches the tray if it changed).
pub fn set_tooltip(text: &str) {
    let changed = TIP.with(|t| {
        let mut t = t.borrow_mut();
        if *t == text {
            false
        } else {
            *t = text.to_string();
            true
        }
    });
    if changed {
        let _ = notify(NIM_MODIFY);
    }
}

fn notify(op: windows::Win32::UI::Shell::NOTIFY_ICON_MESSAGE) -> bool {
    let Some(hwnd) = WINDOW.with(|w| w.get()) else { return false };
    let mut nid = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
        uCallbackMessage: WM_TRAY,
        hIcon: load_icon(),
        ..Default::default()
    };
    TIP.with(|t| {
        for (dst, src) in nid.szTip.iter_mut().zip(t.borrow().encode_utf16().take(127)) {
            *dst = src;
        }
    });
    unsafe { Shell_NotifyIconW(op, &nid).as_bool() }
}

/// Dark icon on a light taskbar, light icon on a dark one, for contrast.
fn load_icon() -> HICON {
    let id = if crate::theme::taskbar_is_light() { ICON_DARK } else { ICON_LIGHT };
    unsafe {
        let hinst = GetModuleHandleW(None).map(|m| HINSTANCE(m.0)).unwrap_or_default();
        LoadImageW(
            hinst,
            PCWSTR(id as usize as *const u16), // a resource number, not a name
            IMAGE_ICON,
            GetSystemMetrics(SM_CXSMICON),
            GetSystemMetrics(SM_CYSMICON),
            LR_DEFAULTCOLOR,
        )
        .map(|h| HICON(h.0))
        .unwrap_or_default()
    }
}

unsafe fn show_menu(hwnd: HWND) {
    let Ok(menu) = CreatePopupMenu() else { return };
    let _ = AppendMenuW(menu, MF_STRING, MENU_SHOW, w!("Show queue"));
    let tick = if crate::recording::is_on() { MF_CHECKED } else { MF_UNCHECKED };
    let _ = AppendMenuW(menu, MF_STRING | tick, MENU_RECORDING, w!("Recording mode (show shortcuts)"));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
    let _ = AppendMenuW(menu, MF_STRING, MENU_QUIT, w!("Quit SnapRelay"));
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    // Required so the menu closes when you click elsewhere.
    let _ = SetForegroundWindow(hwnd);
    let cmd = TrackPopupMenu(
        menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN | TPM_RIGHTALIGN,
        pt.x, pt.y, 0, hwnd, None,
    );
    let _ = PostMessageW(hwnd, WM_NULL, WPARAM(0), LPARAM(0));
    let _ = DestroyMenu(menu);
    match cmd.0 as usize {
        MENU_SHOW => ACTION.with(|a| a.set(Some(Action::ToggleQueue))),
        MENU_QUIT => ACTION.with(|a| a.set(Some(Action::Quit))),
        MENU_RECORDING => ACTION.with(|a| a.set(Some(Action::ToggleRecording))),
        _ => {}
    }
}

extern "system" fn tray_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_TRAY {
            match lp.0 as u32 {
                WM_LBUTTONUP => ACTION.with(|a| a.set(Some(Action::ToggleQueue))),
                WM_RBUTTONUP => show_menu(hwnd),
                _ => {}
            }
            return LRESULT(0);
        }
        if msg != 0 && msg == TASKBAR_CREATED.with(|t| t.get()) {
            let _ = notify(NIM_ADD);
            return LRESULT(0);
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }
}
