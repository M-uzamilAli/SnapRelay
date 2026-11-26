//! Light or dark? Follows the Windows "app mode" setting (Settings > Personalization
//! > Colors). SNAPRELAY_THEME=light|dark overrides it (handy for testing both).

use std::ffi::c_void;

use windows::core::w;
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};

pub fn is_light() -> bool {
    match std::env::var("SNAPRELAY_THEME").ok().as_deref() {
        Some("light") => return true,
        Some("dark") => return false,
        _ => {}
    }
    let mut value: u32 = 1;
    let mut size = std::mem::size_of::<u32>() as u32;
    let err = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut c_void),
            Some(&mut size),
        )
    };
    // Windows' own default is light, so use that if the setting can't be read.
    err.is_err() || value != 0
}
