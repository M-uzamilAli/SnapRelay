//! Capture Engine: grabs the monitor under the mouse and compresses it to PNG.
//!
//! Uses the classic GDI screen copy for the prototype. The design doc's target is
//! Windows Graphics Capture; it can replace this file without touching the rest.

use std::mem::size_of;
use windows::Win32::Foundation::{HWND, POINT};
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

pub fn capture_monitor_under_cursor() -> Result<Frame, String> {
    let (w, h, bgra) = unsafe { grab_monitor()? };
    let png = encode_png(w, h, &bgra)?;
    Ok(Frame { width: w, height: h, png })
}

unsafe fn grab_monitor() -> Result<(u32, u32, Vec<u8>), String> {
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
    Ok((w as u32, h as u32, bgra))
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
