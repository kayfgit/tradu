//! The paint hotkey: freezes the monitor under the mouse, lets the user brush
//! over text on it, and hands back what was painted for OCR.

use std::cell::RefCell;
use std::sync::atomic::{AtomicI32, Ordering};

use windows::core::{w, Interface};
use windows::Win32::Foundation::{COLORREF, E_FAIL, HMODULE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_MODE_ROTATION_IDENTITY, DXGI_MODE_ROTATION_UNSPECIFIED,
};
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1, IDXGIOutput1, IDXGIResource, DXGI_OUTDUPL_FRAME_INFO};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, SetFocus, VK_BACK, VK_CONTROL, VK_ESCAPE, VK_OEM_4, VK_OEM_6,
    VK_RETURN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::ocr::{Image, Painted};

/// The brush radius in logical pixels, kept between paints.
static BRUSH: AtomicI32 = AtomicI32::new(11);
const BRUSH_MIN: i32 = 3;
const BRUSH_MAX: i32 = 80;

// tradu's palette, as BGR.
const AMBER: [u8; 3] = [0x3a, 0xa3, 0xf2];
const AMBER_REF: COLORREF = COLORREF(0x003aa3f2);
const BAR_REF: COLORREF = COLORREF(0x00161a1c);
const TEXT_REF: COLORREF = COLORREF(0x00d8e4e9);
const MUTED_REF: COLORREF = COLORREF(0x0083929a);

const HINTS: [(&str, &str); 5] = [
    ("enter", "read"),
    ("shift", "keep painting"),
    ("right-click", "undo"),
    ("wheel", "brush size"),
    ("esc", "cancel"),
];

/// Captures the monitor under the mouse and lets the user paint on it.
/// None if they cancelled.
pub fn run() -> Option<Painted> {
    let (monitor, area) = monitor_under_cursor();
    let shot = unsafe { capture_dxgi(monitor) }.or_else(|_| unsafe { capture_gdi(area) }).ok()?;
    unsafe { overlay(shot, area) }
}

fn monitor_under_cursor() -> (HMONITOR, RECT) {
    unsafe {
        let mut cursor = POINT::default();
        let _ = GetCursorPos(&mut cursor);
        let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(monitor, &mut info);
        (monitor, info.rcMonitor)
    }
}

// ---------------------------------------------------------------- capture

/// Desktop Duplication: sees borderless and flip-model games, which a plain
/// screen copy can miss.
unsafe fn capture_dxgi(monitor: HMONITOR) -> windows::core::Result<Image> {
    let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
    let mut a = 0;
    while let Ok(adapter) = factory.EnumAdapters1(a) {
        a += 1;
        let mut o = 0;
        while let Ok(output) = adapter.EnumOutputs(o) {
            o += 1;
            let desc = output.GetDesc()?;
            if desc.Monitor != monitor {
                continue;
            }
            if desc.Rotation != DXGI_MODE_ROTATION_IDENTITY && desc.Rotation != DXGI_MODE_ROTATION_UNSPECIFIED {
                return Err(E_FAIL.into());
            }
            let mut device: Option<ID3D11Device> = None;
            let mut context: Option<ID3D11DeviceContext> = None;
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;
            let (device, context) = (device.ok_or(E_FAIL)?, context.ok_or(E_FAIL)?);
            let duplication = output.cast::<IDXGIOutput1>()?.DuplicateOutput(&device)?;

            // The first frames can be pointer-only updates without an image.
            let mut texture = None;
            for _ in 0..4 {
                let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
                let mut resource: Option<IDXGIResource> = None;
                duplication.AcquireNextFrame(60, &mut info, &mut resource)?;
                if info.LastPresentTime != 0 {
                    texture = Some(resource.ok_or(E_FAIL)?.cast::<ID3D11Texture2D>()?);
                    break;
                }
                duplication.ReleaseFrame()?;
            }
            let texture = texture.ok_or(E_FAIL)?;
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&mut desc);
            if desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM {
                let _ = duplication.ReleaseFrame();
                return Err(E_FAIL.into());
            }
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            desc.MiscFlags = 0;
            let mut staging: Option<ID3D11Texture2D> = None;
            device.CreateTexture2D(&desc, None, Some(&mut staging))?;
            let staging = staging.ok_or(E_FAIL)?;
            context.CopyResource(&staging, &texture);
            duplication.ReleaseFrame()?;

            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let (width, height) = (desc.Width as i32, desc.Height as i32);
            let row = width as usize * 4;
            let mut pixels = vec![0u8; row * height as usize];
            for y in 0..height as usize {
                let src = (mapped.pData as *const u8).add(y * mapped.RowPitch as usize);
                std::ptr::copy_nonoverlapping(src, pixels.as_mut_ptr().add(y * row), row);
            }
            context.Unmap(&staging, 0);
            return Ok(Image { width, height, pixels });
        }
    }
    Err(E_FAIL.into())
}

