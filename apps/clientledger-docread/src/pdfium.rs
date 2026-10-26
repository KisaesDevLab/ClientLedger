//! `PDFium` in the worker (plan D2): the only C++ parser of a hostile file,
//! inside this process and its limits. Text layer words through
//! `cl_docread::pdf_text`; pages without a usable text layer are rendered
//! to grey at 300 dpi (within the OCR limit) for tesseract.
//!
//! The library is looked for, in order, at `CL_PDFIUM_PATH` (absolute; the
//! library or its folder; `cl-host` passes its own on), `<exe_dir>/pdfium/`,
//! and in debug builds only `target/pdfium/<triple>/` of the source
//! checkout (`cargo xtask pdfium-fetch`). Never the working folder or the
//! system path.

use cl_docread::model::Word;
use cl_docread::pdf_text;
use cl_docread::protocol::{MAX_PGM_HEIGHT, MAX_PGM_WIDTH};
use image::GrayImage;
use pdfium_render::prelude::{
    PdfDocument, PdfPage, PdfRenderConfig, Pdfium, PdfiumError, PdfiumInternalError,
};
use std::path::{Path, PathBuf};

/// Resolution pages are rendered at for OCR (dots per inch).
pub const OCR_DPI: f32 = 300.0;

/// Where to look for the library, in order (see the module comment).
#[must_use]
pub fn candidates(
    env: Option<PathBuf>,
    exe_dir: Option<&Path>,
    dev: Option<PathBuf>,
) -> Vec<PathBuf> {
    let name = Pdfium::pdfium_platform_library_name();
    let mut out = Vec::new();
    if let Some(p) = env.filter(|p| p.is_absolute()) {
        if p.is_dir() {
            out.push(p.join(&name));
        } else {
            out.push(p);
        }
    }
    if let Some(d) = exe_dir {
        out.push(d.join("pdfium").join(&name));
    }
    if let Some(d) = dev {
        out.push(d.join(&name));
    }
    out
}

/// `target/pdfium/<triple>/` of the checkout this worker was built from.
#[cfg(debug_assertions)]
fn dev_dir() -> Option<PathBuf> {
    let triple = if cfg!(all(windows, target_arch = "x86_64")) {
        "x86_64-pc-windows-msvc"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "aarch64-apple-darwin"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "x86_64-apple-darwin"
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "x86_64-unknown-linux-gnu"
    } else {
        return None;
    };
    Some(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/pdfium")
            .join(triple),
    )
}

#[cfg(not(debug_assertions))]
fn dev_dir() -> Option<PathBuf> {
    None
}

/// Bind the library, or `None` when it is not on this computer (PDFs are
/// then not read; images still are).
#[must_use]
pub fn load() -> Option<Pdfium> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf));
    let env = std::env::var_os("CL_PDFIUM_PATH").map(PathBuf::from);
    candidates(env, exe_dir.as_deref(), dev_dir())
        .into_iter()
        .filter(|c| c.is_file())
        .find_map(|c| match Pdfium::bind_to_library(&c) {
            Ok(b) => Some(Pdfium::new(b)),
            // Bindings are process-wide and only tests bind more than once.
            // `Pdfium::default` would look in the working folder first, so
            // the worker itself never calls it.
            #[cfg(test)]
            Err(PdfiumError::PdfiumLibraryBindingsAlreadyInitialized) => Some(Pdfium::default()),
            Err(_) => None,
        })
}

/// A PDF opened from bytes.
pub enum Opened<'a> {
    Doc(Box<PdfDocument<'a>>),
    /// Password-protected.
    Protected,
    /// Damaged or not a PDF `PDFium` reads.
    Unreadable,
}

/// Open `bytes` (no password).
#[must_use]
pub fn open<'a>(pdfium: &'a Pdfium, bytes: &'a [u8]) -> Opened<'a> {
    match pdfium.load_pdf_from_byte_slice(bytes, None) {
        Ok(d) => Opened::Doc(Box::new(d)),
        Err(PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError)) => {
            Opened::Protected
        }
        Err(_) => Opened::Unreadable,
    }
}

/// Pages in the document (0 when the count can't be had).
#[must_use]
pub fn page_count(doc: &PdfDocument<'_>) -> u16 {
    u16::try_from(doc.pages().len()).unwrap_or(u16::MAX)
}

/// The displayed page's height ÷ width × 10000 (letter portrait 12941).
#[must_use]
pub fn aspect(page: &PdfPage<'_>) -> u32 {
    aspect_of(page.width().value, page.height().value)
}

