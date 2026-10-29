//! Windows direct printing (Q-P19ae): `cl-print`'s resolved layout drawn
//! straight onto the chosen printer through GDI, so printing does not
//! depend on a PDF app (Windows 11's default, Edge, has no print verb).
//!
//! **The second `unsafe` boundary in `ClientLedger`'s own crates** (the
//! first is `procs/win_job_memory.rs`). Every Windows call is in its own
//! small `unsafe` block with a `// SAFETY:` comment; handles are owned by
//! guards that release them once (`DeleteDC`, `DeleteObject`,
//! `RemoveFontMemResourceEx`), and every pointer passed is to memory that
//! outlives the call. Nothing else in this module, and nothing outside it
//! (other than the job module), may use `unsafe`: cl-host sets
//! `unsafe_code = "deny"`, this module alone says `#![allow(unsafe_code)]`,
//! and `cargo xtask unsafe-check` enforces both and the comments.
//!
//! What is drawn matches the PDF: the same embedded IBM Plex faces (loaded
//! privately with `AddFontMemResourceEx`, so nothing is installed), glyphs
//! placed one by one with `ExtTextOutW` spacing computed from the layout's
//! own advances, and millimetres mapped through [`DeviceMap`] from the
//! physical page edge (`PHYSICALOFFSETX/Y`), so field positions hold within
//! ±0.5 mm. Copies are printed collated inside one job. Any failure aborts
//! the job and returns an error; the caller falls back to the PDF hand-off.
#![allow(unsafe_code)]

use crate::os_print::{DeviceMap, DirectPrintError};
use cl_print::image::RgbImage;
use cl_print::layout::Shape;
use cl_print::{Document, Font, ResolvedPage};
use std::collections::HashMap;
use std::path::Path;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{HANDLE, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    AddFontMemResourceEx, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CLIP_DEFAULT_PRECIS, CreateDCW,
    CreateFontIndirectW, CreatePen, CreateSolidBrush, DEFAULT_CHARSET, DEFAULT_QUALITY,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, ExtTextOutW, FillRect, GDI_ERROR, GetDeviceCaps,
    GetStockObject, HDC, HGDIOBJ, LOGFONTW, LOGPIXELSX, LOGPIXELSY, LineTo, MoveToEx, NULL_BRUSH,
    OUT_TT_ONLY_PRECIS, PHYSICALOFFSETX, PHYSICALOFFSETY, PS_SOLID, Rectangle,
    RemoveFontMemResourceEx, SRCCOPY, SelectObject, SetBkMode, SetTextAlign, SetTextColor,
    StretchDIBits, TA_BASELINE, TA_LEFT, TRANSPARENT,
};
use windows_sys::Win32::Graphics::Printing::GetDefaultPrinterW;
use windows_sys::Win32::Storage::Xps::{AbortDoc, DOCINFOW, EndDoc, EndPage, StartDocW, StartPage};

type R<T> = Result<T, DirectPrintError>;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The default printer's name (`GetDefaultPrinterW`).
pub(crate) fn default_printer() -> Option<String> {
    let mut len: u32 = 0;
    // SAFETY: a null buffer with a zero length asks only for the size;
    // `len` is a valid, writable u32 on this stack frame.
    unsafe { GetDefaultPrinterW(null_mut(), &raw mut len) };
    if len == 0 || len > 4096 {
        return None;
    }
    let mut buf = vec![0u16; usize::try_from(len).ok()?];
    // SAFETY: `buf` holds `len` u16 slots and outlives the call; `len` says
    // so and is writable.
    let ok = unsafe { GetDefaultPrinterW(buf.as_mut_ptr(), &raw mut len) };
    if ok == 0 {
        return None;
    }
    let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    String::from_utf16(&buf[..end])
        .ok()
        .filter(|s| !s.is_empty())
}

/// A printer device context, deleted once.
struct Dc(HDC);

