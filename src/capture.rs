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
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// Compressed image. The only full copy of the pixels we keep.
    pub png: Vec<u8>,
    /// Small picture for the queue window, made once at capture time:
    /// (width, height, BGRA rows top-down). Tens of KB.
    pub preview: (u32, u32, Vec<u8>),
}

/// Uncompressed pixels straight from the screen copy. Short-lived: compressed
/// into a Frame right away.
pub struct Raw {
    pub area: RECT,
    /// Scale of the monitor it came from (96 = 100%), for sizing the preview.
    dpi: u32,
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

    pub fn dpi(&self) -> u32 {
        self.dpi
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
    Raw { area, dpi: raw.dpi, width: w, height: h, bgra: out }
}

/// Slow step (~hundreds of ms): make the queue preview and the PNG from the one
/// raw copy we have, then drop the raw pixels. The preview fits in `max_w` x `max_h`.
pub fn compress(raw: Raw, (max_w, max_h): (u32, u32)) -> Result<Frame, String> {
    // Preview first: the PNG step below rewrites the raw pixels in place.
    let preview = shrink(raw.width, raw.height, &raw.bgra, max_w, max_h);
    let png = encode_png(raw.width, raw.height, raw.bgra)?;
    Ok(Frame { width: raw.width, height: raw.height, png, preview })
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
    let (mut dpi, mut _dpi_y) = (96u32, 96u32);
    let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi, &mut _dpi_y);

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
    Ok(Raw { area: r, dpi, width: w as u32, height: h as u32, bgra })
}

/// Compress to PNG. Windows gives 4 numbers per dot (blue, green, red, unused);
/// PNG wants 3 (red, green, blue). We rewrite them inside the same memory rather
/// than making an 11 MB second copy: writing 3 per dot while reading 4 per dot
/// always stays behind the reading, so nothing is overwritten before it's read.
fn encode_png(w: u32, h: u32, mut px: Vec<u8>) -> Result<Vec<u8>, String> {
    let dots = (w * h) as usize;
    for i in 0..dots {
        let (b, g, r) = (px[i * 4], px[i * 4 + 1], px[i * 4 + 2]);
        px[i * 3] = r;
        px[i * 3 + 1] = g;
        px[i * 3 + 2] = b;
    }
    let rgb = &px[..dots * 3];
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        let mut writer = enc.write_header().map_err(|e| e.to_string())?;
        writer.write_image_data(rgb).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// Shrink BGRA pixels (rows top-down) to fit inside max_w x max_h, keeping the shape.
/// Each output dot averages a sample of the source dots it covers.
pub fn shrink(w: u32, h: u32, bgra: &[u8], max_w: u32, max_h: u32) -> (u32, u32, Vec<u8>) {
    let scale = (max_w as f32 / w as f32).min(max_h as f32 / h as f32).min(1.0);
    let tw = ((w as f32 * scale).round() as u32).max(1);
    let th = ((h as f32 * scale).round() as u32).max(1);
    let mut out = Vec::with_capacity((tw * th * 4) as usize);
    for ty in 0..th {
        let (y0, y1) = (ty * h / th, ((ty + 1) * h / th).max(ty * h / th + 1));
        let ystep = ((y1 - y0) / 4).max(1);
        for tx in 0..tw {
            let (x0, x1) = (tx * w / tw, ((tx + 1) * w / tw).max(tx * w / tw + 1));
            let xstep = ((x1 - x0) / 4).max(1);
            let (mut b, mut g, mut r, mut n) = (0u32, 0u32, 0u32, 0u32);
            for y in (y0..y1).step_by(ystep as usize) {
                for x in (x0..x1).step_by(xstep as usize) {
                    let i = ((y * w + x) * 4) as usize;
                    b += bgra[i] as u32;
                    g += bgra[i + 1] as u32;
                    r += bgra[i + 2] as u32;
                    n += 1;
                }
            }
            out.extend_from_slice(&[(b / n) as u8, (g / n) as u8, (r / n) as u8, 255]);
        }
    }
    (tw, th, out)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The in-place rewrite must keep every colour exactly (no swapped red/blue).
    #[test]
    fn png_keeps_exact_colours() {
        // 3 x 2 dots, BGRA as Windows gives them: blue, green, red, unused.
        let dots: [[u8; 3]; 6] = [[255, 0, 0], [0, 255, 0], [0, 0, 255], [16, 163, 127], [1, 2, 3], [250, 251, 252]];
        let bgra: Vec<u8> = dots.iter().flat_map(|&[r, g, b]| [b, g, r, 0]).collect();
        let png = encode_png(3, 2, bgra).unwrap();
        let mut reader = png::Decoder::new(&png[..]).read_info().unwrap();
        let mut out = vec![0u8; reader.output_buffer_size()];
        let info = reader.next_frame(&mut out).unwrap();
        assert_eq!(info.color_type, png::ColorType::Rgb);
        let want: Vec<u8> = dots.iter().flatten().copied().collect();
        assert_eq!(&out[..info.buffer_size()], &want[..]);
    }

    #[test]
    fn preview_keeps_shape_and_colour() {
        // A 2560 x 1440 all-teal screen shrinks to fit 144 x 81 and stays teal.
        let bgra: Vec<u8> = std::iter::repeat([127, 163, 16, 0]).take(2560 * 1440).flatten().collect();
        let (w, h, px) = shrink(2560, 1440, &bgra, 144, 81);
        assert_eq!((w, h), (144, 81));
        assert_eq!(&px[..4], &[127, 163, 16, 255]);
    }
}