/// Height ÷ width × 10000, 0 for a page without a usable size.
#[must_use]
pub fn aspect_of(width: f32, height: f32) -> u32 {
    if !(width.is_finite() && height.is_finite()) || width <= 0.0 || height <= 0.0 {
        return 0;
    }
    let a = (f64::from(height) / f64::from(width) * 10_000.0).round();
    // Clamped to a sane page shape first.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let a = a.clamp(0.0, 1_000_000.0) as u32;
    a
}

/// The text layer's words and its quality score.
#[must_use]
pub fn text_layer(page: &PdfPage<'_>) -> (Vec<Word>, u16) {
    let words = pdf_text::page_words(page);
    let q = pdf_text::text_quality(&words);
    (words, q)
}

/// The pixel size to render a `width` × `height` point page at
/// [`OCR_DPI`], within the OCR limit, aspect kept. `None` for a page
/// without a usable size.
#[must_use]
pub fn ocr_size(width: f32, height: f32) -> Option<(u32, u32)> {
    if !(width.is_finite() && height.is_finite()) || width <= 0.0 || height <= 0.0 {
        return None;
    }
    let (w, h) = (f64::from(width), f64::from(height));
    let dpi = f64::from(OCR_DPI) / 72.0;
    let fit = (f64::from(MAX_PGM_WIDTH) / (w * dpi))
        .min(f64::from(MAX_PGM_HEIGHT) / (h * dpi))
        .min(1.0);
    let s = dpi * fit;
    // Both are in 1..=4400 here.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let px = |v: f64| ((v * s).floor().max(1.0)) as u32;
    let (pw, ph) = (px(w).min(MAX_PGM_WIDTH), px(h).min(MAX_PGM_HEIGHT));
    Some((pw, ph))
}

