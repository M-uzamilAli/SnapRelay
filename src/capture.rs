//! Capture Engine: grabs the monitor under the mouse and compresses it to PNG.
//!
//! Uses the classic GDI screen copy for the prototype. The design doc's target is
//! Windows Graphics Capture; it can replace this file without touching the rest.

use std::mem::size_of;
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits,
    GetMonitorInfoW, MonitorFromPoint, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER,
    BI_RGB, CAPTUREBLT, DIB_RGB_COLORS, MONITORINFO, MONITOR_DEFAULTTONEAREST, ROP_CODE, SRCCOPY,
};
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// Compressed image. The only copy of the pixels we keep.
    pub png: Vec<u8>,
}

/// Uncompressed pixels straight from the screen copy. Short-lived: compressed
/// into a Frame right away.
pub struct Raw {
    pub area: RECT,
    width: u32,
    height: u32,
    bgra: Vec<u8>,
}

/// Fast step (~tens of ms): copy the monitor under the mouse.
pub fn grab_monitor_under_cursor() -> Result<Raw, String> {
    unsafe { grab_monitor() }
}

impl Raw {
    pub fn pixels(&self) -> (u32, u32, &[u8]) {
        (self.width, self.height, &self.bgra)
    }
}

/// Keep only the part of the screenshot inside `r` (pixel coordinates).
pub fn crop(raw: Raw, r: RECT) -> Raw {
    let (w, h) = ((r.right - r.left) as u32, (r.bottom - r.top) as u32);
    let row = raw.width as usize * 4;
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in r.top as usize..r.bottom as usize {
        let start = y * row + r.left as usize * 4;
        out.extend_from_slice(&raw.bgra[start..start + w as usize * 4]);
    }
    let area = RECT {
        left: raw.area.left + r.left,
        top: raw.area.top + r.top,
        right: raw.area.left + r.right,
        bottom: raw.area.top + r.bottom,
    };
    Raw { area, width: w, height: h, bgra: out }
}

/// Slow step (~hundreds of ms): compress to PNG. The raw pixels are dropped after.
pub fn compress(raw: Raw) -> Result<Frame, String> {
    let png = encode_png(raw.width, raw.height, &raw.bgra)?;
    Ok(Frame { width: raw.width, height: raw.height, png })
}

unsafe fn grab_monitor() -> Result<Raw, String> {
    let mut pt = POINT::default();
    GetCursorPos(&mut pt).map_err(|e| format!("couldn't read mouse position: {e}"))?;
    let monitor = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
    let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
    if !GetMonitorInfoW(monitor, &mut info).as_bool() {
        return Err("couldn't read monitor size".into());
    }
    let r = info.rcMonitor;
    let (w, h) = (r.right - r.left, r.bottom - r.top);

    let screen = GetDC(HWND::default());
    let mem = CreateCompatibleDC(screen);
    let bmp = CreateCompatibleBitmap(screen, w, h);
    let old = SelectObject(mem, bmp);

    let copied = BitBlt(mem, 0, 0, w, h, screen, r.left, r.top, ROP_CODE(SRCCOPY.0 | CAPTUREBLT.0));

    let mut bi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h, // negative = rows top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bgra = vec![0u8; (w * h * 4) as usize];
    let lines = GetDIBits(mem, bmp, 0, h as u32, Some(bgra.as_mut_ptr().cast()), &mut bi, DIB_RGB_COLORS);

    SelectObject(mem, old);
    let _ = DeleteObject(bmp);
    let _ = DeleteDC(mem);
    ReleaseDC(HWND::default(), screen);

    copied.map_err(|e| format!("screen copy failed: {e}"))?;
    if lines != h {
        return Err(format!("read {lines} of {h} rows from the screen copy"));
    }
    Ok(Raw { area: r, width: w as u32, height: h as u32, bgra })
}

fn encode_png(w: u32, h: u32, bgra: &[u8]) -> Result<Vec<u8>, String> {
    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
    for px in bgra.chunks_exact(4) {
        rgb.extend_from_slice(&[px[2], px[1], px[0]]);
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        let mut writer = enc.write_header().map_err(|e| e.to_string())?;
        writer.write_image_data(&rgb).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// Decode back to bottom-up BGRA rows, the layout the Windows clipboard expects.
pub fn decode_to_bgra_bottom_up(png_bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let mut reader = png::Decoder::new(png_bytes).read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let (w, h) = (info.width, info.height);
    let rgb = &buf[..info.buffer_size()];
    let row = (w * 3) as usize;
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in (0..h as usize).rev() {
        for px in rgb[y * row..(y + 1) * row].chunks_exact(3) {
            out.extend_from_slice(&[px[2], px[1], px[0], 255]);
        }
    }
    Ok((w, h, out))
}