impl Drop for Dc {
    fn drop(&mut self) {
        // SAFETY: `self.0` came from `CreateDCW`, is not null, and is
        // deleted only here.
        unsafe { DeleteDC(self.0) };
    }
}

/// The embedded Plex faces, loaded privately for this process.
struct PrivateFonts(Vec<HANDLE>);

impl PrivateFonts {
    fn load() -> R<PrivateFonts> {
        let mut out = PrivateFonts(Vec::new());
        for f in Font::ALL {
            let bytes = f.bytes();
            let size = u32::try_from(bytes.len()).map_err(|_| DirectPrintError::FontLoad)?;
            let mut count: u32 = 0;
            // SAFETY: `bytes` is a `'static` font file of `size` bytes, read
            // during the call (GDI copies it); `count` is a writable u32.
            let h = unsafe {
                AddFontMemResourceEx(bytes.as_ptr().cast(), size, null(), &raw mut count)
            };
            if h.is_null() || count == 0 {
                return Err(DirectPrintError::FontLoad);
            }
            out.0.push(h);
        }
        Ok(out)
    }
}

impl Drop for PrivateFonts {
    fn drop(&mut self) {
        for h in &self.0 {
            // SAFETY: each handle came from `AddFontMemResourceEx` and is
            // removed only here.
            unsafe { RemoveFontMemResourceEx(*h) };
        }
    }
}

/// A GDI object (font, pen, brush), deleted once after being deselected.
struct Obj(HGDIOBJ);

impl Drop for Obj {
    fn drop(&mut self) {
        // SAFETY: `self.0` came from a GDI create call, is not null, is no
        // longer selected into a DC (callers restore the previous object),
        // and is deleted only here.
        unsafe { DeleteObject(self.0) };
    }
}

fn obj(h: HGDIOBJ, step: &'static str) -> R<Obj> {
    if h.is_null() {
        Err(DirectPrintError::Gdi(step))
    } else {
        Ok(Obj(h))
    }
}

/// Select `h` into `dc`, returning what was selected before.
fn select(dc: &Dc, h: HGDIOBJ) -> HGDIOBJ {
    // SAFETY: `dc` is a live DC and `h` a live GDI object or stock object.
    unsafe { SelectObject(dc.0, h) }
}

fn device_map(dc: &Dc) -> DeviceMap {
    let cap = |i: u32| {
        let index = i32::try_from(i).unwrap_or(0);
        // SAFETY: `dc` is a live printer DC; GetDeviceCaps only reads it.
        unsafe { GetDeviceCaps(dc.0, index) }
    };
    DeviceMap {
        dpi_x: cap(LOGPIXELSX).max(1),
        dpi_y: cap(LOGPIXELSY).max(1),
        offset_x: cap(PHYSICALOFFSETX),
        offset_y: cap(PHYSICALOFFSETY),
    }
}

/// `COLORREF` for a grey level (0 = black, 1 = white).
fn gray(level: f32) -> u32 {
    // The nearest of the 256 levels.
    let v = (0u8..=255)
        .min_by(|a, b| {
            let da = (f32::from(*a) / 255.0 - level).abs();
            let db = (f32::from(*b) / 255.0 - level).abs();
            da.total_cmp(&db)
        })
        .map_or(0, u32::from);
    v | (v << 8) | (v << 16)
}

