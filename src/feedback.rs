//! Capture feedback: a short shutter "flick" sound and a quick white flash over
//! the captured monitor, so the user knows the capture happened.

use std::sync::{Once, OnceLock};
use std::thread::sleep;
use std::time::Duration;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{GetStockObject, UpdateWindow, HBRUSH, WHITE_BRUSH};
use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, PeekMessageW,
    RegisterClassW, SetLayeredWindowAttributes, ShowWindow, TranslateMessage, HMENU, LWA_ALPHA,
    MSG, PM_REMOVE, SW_SHOWNOACTIVATE, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

/// Play the shutter sound without waiting for it to finish.
pub fn click() {
    static WAV: OnceLock<Vec<u8>> = OnceLock::new();
    let wav = WAV.get_or_init(make_flick_wav); // kept alive for the whole run: playback reads it async
    unsafe {
        let _ = PlaySoundW(
            PCWSTR(wav.as_ptr().cast()),
            None,
            SND_MEMORY | SND_ASYNC | SND_NODEFAULT,
        );
    }
}

/// Flash the given screen area white and fade it out (~200 ms).
/// The window is click-through and never takes focus.
pub fn flash(area: RECT) {
    static REGISTER: Once = Once::new();
    unsafe {
        let hinst = GetModuleHandleW(None).map(|m| HINSTANCE(m.0)).unwrap_or_default();
        REGISTER.call_once(|| {
            let class = WNDCLASSW {
                lpfnWndProc: Some(flash_proc),
                hInstance: hinst,
                lpszClassName: w!("SnapRelayFlash"),
                hbrBackground: HBRUSH(GetStockObject(WHITE_BRUSH).0),
                ..Default::default()
            };
            RegisterClassW(&class);
        });
        let Ok(hwnd) = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("SnapRelayFlash"),
            w!(""),
            WS_POPUP,
            area.left,
            area.top,
            area.right - area.left,
            area.bottom - area.top,
            HWND::default(),
            HMENU::default(),
            hinst,
            None,
        ) else {
            return; // feedback is nice-to-have; never fail a capture over it
        };

        let mut alpha: i32 = 110;
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), alpha as u8, LWA_ALPHA);
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        let _ = UpdateWindow(hwnd);
        while alpha > 0 {
            pump(hwnd);
            sleep(Duration::from_millis(16));
            alpha -= 10;
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), alpha.max(0) as u8, LWA_ALPHA);
        }
        let _ = DestroyWindow(hwnd);
    }
}

/// Handle only the flash window's own messages, so hotkey messages stay queued
/// for the main loop.
unsafe fn pump(hwnd: HWND) {
    let mut msg = MSG::default();
    while PeekMessageW(&mut msg, hwnd, 0, 0, PM_REMOVE).as_bool() {
        let _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
}

extern "system" fn flash_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

/// Builds a ~70 ms mono WAV: two quick, decaying noise+tone ticks, like a shutter.
fn make_flick_wav() -> Vec<u8> {
    const RATE: u32 = 44_100;
    let n = (RATE as f32 * 0.07) as usize;
    let mut seed: u32 = 0x1234_5678;
    let mut noise = || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (seed >> 16) as f32 / 32_768.0 - 1.0
    };
    let tick = |t: f32, n: f32| {
        if t < 0.0 {
            return 0.0;
        }
        let env = (-t / 0.006).exp();
        env * (0.6 * n + 0.4 * (2.0 * std::f32::consts::PI * 2_400.0 * t).sin())
    };
    let samples: Vec<i16> = (0..n)
        .map(|i| {
            let t = i as f32 / RATE as f32;
            let nz = noise();
            let v = tick(t, nz) + 0.6 * tick(t - 0.032, nz);
            (v * 0.35 * i16::MAX as f32) as i16
        })
        .collect();

    let data_len = (samples.len() * 2) as u32;
    let mut wav = Vec::with_capacity(44 + data_len as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&RATE.to_le_bytes());
    wav.extend_from_slice(&(RATE * 2).to_le_bytes()); // bytes per second
    wav.extend_from_slice(&2u16.to_le_bytes()); // bytes per sample frame
    wav.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        wav.extend_from_slice(&s.to_le_bytes());
    }
    wav
}