/// A 32-bit top-down bitmap and a pointer to its pixels.
unsafe fn dib(dc: HDC, width: i32, height: i32) -> windows::core::Result<(HBITMAP, *mut u8)> {
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits = std::ptr::null_mut();
    let bitmap = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
    Ok((bitmap, bits as *mut u8))
}

/// A plain screen copy, for when duplication isn't available.
unsafe fn capture_gdi(area: RECT) -> windows::core::Result<Image> {
    let (width, height) = (area.right - area.left, area.bottom - area.top);
    let screen = GetDC(None);
    let memory = CreateCompatibleDC(Some(screen));
    let result = dib(memory, width, height).and_then(|(bitmap, bits)| {
        let old = SelectObject(memory, bitmap.into());
        let copied = BitBlt(memory, 0, 0, width, height, Some(screen), area.left, area.top, SRCCOPY | CAPTUREBLT);
        let pixels = std::slice::from_raw_parts(bits, (width * height * 4) as usize).to_vec();
        SelectObject(memory, old);
        let _ = DeleteObject(bitmap.into());
        copied.map(|_| Image { width, height, pixels })
    });
    let _ = DeleteDC(memory);
    ReleaseDC(None, screen);
    result
}

// ---------------------------------------------------------------- overlay

struct Stroke {
    radius: i32,
    points: Vec<(i32, i32)>,
}

struct Overlay {
    shot: Image,
    /// What's on screen: the dimmed screenshot with painted parts lit up.
    canvas_dc: HDC,
    canvas_bitmap: HBITMAP,
    canvas_old: HGDIOBJ,
    canvas: *mut u8,
    mask: Vec<u8>,
    bounds: Option<[i32; 4]>,
    strokes: Vec<Stroke>,
    painting: bool,
    radius: i32,
    scale: f32,
    cursor: (i32, i32),
    hint: Option<RECT>,
    font: HFONT,
    /// Some(true): read what's painted. Some(false): cancelled.
    outcome: Option<bool>,
}

thread_local! {
    static OVERLAY: RefCell<Option<Overlay>> = const { RefCell::new(None) };
}

unsafe fn overlay(shot: Image, area: RECT) -> Option<Painted> {
    let instance = GetModuleHandleW(None).ok()?;
    let class = WNDCLASSW {
        lpfnWndProc: Some(window_proc),
        hInstance: instance.into(),
        lpszClassName: w!("tradu-paint"),
        ..Default::default()
    };
    RegisterClassW(&class); // fails harmlessly when already registered

    let (width, height) = (shot.width, shot.height);
    let hwnd = CreateWindowExW(
        WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
        w!("tradu-paint"),
        w!("tradu paint"),
        WS_POPUP,
        area.left,
        area.top,
        width,
        height,
        None,
        None,
        Some(instance.into()),
        None,
    )
    .ok()?;

    let screen = GetDC(Some(hwnd));
    let canvas_dc = CreateCompatibleDC(Some(screen));
    ReleaseDC(Some(hwnd), screen);
    let Ok((canvas_bitmap, canvas)) = dib(canvas_dc, width, height) else {
        let _ = DeleteDC(canvas_dc);
        let _ = DestroyWindow(hwnd);
        return None;
    };
    let canvas_old = SelectObject(canvas_dc, canvas_bitmap.into());

    let scale = GetDpiForWindow(hwnd) as f32 / 96.0;
    let font = CreateFontW(
        -(13.0 * scale) as i32, 0, 0, 0, FW_NORMAL.0 as i32, 0, 0, 0,
        DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY,
        (FIXED_PITCH.0 | FF_MODERN.0) as u32, w!("Consolas"),
    );
    let mut cursor = POINT::default();
    let _ = GetCursorPos(&mut cursor);

    let mut overlay = Overlay {
        mask: vec![0; (width * height) as usize],
        shot,
        canvas_dc,
        canvas_bitmap,
        canvas_old,
        canvas,
        bounds: None,
        strokes: Vec::new(),
        painting: false,
        radius: (BRUSH.load(Ordering::Relaxed) as f32 * scale) as i32,
        scale,
        cursor: (cursor.x - area.left, cursor.y - area.top),
        hint: None,
        font,
        outcome: None,
    };
    overlay.redraw_canvas();
    overlay.hint = Some(overlay.hint_rect());
    OVERLAY.with(|o| *o.borrow_mut() = Some(overlay));

    let _ = ShowWindow(hwnd, SW_SHOW);
    let _ = SetForegroundWindow(hwnd);
    let _ = SetFocus(Some(hwnd));

    let mut msg = MSG::default();
    while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
        let _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }

    let overlay = OVERLAY.with(|o| o.borrow_mut().take())?;
    let _ = DestroyWindow(hwnd);
    SelectObject(overlay.canvas_dc, overlay.canvas_old);
    let _ = DeleteObject(overlay.canvas_bitmap.into());
    let _ = DeleteDC(overlay.canvas_dc);
    let _ = DeleteObject(overlay.font.into());
    BRUSH.store((overlay.radius as f32 / overlay.scale).round() as i32, Ordering::Relaxed);

    let bounds = overlay.bounds?;
    (overlay.outcome == Some(true)).then_some(Painted {
        shot: overlay.shot,
        mask: overlay.mask,
        bounds,
        brush: overlay.radius,
    })
}

