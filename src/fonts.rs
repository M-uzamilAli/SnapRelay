//! Inter (SIL Open Font License, see assets/fonts/Inter-LICENSE.txt), built into
//! the program and loaded privately: only SnapRelay can use it, nothing is installed.

use windows::core::{w, PCWSTR};
use windows::Win32::Graphics::Gdi::AddFontMemResourceEx;

static REGULAR: &[u8] = include_bytes!("../assets/fonts/Inter-Regular.ttf");
static SEMIBOLD: &[u8] = include_bytes!("../assets/fonts/Inter-SemiBold.ttf");

/// Regular weight.
pub const REGULAR_FACE: PCWSTR = w!("Inter");
/// The real SemiBold. Windows files it under its own name; asking for "Inter" at
/// semibold weight would give a fake, thickened Regular instead.
pub const SEMIBOLD_FACE: PCWSTR = w!("Inter SemiBold");

/// Make Inter available to this process. Call once, before drawing anything.
pub fn load() -> bool {
    [REGULAR, SEMIBOLD].iter().all(|data| {
        let mut count = 0u32;
        let handle = unsafe { AddFontMemResourceEx(data.as_ptr().cast(), data.len() as u32, None, &mut count) };
        !handle.is_invalid()
    })
}