/// Print `doc` on `printer` (default printer when `None`), `copies` times.
/// `output` sends the job to a file (only for the opt-in test with a
/// file-writing printer such as "Microsoft Print to PDF").
pub(crate) fn print(
    doc: &Document,
    printer: Option<&str>,
    copies: u16,
    output: Option<&Path>,
) -> R<()> {
    let pages = cl_print::resolve(doc).map_err(DirectPrintError::Layout)?;
    let name = match printer {
        Some(p) => p.to_string(),
        None => default_printer().ok_or(DirectPrintError::NoPrinter)?,
    };
    let fonts = PrivateFonts::load()?;
    let driver = wide("WINSPOOL");
    let device = wide(&name);
    // SAFETY: `driver` and `device` are NUL-terminated UTF-16 strings that
    // outlive the call; no port or DEVMODE (the printer's defaults).
    let h = unsafe { CreateDCW(driver.as_ptr(), device.as_ptr(), null(), null()) };
    if h.is_null() {
        return Err(DirectPrintError::OpenPrinter);
    }
    let dc = Dc(h);
    let map = device_map(&dc);
    // The logo, if it can be decoded; a logo that can't is left out
    // (Q-P19af), never failing the print.
    let logo = doc.logo.as_ref().and_then(|l| l.rgb_on_white().ok());

    let title = wide(if doc.title.is_empty() {
        "ClientLedger"
    } else {
        &doc.title
    });
    let out = output.map(|p| wide(&p.to_string_lossy()));
    let info = DOCINFOW {
        cbSize: i32::try_from(std::mem::size_of::<DOCINFOW>()).unwrap_or(0),
        lpszDocName: title.as_ptr(),
        lpszOutput: out.as_ref().map_or(null(), Vec::as_ptr),
        lpszDatatype: null(),
        fwType: 0,
    };
    // SAFETY: `info` and the strings it points to live on this frame for
    // the whole call; `dc` is a live printer DC.
    if unsafe { StartDocW(dc.0, &raw const info) } <= 0 {
        return Err(DirectPrintError::Gdi("StartDoc"));
    }
    let mut cache = HashMap::new();
    let result = (|| {
        for _ in 0..copies.max(1) {
            for page in &pages {
                // SAFETY: `dc` is a live DC inside a started document.
                if unsafe { StartPage(dc.0) } <= 0 {
                    return Err(DirectPrintError::Gdi("StartPage"));
                }
                draw_page(&dc, map, page, logo.as_ref(), &mut cache)?;
                // SAFETY: as above, with a page started.
                if unsafe { EndPage(dc.0) } <= 0 {
                    return Err(DirectPrintError::Gdi("EndPage"));
                }
            }
        }
        Ok(())
    })();
    match result {
        Ok(()) => {
            // SAFETY: `dc` is a live DC with a started document.
            if unsafe { EndDoc(dc.0) } <= 0 {
                return Err(DirectPrintError::Gdi("EndDoc"));
            }
        }
        Err(e) => {
            // SAFETY: `dc` is a live DC with a started document.
            unsafe { AbortDoc(dc.0) };
            return Err(e);
        }
    }
    // Fonts made for the job go before the DC and the private fonts.
    drop(cache);
    drop(dc);
    drop(fonts);
    Ok(())
}

/// Fonts by face and size (in hundredths of a point).
type FontCache = HashMap<(Font, u32), Obj>;

fn font_for(cache: &mut FontCache, map: DeviceMap, font: Font, size_pt: f32) -> R<HGDIOBJ> {
    let mut key_size = 0u32;
    while f32::from(u16::try_from(key_size).unwrap_or(u16::MAX)) < size_pt * 100.0
        && key_size < 60_000
    {
        key_size += 1;
    }
    if let Some(o) = cache.get(&(font, key_size)) {
        return Ok(o.0);
    }
    let mut lf = LOGFONTW {
        lfHeight: map.font_height(size_pt),
        lfWeight: i32::from(font.weight()),
        lfCharSet: DEFAULT_CHARSET,
        lfOutPrecision: OUT_TT_ONLY_PRECIS,
        lfClipPrecision: CLIP_DEFAULT_PRECIS,
        lfQuality: DEFAULT_QUALITY,
        ..LOGFONTW::default()
    };
    for (slot, c) in lf
        .lfFaceName
        .iter_mut()
        .zip(font.family_name().encode_utf16().take(31))
    {
        *slot = c;
    }
    // SAFETY: `lf` is a fully initialised LOGFONTW on this frame, read
    // during the call.
    let h = unsafe { CreateFontIndirectW(&raw const lf) };
    let o = obj(h, "CreateFont")?;
    let raw = o.0;
    cache.insert((font, key_size), o);
    Ok(raw)
}