impl Overlay {
    fn canvas(&mut self) -> &mut [u8] {
        let len = (self.shot.width * self.shot.height * 4) as usize;
        unsafe { std::slice::from_raw_parts_mut(self.canvas, len) }
    }

    /// The dimmed screenshot, with every stroke lit up again.
    fn redraw_canvas(&mut self) {
        let shot = std::mem::take(&mut self.shot.pixels);
        for (out, src) in self.canvas().chunks_exact_mut(4).zip(shot.chunks_exact(4)) {
            for c in 0..3 {
                out[c] = ((src[c] as u32 * 100) >> 8) as u8;
            }
        }
        self.shot.pixels = shot;
        self.mask.fill(0);
        self.bounds = None;
        let strokes = std::mem::take(&mut self.strokes);
        for stroke in &strokes {
            let mut last = stroke.points[0];
            for &point in &stroke.points {
                self.stamp(last, point, stroke.radius);
                last = point;
            }
        }
        self.strokes = strokes;
    }

    /// Paints a round-ended line from `a` to `b`, returning the area it touched.
    fn stamp(&mut self, a: (i32, i32), b: (i32, i32), radius: i32) -> RECT {
        let (width, height) = (self.shot.width, self.shot.height);
        let x0 = (a.0.min(b.0) - radius).max(0);
        let y0 = (a.1.min(b.1) - radius).max(0);
        let x1 = (a.0.max(b.0) + radius + 1).min(width);
        let y1 = (a.1.max(b.1) + radius + 1).min(height);
        let (dx, dy) = ((b.0 - a.0) as f32, (b.1 - a.1) as f32);
        let length = dx * dx + dy * dy;
        let r2 = (radius * radius) as f32;
        let canvas = self.canvas;
        for y in y0..y1 {
            for x in x0..x1 {
                let i = (y * width + x) as usize;
                if self.mask[i] != 0 {
                    continue;
                }
                let (px, py) = ((x - a.0) as f32, (y - a.1) as f32);
                let t = if length > 0.0 { ((px * dx + py * dy) / length).clamp(0.0, 1.0) } else { 0.0 };
                let (ex, ey) = (px - t * dx, py - t * dy);
                if ex * ex + ey * ey > r2 {
                    continue;
                }
                self.mask[i] = 1;
                // Lit up, with a touch of amber.
                for c in 0..3 {
                    let v = self.shot.pixels[i * 4 + c] as u32 * 226 + AMBER[c] as u32 * 30;
                    unsafe { *canvas.add(i * 4 + c) = (v >> 8) as u8 };
                }
            }
        }
        if x0 < x1 && y0 < y1 {
            self.bounds = Some(match self.bounds {
                Some([l, t, r, b]) => [l.min(x0), t.min(y0), r.max(x1), b.max(y1)],
                None => [x0, y0, x1, y1],
            });
        }
        RECT { left: x0, top: y0, right: x1, bottom: y1 }
    }

