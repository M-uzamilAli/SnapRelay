//! Smooth (anti-aliased) shapes via GDI+. Plain GDI draws rounded corners and
//! diagonal lines as hard pixel steps; GDI+ blends their edges.

use std::sync::Once;

use std::mem::size_of;

use windows::Win32::Foundation::{COLORREF, HWND, POINT, RECT, SIZE};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject,
    AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS,
    HBITMAP, HDC, HGDIOBJ,
};
use windows::Win32::UI::WindowsAndMessaging::{UpdateLayeredWindow, ULW_ALPHA};
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

/// Stroke-only rounded rectangle (1px, smooth), on the inside edge of `r`.
pub unsafe fn outline(hdc: HDC, r: &RECT, radius: f32, c: COLORREF) {
    let canvas = Canvas::on(hdc);
    let (x, y, w, h) = (r.left as f32, r.top as f32, (r.right - r.left) as f32, (r.bottom - r.top) as f32);
    let edge = rounded_path(x + 0.5, y + 0.5, w - 1.0, h - 1.0, (radius - 0.5).max(0.0));
    let mut pen = std::ptr::null_mut();
    let _ = GdipCreatePen1(argb(c), 1.0, UnitPixel, &mut pen);
    let _ = GdipDrawPath(canvas.0, pen, edge);
    let _ = GdipDeletePen(pen);
    let _ = GdipDeletePath(edge);
}

/// A 32-bit, top-down bitmap we can read and write directly.
unsafe fn dib(screen: HDC, w: i32, h: i32) -> (HDC, HBITMAP, HGDIOBJ, *mut u8) {
    let bi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let dc = CreateCompatibleDC(screen);
    let mut bits = std::ptr::null_mut();
    let bmp = CreateDIBSection(screen, &bi, DIB_RGB_COLORS, &mut bits, None, 0).unwrap_or_default();
    let old = SelectObject(dc, bmp);
    if !bits.is_null() {
        std::ptr::write_bytes(bits as *mut u8, 0, (w * h * 4) as usize);
    }
    (dc, bmp, old, bits as *mut u8)
}

/// Show a layered window as a rounded card with smooth (blended) corners and no
/// shadow. `draw` paints the opaque content; outside the rounded shape the
/// window is transparent, with soft edges. Replaces WM_PAINT for these windows.
pub unsafe fn present_rounded(hwnd: HWND, w: i32, h: i32, radius: f32, border: Option<COLORREF>, draw: impl FnOnce(HDC)) {
    if w <= 0 || h <= 0 {
        return;
    }
    let screen = GetDC(HWND::default());
    let (content_dc, content_bmp, content_old, content) = dib(screen, w, h);
    let (mask_dc, mask_bmp, mask_old, mask) = dib(screen, w, h);
    if !content.is_null() && !mask.is_null() {
        draw(content_dc);
        let full = RECT { left: 0, top: 0, right: w, bottom: h };
        if let Some(b) = border {
            outline(content_dc, &full, radius, b);
        }
        // The shape: white on black, smooth edges. Its brightness becomes the opacity.
        rounded(mask_dc, &full, radius, COLORREF(0x00FF_FFFF), None);
        let n = (w * h * 4) as usize;
        let px = std::slice::from_raw_parts_mut(content, n);
        let m = std::slice::from_raw_parts(mask, n);
        for i in (0..n).step_by(4) {
            let a = m[i] as u32;
            // Windows wants colours pre-multiplied by their opacity.
            px[i] = (px[i] as u32 * a / 255) as u8;
            px[i + 1] = (px[i + 1] as u32 * a / 255) as u8;
            px[i + 2] = (px[i + 2] as u32 * a / 255) as u8;
            px[i + 3] = a as u8;
        }
        let size = SIZE { cx: w, cy: h };
        let origin = POINT::default();
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let _ = UpdateLayeredWindow(hwnd, screen, None, Some(&size), content_dc, Some(&origin), COLORREF(0), Some(&blend), ULW_ALPHA);
    }
    SelectObject(content_dc, content_old);
    SelectObject(mask_dc, mask_old);
    let _ = DeleteObject(content_bmp);
    let _ = DeleteObject(mask_bmp);
    let _ = DeleteDC(content_dc);
    let _ = DeleteDC(mask_dc);
    ReleaseDC(HWND::default(), screen);
}