fn draw_page(
    dc: &Dc,
    map: DeviceMap,
    page: &ResolvedPage,
    logo: Option<&RgbImage>,
    cache: &mut FontCache,
) -> R<()> {
    // SAFETY: `dc` is a live DC; these set drawing state only.
    unsafe {
        SetBkMode(dc.0, i32::try_from(TRANSPARENT).unwrap_or(1));
        SetTextAlign(dc.0, TA_BASELINE | TA_LEFT);
        SetTextColor(dc.0, 0);
    }
    for s in &page.shapes {
        draw_shape(dc, map, s)?;
    }
    if let Some(img) = logo {
        for l in &page.logos {
            draw_logo(dc, map, l.rect, img);
        }
    }
    for run in &page.runs {
        let f = font_for(cache, map, run.font, run.size_pt)?;
        let before = select(dc, f);
        let text: Vec<u16> = run.text.encode_utf16().collect();
        let adv: Vec<f32> = run
            .text
            .chars()
            .map(|c| run.font.advance_mm(c, run.size_pt))
            .collect();
        let dx = map.glyph_dx(&run.text, run.x, &adv);
        let n = u32::try_from(text.len()).map_err(|_| DirectPrintError::Gdi("ExtTextOut"))?;
        // SAFETY: `text` and `dx` both hold `n` entries (one spacing per
        // UTF-16 unit) and outlive the call; no clip rectangle.
        let ok = unsafe {
            ExtTextOutW(
                dc.0,
                map.x(run.x),
                map.y(run.baseline),
                0,
                null(),
                text.as_ptr(),
                n,
                dx.as_ptr(),
            )
        };
        select(dc, before);
        if ok == 0 {
            return Err(DirectPrintError::Gdi("ExtTextOut"));
        }
    }
    Ok(())
}

fn draw_shape(dc: &Dc, map: DeviceMap, s: &Shape) -> R<()> {
    match s {
        Shape::Line {
            x1,
            y1,
            x2,
            y2,
            width_pt,
        } => {
            // SAFETY: plain value arguments; returns a new pen or null.
            let raw = unsafe { CreatePen(PS_SOLID, map.pen_width(*width_pt), 0) };
            let pen = obj(raw, "CreatePen")?;
            let before = select(dc, pen.0);
            // SAFETY: `dc` is live; a null previous-point pointer is allowed.
            unsafe {
                MoveToEx(dc.0, map.x(*x1), map.y(*y1), null_mut());
                LineTo(dc.0, map.x(*x2), map.y(*y2));
            }
            select(dc, before);
        }
        Shape::Box {
            rect,
            stroke_pt,
            fill_gray,
        } => {
            let r = RECT {
                left: map.x(rect.x),
                top: map.y(rect.y),
                right: map.x(rect.right()),
                bottom: map.y(rect.bottom()),
            };
            if let Some(g) = fill_gray {
                // SAFETY: plain value argument; returns a new brush or null.
                let brush = obj(unsafe { CreateSolidBrush(gray(*g)) }, "CreateBrush")?;
                // SAFETY: `r` lives on this frame; `brush` is a live brush.
                unsafe { FillRect(dc.0, &raw const r, brush.0) };
            }
            if let Some(w) = stroke_pt {
                // SAFETY: plain value arguments; returns a new pen or null.
                let raw = unsafe { CreatePen(PS_SOLID, map.pen_width(*w), 0) };
                let pen = obj(raw, "CreatePen")?;
                // SAFETY: stock objects are owned by the system and never
                // deleted by us.
                let hollow = unsafe { GetStockObject(NULL_BRUSH) };
                let before_pen = select(dc, pen.0);
                let before_brush = select(dc, hollow);
                // SAFETY: `dc` is live; plain value arguments.
                unsafe { Rectangle(dc.0, r.left, r.top, r.right, r.bottom) };
                select(dc, before_brush);
                select(dc, before_pen);
            }
        }
    }
    Ok(())
}