    fn ring(&self) -> RECT {
        let r = self.radius + 4;
        RECT { left: self.cursor.0 - r, top: self.cursor.1 - r, right: self.cursor.0 + r, bottom: self.cursor.1 + r }
    }

    /// The pill of key hints at the top, centered.
    fn hint_rect(&self) -> RECT {
        let width = unsafe { self.hint_width() };
        let height = (30.0 * self.scale) as i32;
        let left = (self.shot.width - width) / 2;
        let top = (18.0 * self.scale) as i32;
        RECT { left, top, right: left + width, bottom: top + height }
    }

    unsafe fn hint_width(&self) -> i32 {
        let dc = self.canvas_dc;
        let old = SelectObject(dc, self.font.into());
        let gap = (18.0 * self.scale) as i32;
        let mut width = (16.0 * self.scale) as i32 * 2 - gap;
        for (key, label) in HINTS {
            width += text_width(dc, key) + text_width(dc, &format!(" {label}")) + gap;
        }
        SelectObject(dc, old);
        width
    }

    unsafe fn draw_hint(&self, dc: HDC, rect: RECT) {
        let brush = CreateSolidBrush(BAR_REF);
        let pen = CreatePen(PS_SOLID, 1, BAR_REF);
        let (old_brush, old_pen) = (SelectObject(dc, brush.into()), SelectObject(dc, pen.into()));
        let round = (rect.bottom - rect.top) / 2;
        let _ = RoundRect(dc, rect.left, rect.top, rect.right, rect.bottom, round * 2, round * 2);
        SelectObject(dc, old_brush);
        SelectObject(dc, old_pen);
        let _ = DeleteObject(brush.into());
        let _ = DeleteObject(pen.into());

        let old_font = SelectObject(dc, self.font.into());
        SetBkMode(dc, TRANSPARENT);
        let mut size = SIZE::default();
        let _ = GetTextExtentPoint32W(dc, &wide("x"), &mut size);
        let y = rect.top + (rect.bottom - rect.top - size.cy) / 2;
        let gap = (18.0 * self.scale) as i32;
        let mut x = rect.left + (16.0 * self.scale) as i32;
        for (key, label) in HINTS {
            SetTextColor(dc, AMBER_REF);
            let _ = TextOutW(dc, x, y, &wide(key));
            x += text_width(dc, key);
            SetTextColor(dc, MUTED_REF);
            let label = format!(" {label}");
            let _ = TextOutW(dc, x, y, &wide(&label));
            x += text_width(dc, &label) + gap;
        }
        SetTextColor(dc, TEXT_REF);
        SelectObject(dc, old_font);
    }

    unsafe fn draw_ring(&self, dc: HDC) {
        let (x, y, r) = (self.cursor.0, self.cursor.1, self.radius);
        let old_brush = SelectObject(dc, GetStockObject(NULL_BRUSH));
        // Dark under amber, so the ring shows on light and dark alike.
        for (width, color) in [(4, BAR_REF), (2, AMBER_REF)] {
            let pen = CreatePen(PS_SOLID, width, color);
            let old = SelectObject(dc, pen.into());
            let _ = Ellipse(dc, x - r, y - r, x + r + 1, y + r + 1);
            SelectObject(dc, old);
            let _ = DeleteObject(pen.into());
        }
        SelectObject(dc, old_brush);
    }

    unsafe fn paint(&self, hwnd: HWND) {
        let mut ps = PAINTSTRUCT::default();
        let dc = BeginPaint(hwnd, &mut ps);
        self.draw(dc, ps.rcPaint);
        let _ = EndPaint(hwnd, &ps);
    }

    /// Draws the part `r` of the overlay into `dc`.
    unsafe fn draw(&self, dc: HDC, r: RECT) {
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        if w <= 0 || h <= 0 {
            return;
        }
        // Drawn off screen first, so the ring doesn't flicker.
        let back = CreateCompatibleDC(Some(dc));
        let bitmap = CreateCompatibleBitmap(dc, w, h);
        let old = SelectObject(back, bitmap.into());
        let _ = BitBlt(back, 0, 0, w, h, Some(self.canvas_dc), r.left, r.top, SRCCOPY);
        let _ = SetViewportOrgEx(back, -r.left, -r.top, None);
        if let Some(hint) = self.hint {
            self.draw_hint(back, hint);
        }
        self.draw_ring(back);
        let _ = SetViewportOrgEx(back, 0, 0, None);
        let _ = BitBlt(dc, r.left, r.top, w, h, Some(back), 0, 0, SRCCOPY);
        SelectObject(back, old);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(back);
    }

