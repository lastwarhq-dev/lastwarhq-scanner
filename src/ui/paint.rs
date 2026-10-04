//! Draws the window's content: shapes through GDI+ (smooth edges), text through GDI
//! (ClearType), with hand-written declarations, no crates. Sizes are in 96-DPI pixels, scaled.

#![allow(clippy::upper_case_acronyms)]

use std::ffi::c_void;

use crate::ui::status::{DAY_NAMES, Day, Mark, Screen};

pub type HANDLE = *mut c_void;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct RECT {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct PointF {
    x: f32,
    y: f32,
}

#[repr(C)]
struct GdiplusStartupInput {
    version: u32,
    debug_callback: *const c_void,
    suppress_background_thread: i32,
    suppress_external_codecs: i32,
}

#[link(name = "gdiplus")]
unsafe extern "system" {
    fn GdiplusStartup(
        token: *mut usize,
        input: *const GdiplusStartupInput,
        output: *mut c_void,
    ) -> i32;
    fn GdipCreateFromHDC(hdc: HANDLE, graphics: *mut HANDLE) -> i32;
    fn GdipDeleteGraphics(graphics: HANDLE) -> i32;
    fn GdipSetSmoothingMode(graphics: HANDLE, mode: i32) -> i32;
    fn GdipSetPixelOffsetMode(graphics: HANDLE, mode: i32) -> i32;
    fn GdipCreateSolidFill(argb: u32, brush: *mut HANDLE) -> i32;
    fn GdipDeleteBrush(brush: HANDLE) -> i32;
    fn GdipCreatePen1(argb: u32, width: f32, unit: i32, pen: *mut HANDLE) -> i32;
    fn GdipDeletePen(pen: HANDLE) -> i32;
    fn GdipSetPenStartCap(pen: HANDLE, cap: i32) -> i32;
    fn GdipSetPenEndCap(pen: HANDLE, cap: i32) -> i32;
    fn GdipSetPenLineJoin(pen: HANDLE, join: i32) -> i32;
    fn GdipFillEllipse(graphics: HANDLE, brush: HANDLE, x: f32, y: f32, w: f32, h: f32) -> i32;
    fn GdipDrawEllipse(graphics: HANDLE, pen: HANDLE, x: f32, y: f32, w: f32, h: f32) -> i32;
    fn GdipDrawLines(graphics: HANDLE, pen: HANDLE, points: *const PointF, count: i32) -> i32;
    fn GdipCreatePath(fill_mode: i32, path: *mut HANDLE) -> i32;
    fn GdipDeletePath(path: HANDLE) -> i32;
    fn GdipAddPathArc(path: HANDLE, x: f32, y: f32, w: f32, h: f32, start: f32, sweep: f32) -> i32;
    fn GdipClosePathFigure(path: HANDLE) -> i32;
    fn GdipFillPath(graphics: HANDLE, brush: HANDLE, path: HANDLE) -> i32;
    fn GdipDrawPath(graphics: HANDLE, pen: HANDLE, path: HANDLE) -> i32;
    fn GdipCreateBitmapFromScan0(
        width: i32,
        height: i32,
        stride: i32,
        format: i32,
        scan0: *mut u8,
        bitmap: *mut HANDLE,
    ) -> i32;
    fn GdipGetImageGraphicsContext(image: HANDLE, graphics: *mut HANDLE) -> i32;
    fn GdipCreateHICONFromBitmap(bitmap: HANDLE, icon: *mut HANDLE) -> i32;
    fn GdipDisposeImage(image: HANDLE) -> i32;
}

#[link(name = "gdi32")]
unsafe extern "system" {
    fn CreateFontW(
        height: i32,
        width: i32,
        escapement: i32,
        orientation: i32,
        weight: i32,
        italic: u32,
        underline: u32,
        strike_out: u32,
        charset: u32,
        out_precision: u32,
        clip_precision: u32,
        quality: u32,
        pitch_and_family: u32,
        face: *const u16,
    ) -> HANDLE;
    fn SelectObject(hdc: HANDLE, object: HANDLE) -> HANDLE;
    fn DeleteObject(object: HANDLE) -> i32;
    fn SetTextColor(hdc: HANDLE, color: u32) -> u32;
    fn SetBkMode(hdc: HANDLE, mode: i32) -> i32;
    fn CreateSolidBrush(color: u32) -> HANDLE;
}

#[link(name = "user32")]
unsafe extern "system" {
    fn DrawTextW(hdc: HANDLE, text: *const u16, len: i32, rect: *mut RECT, format: u32) -> i32;
    fn FillRect(hdc: HANDLE, rect: *const RECT, brush: HANDLE) -> i32;
}

const SMOOTHING_ANTIALIAS: i32 = 4;
const PIXEL_OFFSET_HALF: i32 = 4;
const UNIT_PIXEL: i32 = 2;
const LINE_CAP_ROUND: i32 = 2;
const LINE_JOIN_ROUND: i32 = 2;
const PIXEL_FORMAT_32BPP_ARGB: i32 = 0x0026_200A;
const TRANSPARENT: i32 = 1;

const DT_CENTER: u32 = 0x1;
const DT_RIGHT: u32 = 0x2;
const DT_VCENTER: u32 = 0x4;
const DT_WORDBREAK: u32 = 0x10;
const DT_SINGLELINE: u32 = 0x20;
const DT_NOPREFIX: u32 = 0x800;
const DT_END_ELLIPSIS: u32 = 0x8000;

/// Colours as 0xRRGGBB.
const WHITE: u32 = 0xFFFFFF;
const TEXT: u32 = 0x111827;
const TEXT_SOFT: u32 = 0x4B5563;
const BORDER: u32 = 0xE3E6EA;
const GREEN: u32 = 0x1E9B4B;
const GREEN_TEXT: u32 = 0x1A8A43;
const GREEN_PALE: u32 = 0xE2F6E7;
const AMBER: u32 = 0xD97706;
const AMBER_TEXT: u32 = 0xB45309;
const AMBER_PALE: u32 = 0xFDF0DA;
const RED: u32 = 0xDC2626;
const RED_PALE: u32 = 0xFDE3E3;
const GREY: u32 = 0x9CA3AF;
const GREY_PALE: u32 = 0xF0F1F3;
pub const BLUE: u32 = 0x0B6BDE;
const BLUE_PRESSED: u32 = 0x0957B8;
const BLUE_DISABLED: u32 = 0x9DBDE6;

/// The layout, in 96-DPI pixels.
pub const WIDTH: f32 = 540.0;
pub const HEIGHT: f32 = FOOTER_Y + FOOTER_H + 16.0;
const MARGIN: f32 = 16.0;
const HEADER_Y: f32 = 22.0;
const BADGE: f32 = 68.0;
const CARD_Y: f32 = 114.0;
const ROWS: f32 = 5.0;
const ROW_H: f32 = 54.0;
const VS_Y: f32 = CARD_Y + ROWS * ROW_H + 24.0;
const TILES_Y: f32 = VS_Y + 34.0;
const TILE_H: f32 = 84.0;
const TILE_GAP: f32 = 8.0;
const DIVIDER_Y: f32 = TILES_Y + TILE_H + 24.0;
const FOOTER_Y: f32 = DIVIDER_Y + 16.0;
const FOOTER_H: f32 = 44.0;
const BUTTON_W: f32 = 148.0;

/// Where the update button goes, in 96-DPI pixels: x, y, width, height.
pub const BUTTON: (f32, f32, f32, f32) = (WIDTH - MARGIN - BUTTON_W, FOOTER_Y, BUTTON_W, FOOTER_H);

/// Which card row (0 = the first) the 96-DPI point (x, y) is on, if any.
pub fn row_at(x: f32, y: f32) -> Option<usize> {
    let inside =
        (MARGIN..WIDTH - MARGIN).contains(&x) && (CARD_Y..CARD_Y + ROWS * ROW_H).contains(&y);
    inside.then(|| ((y - CARD_Y) / ROW_H) as usize)
}

fn argb(rgb: u32) -> u32 {
    0xFF00_0000 | rgb
}

/// 0xRRGGBB as a GDI COLORREF, 0x00BBGGRR.
fn colorref(rgb: u32) -> u32 {
    ((rgb & 0xFF) << 16) | (rgb & 0xFF00) | (rgb >> 16)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// Starts GDI+ for the life of the process; call once before drawing.
pub fn start() -> Result<(), String> {
    let input = GdiplusStartupInput {
        version: 1,
        debug_callback: std::ptr::null(),
        suppress_background_thread: 0,
        suppress_external_codecs: 0,
    };
    let mut token = 0usize;
    // SAFETY: `input` is a valid version-1 startup input; no output is requested.
    let status = unsafe { GdiplusStartup(&mut token, &input, std::ptr::null_mut()) };
    if status != 0 {
        return Err(format!("cannot start GDI+ (status {status})"));
    }
    Ok(())
}

/// How a button is to be drawn.
#[derive(Debug, Clone, Copy)]
pub struct ButtonState {
    pub pressed: bool,
    pub enabled: bool,
    pub focus: bool,
}

#[derive(Clone, Copy)]
enum Font {
    Headline,
    Body,
    Label,
    Small,
    Button,
}

/// Draws onto one device context at one scale.
pub struct Painter {
    hdc: HANDLE,
    scale: f32,
    fonts: Vec<(u8, HANDLE)>,
}

impl Drop for Painter {
    fn drop(&mut self) {
        for (_, font) in &self.fonts {
            // SAFETY: the fonts were created by this painter, and are no longer selected
            // into its DC (each text call restores the DC's previous font).
            unsafe { DeleteObject(*font) };
        }
    }
}

impl Painter {
    /// # Safety
    ///
    /// `hdc` must be a valid device context for the painter's whole life.
    pub unsafe fn new(hdc: HANDLE, scale: f32) -> Painter {
        // SAFETY: plain setting on a DC the caller vouches for.
        unsafe { SetBkMode(hdc, TRANSPARENT) };
        Painter {
            hdc,
            scale,
            fonts: Vec::new(),
        }
    }

    fn px(&self, v: f32) -> f32 {
        (v * self.scale).round()
    }

    fn font(&mut self, font: Font) -> HANDLE {
        let (size, weight) = match font {
            Font::Headline => (30.0, 700),
            Font::Body => (15.0, 400),
            Font::Label => (17.0, 600),
            Font::Small => (15.0, 400),
            Font::Button => (15.0, 600),
        };
        let key = font as u8;
        if let Some((_, handle)) = self.fonts.iter().find(|(k, _)| *k == key) {
            return *handle;
        }
        let face: Vec<u16> = "Segoe UI".encode_utf16().chain([0]).collect();
        // SAFETY: `face` is NUL-terminated; the other arguments are plain values.
        let handle = unsafe {
            CreateFontW(
                -(size * self.scale).round() as i32,
                0,
                0,
                0,
                weight,
                0,
                0,
                0,
                1,
                0,
                0,
                5,
                0,
                face.as_ptr(),
            )
        };
        self.fonts.push((key, handle));
        handle
    }

    /// Runs `draw` with a GDI+ graphics on the DC, smoothing on.
    fn graphics(&self, draw: impl FnOnce(HANDLE)) {
        let mut g: HANDLE = std::ptr::null_mut();
        // SAFETY: the DC is valid; the graphics is deleted before returning.
        unsafe {
            if GdipCreateFromHDC(self.hdc, &mut g) != 0 {
                return;
            }
            GdipSetSmoothingMode(g, SMOOTHING_ANTIALIAS);
            GdipSetPixelOffsetMode(g, PIXEL_OFFSET_HALF);
            draw(g);
            GdipDeleteGraphics(g);
        }
    }

    pub fn fill_rect(&self, x: f32, y: f32, w: f32, h: f32, rgb: u32) {
        let rect = RECT {
            left: self.px(x) as i32,
            top: self.px(y) as i32,
            right: self.px(x + w) as i32,
            bottom: self.px(y + h) as i32,
        };
        // SAFETY: the brush is created, used and deleted here; the DC is valid.
        unsafe {
            let brush = CreateSolidBrush(colorref(rgb));
            FillRect(self.hdc, &rect, brush);
            DeleteObject(brush);
        }
    }

    /// A rounded rectangle, filled with `fill` and outlined with `line`, if given.
    fn rounded(&self, rect: (f32, f32, f32, f32), r: f32, fill: Option<u32>, line: Option<u32>) {
        let (x, y, w, h) = rect;
        let (x, y, w, h, d) = (
            self.px(x),
            self.px(y),
            self.px(w),
            self.px(h),
            self.px(r * 2.0),
        );
        let scale = self.scale;
        self.graphics(|g| {
            // SAFETY: `g` is valid for this closure; every object made here is deleted here.
            unsafe {
                let mut path: HANDLE = std::ptr::null_mut();
                if GdipCreatePath(0, &mut path) != 0 {
                    return;
                }
                GdipAddPathArc(path, x, y, d, d, 180.0, 90.0);
                GdipAddPathArc(path, x + w - d, y, d, d, 270.0, 90.0);
                GdipAddPathArc(path, x + w - d, y + h - d, d, d, 0.0, 90.0);
                GdipAddPathArc(path, x, y + h - d, d, d, 90.0, 90.0);
                GdipClosePathFigure(path);
                if let Some(fill) = fill {
                    let mut brush: HANDLE = std::ptr::null_mut();
                    if GdipCreateSolidFill(argb(fill), &mut brush) == 0 {
                        GdipFillPath(g, brush, path);
                        GdipDeleteBrush(brush);
                    }
                }
                if let Some(line) = line {
                    let mut pen: HANDLE = std::ptr::null_mut();
                    if GdipCreatePen1(argb(line), scale.max(1.0), UNIT_PIXEL, &mut pen) == 0 {
                        GdipDrawPath(g, pen, path);
                        GdipDeletePen(pen);
                    }
                }
                GdipDeletePath(path);
            }
        });
    }

    /// `text` in `rect` (96-DPI x, y, w, h) with DrawText `flags`.
    fn text(&mut self, text: &str, rect: (f32, f32, f32, f32), font: Font, rgb: u32, flags: u32) {
        let handle = self.font(font);
        let (x, y, w, h) = rect;
        let mut r = RECT {
            left: self.px(x) as i32,
            top: self.px(y) as i32,
            right: self.px(x + w) as i32,
            bottom: self.px(y + h) as i32,
        };
        let text = wide(text);
        // SAFETY: the font is alive; `text` and `r` outlive the call; the old font goes back.
        unsafe {
            let old = SelectObject(self.hdc, handle);
            SetTextColor(self.hdc, colorref(rgb));
            DrawTextW(
                self.hdc,
                text.as_ptr(),
                text.len() as i32,
                &mut r,
                flags | DT_NOPREFIX,
            );
            SelectObject(self.hdc, old);
        }
    }

    /// A circle of radius `r` around (cx, cy) showing `mark`: solid colour with a white symbol,
    /// or (`pale`) a pale circle with a coloured symbol.
    fn mark(&self, cx: f32, cy: f32, r: f32, mark: Mark, pale: bool) {
        let (color, light) = match mark {
            Mark::Done => (GREEN, GREEN_PALE),
            Mark::Pending | Mark::Off => (GREY, GREY_PALE),
            Mark::Action => (AMBER, AMBER_PALE),
            Mark::Failed => (RED, RED_PALE),
            Mark::Download => (BLUE, 0xE3EEFC),
        };
        let (fill, ink) = if pale { (light, color) } else { (color, WHITE) };
        let s = self.scale;
        let (cx, cy, r) = (cx * s, cy * s, r * s);
        self.graphics(|g| {
            // SAFETY: `g` is valid for this closure; every object made here is deleted here.
            unsafe {
                let mut brush: HANDLE = std::ptr::null_mut();
                if GdipCreateSolidFill(argb(fill), &mut brush) == 0 {
                    GdipFillEllipse(g, brush, cx - r, cy - r, 2.0 * r, 2.0 * r);
                    GdipDeleteBrush(brush);
                }
                let at = |x: f32, y: f32| PointF {
                    x: cx + x * r,
                    y: cy + y * r,
                };
                let mut pen: HANDLE = std::ptr::null_mut();
                if GdipCreatePen1(argb(ink), r * 0.2, UNIT_PIXEL, &mut pen) != 0 {
                    return;
                }
                GdipSetPenStartCap(pen, LINE_CAP_ROUND);
                GdipSetPenEndCap(pen, LINE_CAP_ROUND);
                GdipSetPenLineJoin(pen, LINE_JOIN_ROUND);
                let lines = |points: &[PointF]| {
                    GdipDrawLines(g, pen, points.as_ptr(), points.len() as i32);
                };
                let dot = |x: f32, y: f32, radius: f32| {
                    let mut brush: HANDLE = std::ptr::null_mut();
                    if GdipCreateSolidFill(argb(ink), &mut brush) == 0 {
                        let p = at(x, y);
                        let d = radius * r;
                        GdipFillEllipse(g, brush, p.x - d, p.y - d, 2.0 * d, 2.0 * d);
                        GdipDeleteBrush(brush);
                    }
                };
                match mark {
                    Mark::Done => lines(&[at(-0.40, 0.02), at(-0.12, 0.30), at(0.40, -0.26)]),
                    Mark::Pending => {
                        for x in [-0.42, 0.0, 0.42] {
                            dot(x, 0.0, 0.12);
                        }
                    }
                    Mark::Action => {
                        lines(&[at(0.0, -0.42), at(0.0, 0.10)]);
                        dot(0.0, 0.38, 0.12);
                    }
                    Mark::Failed => {
                        lines(&[at(-0.28, -0.28), at(0.28, 0.28)]);
                        lines(&[at(0.28, -0.28), at(-0.28, 0.28)]);
                    }
                    Mark::Off => lines(&[at(-0.36, 0.0), at(0.36, 0.0)]),
                    Mark::Download => {
                        lines(&[at(0.0, -0.45), at(0.0, 0.18)]);
                        lines(&[at(-0.24, -0.06), at(0.0, 0.18), at(0.24, -0.06)]);
                        lines(&[at(-0.40, 0.30), at(0.40, 0.30)]);
                    }
                }
                GdipDeletePen(pen);
            }
        });
    }

    /// The bare download symbol, for the footer: an arrow into a tray, in blue.
    fn download_icon(&self, cx: f32, cy: f32, size: f32) {
        let s = self.scale;
        let (cx, cy, h) = (cx * s, cy * s, size * s / 2.0);
        self.graphics(|g| {
            // SAFETY: `g` is valid for this closure; the pen is deleted here.
            unsafe {
                let mut pen: HANDLE = std::ptr::null_mut();
                if GdipCreatePen1(argb(BLUE), 2.0 * s, UNIT_PIXEL, &mut pen) != 0 {
                    return;
                }
                GdipSetPenStartCap(pen, LINE_CAP_ROUND);
                GdipSetPenEndCap(pen, LINE_CAP_ROUND);
                GdipSetPenLineJoin(pen, LINE_JOIN_ROUND);
                let at = |x: f32, y: f32| PointF {
                    x: cx + x * h,
                    y: cy + y * h,
                };
                let lines = |points: &[PointF]| {
                    GdipDrawLines(g, pen, points.as_ptr(), points.len() as i32);
                };
                lines(&[at(0.0, -0.95), at(0.0, 0.25)]);
                lines(&[at(-0.45, -0.20), at(0.0, 0.25), at(0.45, -0.20)]);
                lines(&[at(-0.9, 0.2), at(-0.9, 0.9), at(0.9, 0.9), at(0.9, 0.2)]);
                GdipDeletePen(pen);
            }
        });
    }

    /// A hollow circle, for a VS day still to come.
    fn ring(&self, cx: f32, cy: f32, r: f32) {
        let s = self.scale;
        let (cx, cy, r) = (cx * s, cy * s, r * s);
        self.graphics(|g| {
            // SAFETY: `g` is valid for this closure; the pen is deleted here.
            unsafe {
                let mut pen: HANDLE = std::ptr::null_mut();
                if GdipCreatePen1(argb(BORDER), 2.0 * s, UNIT_PIXEL, &mut pen) == 0 {
                    GdipDrawEllipse(g, pen, cx - r, cy - r, 2.0 * r, 2.0 * r);
                    GdipDeletePen(pen);
                }
            }
        });
    }

    /// The whole window, onto a DC the size of the client area.
    pub fn screen(&mut self, screen: &Screen) {
        self.fill_rect(0.0, 0.0, WIDTH, HEIGHT, WHITE);

        // Header: the big mark, the headline, and the connection with its dot.
        let (bx, by) = (MARGIN + 6.0, HEADER_Y);
        self.mark(
            bx + BADGE / 2.0,
            by + BADGE / 2.0,
            BADGE / 2.0,
            screen.mark,
            true,
        );
        let tx = bx + BADGE + 18.0;
        let tw = WIDTH - MARGIN - tx;
        self.text(
            &screen.headline,
            (tx, by - 2.0, tw, 42.0),
            Font::Headline,
            TEXT,
            DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
        );
        let dot = match screen.connection_mark {
            Mark::Done => GREEN,
            Mark::Failed => RED,
            Mark::Action => AMBER,
            Mark::Pending | Mark::Download | Mark::Off => GREY,
        };
        let cy = by + 52.0;
        self.graphics(|g| {
            let s = self.scale;
            // SAFETY: `g` is valid for this closure; the brush is deleted here.
            unsafe {
                let mut brush: HANDLE = std::ptr::null_mut();
                if GdipCreateSolidFill(argb(dot), &mut brush) == 0 {
                    GdipFillEllipse(g, brush, (tx + 1.0) * s, (cy - 5.0) * s, 10.0 * s, 10.0 * s);
                    GdipDeleteBrush(brush);
                }
            }
        });
        // Errors can be long: they wrap onto a second line.
        self.text(
            &screen.connection,
            (tx + 20.0, cy - 11.0, tw - 20.0, 40.0),
            Font::Body,
            TEXT_SOFT,
            DT_WORDBREAK | DT_END_ELLIPSIS,
        );

        // The rows, in a bordered card.
        let (cx, cw) = (MARGIN, WIDTH - 2.0 * MARGIN);
        self.rounded((cx, CARD_Y, cw, ROWS * ROW_H), 8.0, None, Some(BORDER));
        let rows = std::iter::once(&screen.sign_in).chain(&screen.rows);
        for (i, row) in rows.enumerate() {
            let y = CARD_Y + i as f32 * ROW_H;
            if i > 0 {
                self.fill_rect(cx + 1.0, y, cw - 2.0, 1.0, BORDER);
            }
            let label_w = 130.0;
            self.text(
                row.label,
                (cx + 20.0, y, label_w, ROW_H),
                Font::Label,
                TEXT,
                DT_SINGLELINE | DT_VCENTER,
            );
            let color = match row.mark {
                Mark::Done => GREEN_TEXT,
                Mark::Action => AMBER_TEXT,
                Mark::Failed => RED,
                Mark::Pending | Mark::Download | Mark::Off => TEXT_SOFT,
            };
            // The mark at the right edge, so every row's mark lines up; the text ends beside it.
            let right = cx + cw - 20.0;
            self.mark(right - 12.0, y + ROW_H / 2.0, 12.0, row.mark, false);
            let text_right = right - 24.0 - 10.0;
            let text_left = cx + 20.0 + label_w + 10.0;
            self.text(
                &row.text,
                (text_left, y, text_right - text_left, ROW_H),
                Font::Body,
                color,
                DT_SINGLELINE | DT_VCENTER | DT_RIGHT | DT_END_ELLIPSIS,
            );
        }

        // VS days.
        self.text(
            "VS scores",
            (MARGIN, VS_Y, 200.0, 24.0),
            Font::Label,
            TEXT,
            DT_SINGLELINE | DT_VCENTER,
        );
        if let Some(hint) = &screen.vs_hint {
            self.text(
                hint,
                (WIDTH / 2.0, VS_Y, WIDTH / 2.0 - MARGIN, 24.0),
                Font::Small,
                AMBER_TEXT,
                DT_SINGLELINE | DT_VCENTER | DT_RIGHT | DT_END_ELLIPSIS,
            );
        }
        let tile_w = (cw - 5.0 * TILE_GAP) / 6.0;
        for (d, day) in screen.vs_days.iter().enumerate() {
            let x = MARGIN + d as f32 * (tile_w + TILE_GAP);
            self.rounded((x, TILES_Y, tile_w, TILE_H), 6.0, None, Some(BORDER));
            self.text(
                DAY_NAMES[d],
                (x, TILES_Y + 10.0, tile_w, 24.0),
                Font::Body,
                TEXT,
                DT_SINGLELINE | DT_VCENTER | DT_CENTER,
            );
            let (mx, my) = (x + tile_w / 2.0, TILES_Y + 54.0);
            match day {
                Day::Loaded => self.mark(mx, my, 13.0, Mark::Done, false),
                Day::Today => self.mark(mx, my, 13.0, Mark::Pending, true),
                Day::Missing => self.mark(mx, my, 13.0, Mark::Action, false),
                Day::Later => self.ring(mx, my, 12.0),
            }
        }

        // Footer: the version or an update, beside the update button (a child window).
        self.fill_rect(MARGIN, DIVIDER_Y, cw, 1.0, BORDER);
        let mut x = MARGIN;
        let my = FOOTER_Y + FOOTER_H / 2.0;
        match screen.footer.mark {
            Some(Mark::Download) => {
                self.download_icon(x + 12.0, my, 22.0);
                x += 38.0;
            }
            Some(mark) => {
                self.mark(x + 12.0, my, 12.0, mark, false);
                x += 38.0;
            }
            None => {}
        }
        let (color, font) = match screen.footer.mark {
            None => (TEXT_SOFT, Font::Small),
            Some(Mark::Failed) => (RED, Font::Body),
            Some(_) => (TEXT, Font::Body),
        };
        let right = if screen.footer.button.is_some() {
            BUTTON.0 - 12.0
        } else {
            WIDTH - MARGIN
        };
        self.text(
            &screen.footer.text,
            (x, FOOTER_Y, right - x, FOOTER_H),
            font,
            color,
            DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
        );
    }

    /// The update button, solid blue, filling the DC's `rect` (device pixels).
    pub fn button(&mut self, rect: RECT, label: &str, state: ButtonState) {
        let s = self.scale;
        let (x, y) = (rect.left as f32 / s, rect.top as f32 / s);
        let (w, h) = (
            (rect.right - rect.left) as f32 / s,
            (rect.bottom - rect.top) as f32 / s,
        );
        self.fill_rect(x, y, w, h, WHITE);
        let fill = match (state.enabled, state.pressed) {
            (false, _) => BLUE_DISABLED,
            (true, true) => BLUE_PRESSED,
            (true, false) => BLUE,
        };
        self.rounded((x, y, w, h), 5.0, Some(fill), None);
        if state.focus {
            self.rounded(
                (x + 3.0, y + 3.0, w - 6.0, h - 6.0),
                3.0,
                None,
                Some(0xBFD8F8),
            );
        }
        self.text(
            label,
            (x, y, w, h),
            Font::Button,
            WHITE,
            DT_SINGLELINE | DT_VCENTER | DT_CENTER,
        );
    }
}

/// The app's icon at `size` pixels: a blue scan frame, four corners and a line across.
pub fn app_icon(size: i32) -> HANDLE {
    // SAFETY: the bitmap and its graphics are made, drawn on and released here; the icon is
    // a new handle the caller keeps for the process's life.
    unsafe {
        let mut bitmap: HANDLE = std::ptr::null_mut();
        if GdipCreateBitmapFromScan0(
            size,
            size,
            0,
            PIXEL_FORMAT_32BPP_ARGB,
            std::ptr::null_mut(),
            &mut bitmap,
        ) != 0
        {
            return std::ptr::null_mut();
        }
        let mut g: HANDLE = std::ptr::null_mut();
        if GdipGetImageGraphicsContext(bitmap, &mut g) == 0 {
            GdipSetSmoothingMode(g, SMOOTHING_ANTIALIAS);
            GdipSetPixelOffsetMode(g, PIXEL_OFFSET_HALF);
            let n = size as f32;
            let mut pen: HANDLE = std::ptr::null_mut();
            if GdipCreatePen1(argb(BLUE), (n * 0.11).max(1.5), UNIT_PIXEL, &mut pen) == 0 {
                GdipSetPenStartCap(pen, LINE_CAP_ROUND);
                GdipSetPenEndCap(pen, LINE_CAP_ROUND);
                GdipSetPenLineJoin(pen, LINE_JOIN_ROUND);
                let p = |x: f32, y: f32| PointF { x: x * n, y: y * n };
                let (a, b, l) = (0.12, 0.88, 0.30);
                for corner in [
                    [p(a, a + l), p(a, a), p(a + l, a)],
                    [p(b - l, a), p(b, a), p(b, a + l)],
                    [p(b, b - l), p(b, b), p(b - l, b)],
                    [p(a + l, b), p(a, b), p(a, b - l)],
                ] {
                    GdipDrawLines(g, pen, corner.as_ptr(), 3);
                }
                let line = [p(0.30, 0.5), p(0.70, 0.5)];
                GdipDrawLines(g, pen, line.as_ptr(), 2);
                GdipDeletePen(pen);
            }
            GdipDeleteGraphics(g);
        }
        let mut icon: HANDLE = std::ptr::null_mut();
        GdipCreateHICONFromBitmap(bitmap, &mut icon);
        GdipDisposeImage(bitmap);
        icon
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_are_found_by_position() {
        let x = WIDTH / 2.0;
        assert_eq!(row_at(x, CARD_Y), Some(0));
        assert_eq!(row_at(x, CARD_Y + ROW_H - 1.0), Some(0));
        assert_eq!(row_at(x, CARD_Y + ROW_H), Some(1));
        assert_eq!(
            row_at(x, CARD_Y + ROWS * ROW_H - 1.0),
            Some(ROWS as usize - 1)
        );
        assert_eq!(row_at(x, CARD_Y + ROWS * ROW_H), None);
        assert_eq!(row_at(x, CARD_Y - 1.0), None);
        assert_eq!(row_at(MARGIN - 1.0, CARD_Y + 1.0), None);
        assert_eq!(row_at(WIDTH - MARGIN, CARD_Y + 1.0), None);
    }
}
