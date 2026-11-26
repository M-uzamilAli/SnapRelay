//! Smooth (anti-aliased) shapes via GDI+. Plain GDI draws rounded corners and
//! diagonal lines as hard pixel steps; GDI+ blends their edges.

use std::sync::Once;

use windows::Win32::Foundation::{COLORREF, HWND, RECT};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
};
use windows::Win32::Graphics::Gdi::HDC;
use windows::Win32::Graphics::GdiPlus::{
    FillModeAlternate, GdipAddPathArc, GdipClosePathFigure, GdipCreateFromHDC, GdipCreatePath,
    GdipCreatePen1, GdipCreateSolidFill, GdipDeleteBrush, GdipDeleteGraphics, GdipDeletePath,
    GdipDeletePen, GdipDrawLine, GdipDrawPath, GdipFillPath, GdipSetPenEndCap,
    GdipSetPenStartCap, GdipSetPixelOffsetMode, GdipSetSmoothingMode, GdiplusStartup,
    GdiplusStartupInput, GpGraphics, GpPath, LineCapRound, PixelOffsetModeHalf,
    SmoothingModeAntiAlias, UnitPixel,
};

fn start() {
    static START: Once = Once::new();
    START.call_once(|| unsafe {
        let mut token = 0usize;
        let input = GdiplusStartupInput { GdiplusVersion: 1, ..Default::default() };
        let _ = GdiplusStartup(&mut token, &input, std::ptr::null_mut());
    });
}

/// 0x00BBGGRR (GDI) -> 0xAARRGGBB (GDI+), fully opaque.
fn argb(c: COLORREF) -> u32 {
    let (r, g, b) = (c.0 & 0xFF, (c.0 >> 8) & 0xFF, (c.0 >> 16) & 0xFF);
    0xFF00_0000 | (r << 16) | (g << 8) | b
}

struct Canvas(*mut GpGraphics);

impl Canvas {
    unsafe fn on(hdc: HDC) -> Canvas {
        start();
        let mut g = std::ptr::null_mut();
        let _ = GdipCreateFromHDC(hdc, &mut g);
        let _ = GdipSetSmoothingMode(g, SmoothingModeAntiAlias);
        let _ = GdipSetPixelOffsetMode(g, PixelOffsetModeHalf);
        Canvas(g)
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        unsafe {
            let _ = GdipDeleteGraphics(self.0);
        }
    }
}

unsafe fn rounded_path(x: f32, y: f32, w: f32, h: f32, radius: f32) -> *mut GpPath {
    let mut p = std::ptr::null_mut();
    let _ = GdipCreatePath(FillModeAlternate, &mut p);
    let d = (radius * 2.0).min(w).min(h);
    let _ = GdipAddPathArc(p, x, y, d, d, 180.0, 90.0);
    let _ = GdipAddPathArc(p, x + w - d, y, d, d, 270.0, 90.0);
    let _ = GdipAddPathArc(p, x + w - d, y + h - d, d, d, 0.0, 90.0);
    let _ = GdipAddPathArc(p, x, y + h - d, d, d, 90.0, 90.0);
    let _ = GdipClosePathFigure(p);
    p
}

/// Filled rounded rectangle, with an optional 1px border on its inside edge.
pub unsafe fn rounded(hdc: HDC, r: &RECT, radius: f32, fill: COLORREF, border: Option<COLORREF>) {
    let c = Canvas::on(hdc);
    let (x, y, w, h) = (r.left as f32, r.top as f32, (r.right - r.left) as f32, (r.bottom - r.top) as f32);
    let path = rounded_path(x, y, w, h, radius);
    let mut brush = std::ptr::null_mut();
    let _ = GdipCreateSolidFill(argb(fill), &mut brush);
    let _ = GdipFillPath(c.0, brush.cast(), path);
    let _ = GdipDeleteBrush(brush.cast());
    let _ = GdipDeletePath(path);
    if let Some(b) = border {
        // Inset by half a pixel so the 1px line sits exactly on whole pixels.
        let edge = rounded_path(x + 0.5, y + 0.5, w - 1.0, h - 1.0, (radius - 0.5).max(0.0));
        let mut pen = std::ptr::null_mut();
        let _ = GdipCreatePen1(argb(b), 1.0, UnitPixel, &mut pen);
        let _ = GdipDrawPath(c.0, pen, edge);
        let _ = GdipDeletePen(pen);
        let _ = GdipDeletePath(edge);
    }
}

/// A line with round ends.
pub unsafe fn line(hdc: HDC, (x1, y1): (f32, f32), (x2, y2): (f32, f32), c: COLORREF, width: f32) {
    let canvas = Canvas::on(hdc);
    let mut pen = std::ptr::null_mut();
    let _ = GdipCreatePen1(argb(c), width, UnitPixel, &mut pen);
    let _ = GdipSetPenStartCap(pen, LineCapRound);
    let _ = GdipSetPenEndCap(pen, LineCapRound);
    let _ = GdipDrawLine(canvas.0, pen, x1, y1, x2, y2);
    let _ = GdipDeletePen(pen);
}

/// Ask Windows 11 to round the window's corners itself (smooth, like its own
/// menus) and draw its 1px edge in `border` (None = no edge).
pub fn round_corners(hwnd: HWND, border: Option<COLORREF>) {
    const NO_BORDER: u32 = 0xFFFF_FFFE; // DWMWA_COLOR_NONE
    unsafe {
        let pref = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(
            hwnd, DWMWA_WINDOW_CORNER_PREFERENCE,
            &pref as *const _ as *const std::ffi::c_void, std::mem::size_of_val(&pref) as u32,
        );
        let color: u32 = border.map(|c| c.0).unwrap_or(NO_BORDER);
        let _ = DwmSetWindowAttribute(
            hwnd, DWMWA_BORDER_COLOR,
            &color as *const _ as *const std::ffi::c_void, std::mem::size_of::<u32>() as u32,
        );
    }
}