    fn finish(&mut self, read: bool) {
        self.outcome = Some(read);
        unsafe { PostQuitMessage(0) };
    }

    fn undo(&mut self, hwnd: HWND) {
        if self.strokes.pop().is_none() {
            return self.finish(false);
        }
        self.redraw_canvas();
        unsafe {
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }

    fn resize_brush(&mut self, hwnd: HWND, steps: i32) {
        let before = self.ring();
        let step = (2.0 * self.scale).max(1.0) as i32;
        let (min, max) = ((BRUSH_MIN as f32 * self.scale) as i32, (BRUSH_MAX as f32 * self.scale) as i32);
        self.radius = (self.radius + steps * step).clamp(min, max);
        invalidate(hwnd, before);
        invalidate(hwnd, self.ring());
    }

    fn handle(&mut self, hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
        let point = || ((lparam.0 & 0xffff) as i16 as i32, ((lparam.0 >> 16) & 0xffff) as i16 as i32);
        match msg {
            WM_PAINT => unsafe { self.paint(hwnd) },
            WM_PRINTCLIENT => unsafe {
                let all = RECT { left: 0, top: 0, right: self.shot.width, bottom: self.shot.height };
                self.draw(HDC(wparam.0 as _), all);
            },
            WM_ERASEBKGND => return Some(LRESULT(1)),
            WM_SETCURSOR => {
                // The ring is the cursor.
                unsafe { SetCursor(None) };
                return Some(LRESULT(1));
            }
            WM_MOUSEMOVE => {
                let before = self.ring();
                let last = self.cursor;
                self.cursor = point();
                if self.painting {
                    let touched = self.stamp(last, self.cursor, self.radius);
                    if let Some(stroke) = self.strokes.last_mut() {
                        stroke.points.push(self.cursor);
                    }
                    invalidate(hwnd, touched);
                }
                invalidate(hwnd, before);
                invalidate(hwnd, self.ring());
            }
            WM_LBUTTONDOWN => {
                unsafe { SetCapture(hwnd) };
                self.painting = true;
                self.cursor = point();
                self.strokes.push(Stroke { radius: self.radius, points: vec![self.cursor] });
                let touched = self.stamp(self.cursor, self.cursor, self.radius);
                invalidate(hwnd, touched);
                // The hints go once painting starts: they might cover text.
                if let Some(hint) = self.hint.take() {
                    invalidate(hwnd, hint);
                }
            }
            WM_LBUTTONUP if self.painting => {
                self.painting = false;
                unsafe {
                    let _ = ReleaseCapture();
                }
                if !key_down(VK_SHIFT.0) {
                    self.finish(true);
                }
            }
            WM_RBUTTONDOWN => self.undo(hwnd),
            WM_MOUSEWHEEL => {
                let delta = ((wparam.0 >> 16) & 0xffff) as i16 as i32;
                self.resize_brush(hwnd, delta.signum());
            }
            WM_KEYDOWN => match wparam.0 as u16 {
                k if k == VK_ESCAPE.0 => self.finish(false),
                k if k == VK_RETURN.0 && self.bounds.is_some() => self.finish(true),
                k if k == VK_BACK.0 => self.undo(hwnd),
                // Ctrl+Z
                0x5A if key_down(VK_CONTROL.0) => self.undo(hwnd),
                k if k == VK_OEM_4.0 => self.resize_brush(hwnd, -1),
                k if k == VK_OEM_6.0 => self.resize_brush(hwnd, 1),
                _ => {}
            },
            WM_ACTIVATE if (wparam.0 & 0xffff) as u32 == WA_INACTIVE => self.finish(false),
            _ => return None,
        }
        Some(LRESULT(0))
    }
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // Messages sent while the overlay is busy (or being torn down) get the defaults.
    let handled = OVERLAY.with(|o| match o.try_borrow_mut() {
        Ok(mut overlay) => overlay.as_mut().and_then(|o| o.handle(hwnd, msg, wparam, lparam)),
        Err(_) => None,
    });
    handled.unwrap_or_else(|| DefWindowProcW(hwnd, msg, wparam, lparam))
}

fn invalidate(hwnd: HWND, rect: RECT) {
    unsafe {
        let _ = InvalidateRect(Some(hwnd), Some(&rect), false);
    }
}

fn key_down(vk: u16) -> bool {
    unsafe { GetKeyState(vk as i32) < 0 }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

unsafe fn text_width(dc: HDC, s: &str) -> i32 {
    let mut size = SIZE::default();
    let _ = GetTextExtentPoint32W(dc, &wide(s), &mut size);
    size.cx
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};

    #[test]
    #[ignore = "reads the screen"]
    fn captures_the_screen() {
        let (monitor, area) = monitor_under_cursor();
        for (name, shot) in [("dxgi", unsafe { capture_dxgi(monitor) }), ("gdi", unsafe { capture_gdi(area) })] {
            let shot = shot.unwrap_or_else(|e| panic!("{name}: {e}"));
            let lit = shot.pixels.chunks_exact(4).filter(|p| p[0] as u32 + p[1] as u32 + p[2] as u32 > 30).count();
            println!("{name}: {}x{}, {:.0}% not black", shot.width, shot.height, lit as f64 * 100.0 / (shot.width * shot.height) as f64);
            assert_eq!((shot.width, shot.height), (area.right - area.left, area.bottom - area.top));
            assert!(lit > 0);
        }
    }

    /// Runs the overlay off screen on a fake tooltip, paints a stroke with
    /// posted mouse messages, saves what it shows, and reads the result.
    #[test]
    #[ignore = "opens a window"]
    fn paints_and_reads() {
        let mut shot = crate::ocr::tests::render(&["Erhöht den Schaden um 20 Prozent", "Gewährt zusätzliche Lauftempo"], 18);
        // Room for the key hints above the text.
        let top = 60;
        let mut pixels = shot.pixels[..(shot.width * top * 4) as usize].to_vec();
        pixels.extend_from_slice(&shot.pixels);
        shot = crate::ocr::Image { width: shot.width, height: shot.height + top, pixels };
        let (w, h) = (shot.width, shot.height);
        let area = RECT { left: -20000, top: -20000, right: -20000 + w, bottom: -20000 + h };
        let out = std::env::var("PAINT_SNAPSHOT").unwrap_or_default();
        std::thread::spawn(move || unsafe {
            let hwnd = loop {
                if let Ok(hwnd) = FindWindowW(w!("tradu-paint"), None) {
                    break hwnd;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            };
            let at = |x: i32, y: i32| LPARAM(((y as u32) << 16 | x as u32) as isize);
            let post = |msg, x, y| {
                let _ = PostMessageW(Some(hwnd), msg, WPARAM(0), at(x, y));
                std::thread::sleep(std::time::Duration::from_millis(15));
            };
            let snapshot = |path: String| {
                if out.is_empty() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
                let dc = GetDC(None);
                let memory = CreateCompatibleDC(Some(dc));
                let (bitmap, bits) = dib(memory, w, h).unwrap();
                let old = SelectObject(memory, bitmap.into());
                let _ = PrintWindow(hwnd, memory, PRINT_WINDOW_FLAGS(0));
                let pixels = std::slice::from_raw_parts(bits, (w * h * 4) as usize);
                let mut file = format!("P6\n{w} {h}\n255\n").into_bytes();
                for p in pixels.chunks_exact(4) {
                    file.extend([p[2], p[1], p[0]]);
                }
                std::fs::write(path, file).unwrap();
                SelectObject(memory, old);
                let _ = DeleteObject(bitmap.into());
                let _ = DeleteDC(memory);
                ReleaseDC(None, dc);
            };
            // Along the second line: "Gewährt zusätzliche".
            let y = 60 + 20 + 27 + 13;
            post(WM_MOUSEMOVE, 22, y);
            snapshot(out.replace(".ppm", "-hint.ppm"));
            post(WM_LBUTTONDOWN, 22, y);
            for x in (22..180).step_by(12) {
                post(WM_MOUSEMOVE, x, y);
            }
            snapshot(out.clone());
            post(WM_LBUTTONUP, 180, y);
        });
        let painted = unsafe { overlay(shot, area) }.expect("a painted result");
        println!("bounds {:?}, brush {}", painted.bounds, painted.brush);
        let engine = crate::ocr::engine("de").ok().or_else(|| crate::ocr::engine("").ok()).unwrap();
        let text = crate::ocr::read(&engine, &painted).unwrap();
        println!("{text:?}");
        assert_eq!(text, "Gewährt zusätzliche");
    }
}