/// Draw the logo; failures leave it out (Q-P19af).
fn draw_logo(dc: &Dc, map: DeviceMap, rect: cl_print::Rect, img: &RgbImage) {
    let (Ok(w), Ok(h)) = (usize::try_from(img.width), usize::try_from(img.height)) else {
        return;
    };
    let stride = (w * 3).div_ceil(4) * 4;
    let mut bits = vec![255u8; stride * h];
    for row in 0..h {
        for col in 0..w {
            let s = (row * w + col) * 3;
            let d = row * stride + col * 3;
            if let (Some(src), Some(dst)) = (img.rgb.get(s..s + 3), bits.get_mut(d..d + 3)) {
                dst[0] = src[2];
                dst[1] = src[1];
                dst[2] = src[0];
            }
        }
    }
    let (Ok(wi), Ok(hi)) = (i32::try_from(img.width), i32::try_from(img.height)) else {
        return;
    };
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: u32::try_from(std::mem::size_of::<BITMAPINFOHEADER>()).unwrap_or(40),
            biWidth: wi,
            biHeight: -hi, // top-down rows
            biPlanes: 1,
            biBitCount: 24,
            biCompression: BI_RGB,
            ..BITMAPINFOHEADER::default()
        },
        ..BITMAPINFO::default()
    };
    // SAFETY: `bits` holds `stride * h` bytes (24-bit rows padded to four
    // bytes, top-down as `biHeight < 0` says) and `info` describes exactly
    // that; both outlive the call.
    let done = unsafe {
        StretchDIBits(
            dc.0,
            map.x(rect.x),
            map.y(rect.y),
            map.len_x(rect.w),
            map.len_y(rect.h),
            0,
            0,
            wi,
            hi,
            bits.as_ptr().cast(),
            &raw const info,
            DIB_RGB_COLORS,
            SRCCOPY,
        )
    };
    if done == 0 || done == GDI_ERROR {
        tracing::warn!("the logo could not be drawn; printing without it");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grey_levels_map_to_colorrefs() {
        assert_eq!(gray(0.0), 0);
        assert_eq!(gray(1.0), 0x00FF_FFFF);
        assert_eq!(gray(0.92) & 0xFF, 235);
    }

    /// Opt-in only: set `CL_PRINT_TEST_PRINTER` (for example
    /// `Microsoft Print to PDF`) and this prints a check sheet to a file in
    /// the temp folder. Never runs by default, and never on paper unless
    /// the named printer is a real one.
    #[test]
    fn prints_to_a_file_printer_when_opted_in() {
        let Ok(printer) = std::env::var("CL_PRINT_TEST_PRINTER") else {
            return;
        };
        let check = cl_print::CheckData {
            number: "1001".into(),
            date: "10/29/2026".into(),
            payee: "Acme Supply Co.".into(),
            amount: rust_decimal::Decimal::new(123_456, 2),
            address: vec!["Acme Supply Co.".into(), "100 Industrial Way".into()],
            memo: "Test".into(),
            stub: cl_print::StubData::default(),
        };
        let opts = cl_print::CheckOptions {
            style: cl_print::CheckStyle::Standard,
            ..cl_print::CheckOptions::default()
        };
        let doc = cl_print::layout_checks(&[check], &opts).unwrap();
        let out = std::env::temp_dir().join(format!("cl-direct-print-{}.pdf", std::process::id()));
        print(&doc, Some(&printer), 1, Some(&out)).unwrap();
        let bytes = std::fs::read(&out).unwrap();
        assert!(bytes.starts_with(b"%PDF"));
        let _ = std::fs::remove_file(&out);
    }
}