/// Render the displayed page to grey for OCR.
#[must_use]
pub fn render_grey(page: &PdfPage<'_>) -> Option<GrayImage> {
    let (w, h) = ocr_size(page.width().value, page.height().value)?;
    let config = PdfRenderConfig::new()
        .set_target_size(i32::try_from(w).ok()?, i32::try_from(h).ok()?)
        .render_form_data(true);
    let bitmap = page.render_with_config(&config).ok()?;
    let (bw, bh) = (
        u32::try_from(bitmap.width()).ok()?,
        u32::try_from(bitmap.height()).ok()?,
    );
    let rgba = bitmap.as_rgba_bytes();
    crate::decode::grey_from_rgba(bw, bh, &rgba)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::test_pdf::{Page, build, text};

    /// `PDFium` when it is here; `None` (test skipped) when it is not, unless
    /// `CL_PDFIUM_REQUIRE=1` (CI) makes that a failure.
    pub(crate) fn pdfium() -> Option<Pdfium> {
        let p = load();
        if p.is_none() {
            assert!(
                std::env::var("CL_PDFIUM_REQUIRE").as_deref() != Ok("1"),
                "PDFium is required (CL_PDFIUM_REQUIRE=1) but was not found"
            );
            eprintln!("PDFium not found: skipped (cargo xtask pdfium-fetch)");
        }
        p
    }

    fn first_page_words(pdfium: &Pdfium, bytes: &[u8]) -> (Vec<Word>, u16, u32) {
        let Opened::Doc(doc) = open(pdfium, bytes) else {
            panic!("the PDF opens");
        };
        let page = doc.pages().get(0).unwrap();
        let (w, q) = text_layer(&page);
        (w, q, aspect(&page))
    }

    fn find<'a>(words: &'a [Word], text: &str) -> &'a Word {
        words
            .iter()
            .find(|w| w.text == text)
            .unwrap_or_else(|| panic!("no word {text}"))
    }

    #[test]
    fn text_layer_words_land_where_the_page_shows_them() {
        let Some(p) = pdfium() else { return };
        let pdf = build(&[Page::letter(
            text(72, 700, 28, "Invoice INV-1001") + &text(400, 100, 12, "Total 544.82"),
        )]);
        let (words, quality, aspect) = first_page_words(&p, &pdf);
        let names: Vec<&str> = words.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(names, ["Invoice", "INV-1001", "Total", "544.82"]);
        assert_eq!(aspect, 12_941);
        // Four words: below the full-page count, so scaled down.
        assert!(quality < MIN_QUALITY_FOR_FEW_WORDS, "{quality}");
        let inv = find(&words, "Invoice").bbox;
        // x = 72 pt of 612; baseline 700 pt of 792 (y up): near the top.
        assert!((1_150..=1_200).contains(&inv.left()), "{inv:?}");
        assert!((800..=1_000).contains(&inv.top()), "{inv:?}");
        assert!(inv.bottom() > inv.top() && inv.bottom() < 1_300, "{inv:?}");
        let total = find(&words, "544.82").bbox;
        assert!(total.top() > 8_500 && total.left() > 6_500, "{total:?}");
    }

    const MIN_QUALITY_FOR_FEW_WORDS: u16 = pdf_text::MIN_TEXT_QUALITY_BP;

    #[test]
    fn a_rotated_page_maps_words_to_the_turned_page() {
        let Some(p) = pdfium() else { return };
        // A landscape scan: the page is turned a quarter clockwise and its
        // text is set sideways so that it reads upright once turned.
        let mut page = Page::letter("BT /F1 28 Tf 0 1 -1 0 100 72 Tm (Turned page) Tj ET\n".into());
        page.rotate = 90;
        let (words, _, aspect) = first_page_words(&p, &build(&[page]));
        // Shown 792 wide × 612 tall; user (x, y) shows at (y, x) from the
        // top left: baseline x = 100 pt is 100 pt from the top, y = 72 pt
        // is 72 pt from the left.
        assert_eq!(aspect, 7_727);
        let names: Vec<&str> = words.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(names, ["Turned", "page"]);
        let w = find(&words, "Turned").bbox;
        assert!((880..=940).contains(&w.left()), "{w:?}");
        assert!((1_150..=1_450).contains(&w.top()), "{w:?}");
        assert!((1_550..=1_850).contains(&w.bottom()), "{w:?}");
        assert!(w.width() > w.height(), "the word reads across: {w:?}");
    }

    #[test]
    fn a_crop_box_moves_the_origin() {
        let Some(p) = pdfium() else { return };
        let mut page = Page::letter(text(172, 600, 20, "Cropped"));
        page.crop = Some([100, 100, 612, 792]);
        let (words, _, aspect) = first_page_words(&p, &build(&[page]));
        assert_eq!(aspect, 13_516);
        let w = find(&words, "Cropped").bbox;
        // (172 − 100) / 512 of the shown width.
        assert!((1_380..=1_430).contains(&w.left()), "{w:?}");
    }

    #[test]
    fn protected_damaged_and_full_pages() {
        let Some(p) = pdfium() else { return };
        let enc = std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/docs/encrypted.pdf"),
        )
        .unwrap();
        assert!(matches!(open(&p, &enc), Opened::Protected));
        assert!(matches!(open(&p, b"%PDF-1.4 nothing"), Opened::Unreadable));
        let body: String = (0..40)
            .map(|i| text(72, 760 - i * 18, 10, &format!("Line {i} Widget 3 x 4.00")))
            .collect();
        let pdf = build(&[Page::letter(body)]);
        let (words, quality, _) = first_page_words(&p, &pdf);
        assert_eq!(words.len(), 40 * 6);
        // Letters and digits score; the points in "4.00" do not.
        assert!((9_000..10_000).contains(&quality), "{quality}");
        let Opened::Doc(doc) = open(&p, &pdf) else {
            panic!()
        };
        assert_eq!(page_count(&doc), 1);
        let grey = render_grey(&doc.pages().get(0).unwrap()).unwrap();
        assert_eq!(grey.dimensions(), (2550, 3300));
        assert!(grey.pixels().any(|px| px.0[0] < 100), "text is drawn");
        assert!(grey.pixels().filter(|px| px.0[0] == 255).count() > 2550 * 3000);
    }

    #[test]
    fn the_library_is_never_searched_in_the_working_folder() {
        let exe = std::env::temp_dir().join("app");
        let list = candidates(Some(PathBuf::from("relative")), Some(&exe), None);
        assert_eq!(list.len(), 1, "a relative CL_PDFIUM_PATH is ignored");
        assert!(list[0].starts_with(exe.join("pdfium")));
    }

    #[test]
    fn pages_render_at_300_dpi_within_the_ocr_limit() {
        // Letter: 8.5 × 11 in.
        assert_eq!(ocr_size(612.0, 792.0), Some((2550, 3300)));
        // Legal fits; a longer page is scaled to fit the limit.
        assert_eq!(ocr_size(612.0, 1008.0), Some((2550, 4200)));
        let (w, h) = ocr_size(612.0, 1100.0).unwrap();
        assert!(w < 2550 && (4399..=MAX_PGM_HEIGHT).contains(&h), "{w}x{h}");
        // A huge or odd page.
        let (w, h) = ocr_size(14_400.0, 14_400.0).unwrap();
        assert!(w <= MAX_PGM_WIDTH && h <= MAX_PGM_HEIGHT);
        let (w, h) = ocr_size(1.0, 14_400.0).unwrap();
        assert!(w >= 1 && h <= MAX_PGM_HEIGHT);
        assert_eq!(ocr_size(0.0, 10.0), None);
        assert_eq!(ocr_size(f32::NAN, 10.0), None);
        assert_eq!(aspect_of(612.0, 792.0), 12_941);
        assert_eq!(aspect_of(0.0, 792.0), 0);
    }
}
