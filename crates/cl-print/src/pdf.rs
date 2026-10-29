//! The PDF renderer: resolved pages to PDF bytes on `pdf-writer`.
//!
//! Text is embedded IBM Plex, subset to the glyphs used (CID fonts,
//! Identity-H) with a `ToUnicode` map so the text can be searched and
//! copied. Each [`Run`] becomes one `BT … Tm … Tj ET` block at the run's
//! baseline origin, so the drawn position is the resolved position. Page
//! content streams are left uncompressed (they are small; tests read
//! them); embedded fonts and logo samples are deflated.

use crate::PrintError;
use crate::fonts::{Font, MM_PER_PT};
use crate::image::{ColorSpace, Logo, LogoData};
use crate::layout::{Document, ResolvedPage, Run, Shape, resolve};
use pdf_writer::types::{CidFontType, FontFlags, SystemInfo};
use pdf_writer::{Content, Filter, Finish, Name, Pdf, Rect as PdfRect, Ref, Str, TextStr};
use std::collections::BTreeMap;

/// Millimetres to points.
#[must_use]
pub fn pt(mm: f32) -> f32 {
    mm / MM_PER_PT
}

/// Resolve and render a document.
///
/// # Errors
/// [`PrintError::NoPages`], or [`PrintError::Font`] when an embedded font
/// cannot be subset.
pub fn render(doc: &Document) -> Result<Vec<u8>, PrintError> {
    let pages = resolve(doc)?;
    render_resolved(&pages, doc.logo.as_ref(), &doc.title)
}

struct Embedded {
    remap: subsetter::GlyphRemapper,
    /// char → glyph id in the original font.
    glyphs: BTreeMap<char, u16>,
}

/// Render pages that were already resolved.
///
/// # Errors
/// [`PrintError::NoPages`] or [`PrintError::Font`].
#[allow(clippy::too_many_lines)] // One linear PDF assembly.
pub fn render_resolved(
    pages: &[ResolvedPage],
    logo: Option<&Logo>,
    title: &str,
) -> Result<Vec<u8>, PrintError> {
    if pages.is_empty() {
        return Err(PrintError::NoPages);
    }
    let mut used: BTreeMap<Font, Embedded> = BTreeMap::new();
    for run in pages.iter().flat_map(|p| &p.runs) {
        let face = run.font.face()?;
        let e = used.entry(run.font).or_insert_with(|| Embedded {
            remap: subsetter::GlyphRemapper::new(),
            glyphs: BTreeMap::new(),
        });
        for c in run.text.chars() {
            let g = face.glyph(c);
            e.remap.remap(g);
            e.glyphs.insert(c, g);
        }
    }

    let mut pdf = Pdf::new();
    let mut next = 1;
    let mut alloc = || {
        let r = Ref::new(next);
        next += 1;
        r
    };
    let catalog = alloc();
    let tree = alloc();
    let info = alloc();

    let mut font_refs: BTreeMap<Font, (Ref, String)> = BTreeMap::new();
    for (i, (font, e)) in used.iter().enumerate() {
        let face = font.face()?;
        let upm = f32::from(face.units_per_em());
        let scale = |v: f32| v * 1000.0 / upm;
        let sub = subsetter::subset(font.bytes(), 0, &e.remap)
            .map_err(|err| PrintError::Font(format!("subset {}: {err}", font.ps_name())))?;
        let (type0, cid, desc, file, cmap) = (alloc(), alloc(), alloc(), alloc(), alloc());
        // A six-letter subset tag, distinct per font.
        let tag: String = (0..6u8)
            .map(|k| char::from(b'A' + (u8::try_from(i % 26).unwrap_or(0) + k * 3) % 26))
            .collect();
        let base = format!("{tag}+{}", font.ps_name());
        pdf.type0_font(type0)
            .base_font(Name(base.as_bytes()))
            .encoding_predefined(Name(b"Identity-H"))
            .descendant_font(cid)
            .to_unicode(cmap);
        let sys = SystemInfo {
            registry: Str(b"Adobe"),
            ordering: Str(b"Identity"),
            supplement: 0,
        };
        let widths: Vec<f32> = e
            .remap
            .remapped_gids()
            .map(|old| face.advance(old) * 1000.0 / upm)
            .collect();
        let mut c = pdf.cid_font(cid);
        c.subtype(CidFontType::Type2)
            .base_font(Name(base.as_bytes()))
            .system_info(sys)
            .font_descriptor(desc)
            .cid_to_gid_map_predefined(Name(b"Identity"));
        c.widths().consecutive(0, widths);
        c.finish();
        let [x0, y0, x1, y1] = face.bbox();
        let mut flags = FontFlags::NON_SYMBOLIC;
        if font.monospace() {
            flags |= FontFlags::FIXED_PITCH;
        }
        pdf.font_descriptor(desc)
            .name(Name(base.as_bytes()))
            .flags(flags)
            .bbox(PdfRect::new(scale(x0), scale(y0), scale(x1), scale(y1)))
            .italic_angle(0.0)
            .ascent(scale(face.ascender()))
            .descent(scale(face.descender()))
            .cap_height(scale(face.cap_height()))
            .stem_v(80.0)
            .font_file2(file);
        let compressed = deflate(&sub);
        pdf.stream(file, &compressed)
            .filter(Filter::FlateDecode)
            .pair(Name(b"Length1"), i32::try_from(sub.len()).unwrap_or(0));
        let mut u = pdf_writer::types::UnicodeCmap::new(Name(b"Custom"), sys);
        for (&ch, &old) in &e.glyphs {
            if let Some(new) = e.remap.get(old) {
                u.pair(new, ch);
            }
        }
        pdf.cmap(cmap, &u.finish()).system_info(sys);
        font_refs.insert(*font, (type0, format!("F{i}")));
    }

    // The logo, once, if any page draws it.
    let logo_ref = match logo {
        Some(l) if pages.iter().any(|p| !p.logos.is_empty()) => {
            let r = alloc();
            let mask = alloc();
            write_logo(&mut pdf, r, mask, l);
            Some(r)
        }
        _ => None,
    };

    let page_ids: Vec<(Ref, Ref)> = pages.iter().map(|_| (alloc(), alloc())).collect();
    pdf.catalog(catalog).pages(tree);
    pdf.pages(tree)
        .kids(page_ids.iter().map(|(p, _)| *p))
        .count(i32::try_from(pages.len()).unwrap_or(0));
    for (page, (pid, cid)) in pages.iter().zip(&page_ids) {
        let mut p = pdf.page(*pid);
        p.media_box(PdfRect::new(0.0, 0.0, pt(page.size.w), pt(page.size.h)))
            .parent(tree)
            .contents(*cid);
        let mut res = p.resources();
        let mut fs = res.fonts();
        for (r, name) in font_refs.values() {
            fs.pair(Name(name.as_bytes()), *r);
        }
        fs.finish();
        if let (Some(r), false) = (logo_ref, page.logos.is_empty()) {
            res.x_objects().pair(Name(b"Logo"), r);
        }
        res.finish();
        p.finish();
        let content = content(page, &used, &font_refs, logo_ref.is_some());
        pdf.stream(*cid, &content);
    }
    pdf.document_info(info)
        .producer(TextStr("ClientLedger"))
        .title(TextStr(title));
    Ok(pdf.finish())
}

fn write_logo(pdf: &mut Pdf, id: Ref, mask: Ref, logo: &Logo) {
    let w = i32::try_from(logo.width).unwrap_or(1);
    let h = i32::try_from(logo.height).unwrap_or(1);
    let space: &[u8] = match logo.color {
        ColorSpace::Gray => b"DeviceGray",
        ColorSpace::Rgb => b"DeviceRGB",
        ColorSpace::Cmyk => b"DeviceCMYK",
    };
    match &logo.data {
        LogoData::Jpeg(bytes) => {
            let mut img = pdf.image_xobject(id, bytes);
            img.filter(Filter::DctDecode);
            img.width(w)
                .height(h)
                .color_space_name(Name(space))
                .bits_per_component(8);
        }
        LogoData::Raw { samples, alpha } => {
            let data = deflate(samples);
            let mut img = pdf.image_xobject(id, &data);
            img.filter(Filter::FlateDecode);
            img.width(w)
                .height(h)
                .color_space_name(Name(space))
                .bits_per_component(8);
            if alpha.is_some() {
                img.s_mask(mask);
            }
            img.finish();
            if let Some(a) = alpha {
                let data = deflate(a);
                let mut sm = pdf.image_xobject(mask, &data);
                sm.filter(Filter::FlateDecode);
                sm.width(w)
                    .height(h)
                    .color_space_name(Name(b"DeviceGray"))
                    .bits_per_component(8);
            }
        }
    }
}

/// The page's drawing operators.
fn content(
    page: &ResolvedPage,
    used: &BTreeMap<Font, Embedded>,
    refs: &BTreeMap<Font, (Ref, String)>,
    has_logo: bool,
) -> Vec<u8> {
    let mut c = Content::new();
    let top = pt(page.size.h);
    for s in &page.shapes {
        match s {
            Shape::Line {
                x1,
                y1,
                x2,
                y2,
                width_pt,
            } => {
                c.set_line_width(*width_pt);
                c.move_to(pt(*x1), top - pt(*y1));
                c.line_to(pt(*x2), top - pt(*y2));
                c.stroke();
            }
            Shape::Box {
                rect,
                stroke_pt,
                fill_gray,
            } => {
                c.save_state();
                if let Some(g) = fill_gray {
                    c.set_fill_gray(*g);
                }
                if let Some(w) = stroke_pt {
                    c.set_line_width(*w);
                }
                c.rect(pt(rect.x), top - pt(rect.bottom()), pt(rect.w), pt(rect.h));
                match (fill_gray.is_some(), stroke_pt.is_some()) {
                    (true, true) => c.fill_nonzero_and_stroke(),
                    (true, false) => c.fill_nonzero(),
                    _ => c.stroke(),
                };
                c.restore_state();
            }
        }
    }
    if has_logo {
        for l in &page.logos {
            c.save_state();
            c.transform([
                pt(l.rect.w),
                0.0,
                0.0,
                pt(l.rect.h),
                pt(l.rect.x),
                top - pt(l.rect.bottom()),
            ]);
            c.x_object(Name(b"Logo"));
            c.restore_state();
        }
    }
    for run in &page.runs {
        draw_run(&mut c, run, used, refs, top);
    }
    c.finish().into_vec()
}

fn draw_run(
    c: &mut Content,
    run: &Run,
    used: &BTreeMap<Font, Embedded>,
    refs: &BTreeMap<Font, (Ref, String)>,
    top: f32,
) {
    let (Some(e), Some((_, name))) = (used.get(&run.font), refs.get(&run.font)) else {
        return;
    };
    let mut bytes = Vec::with_capacity(run.text.len() * 2);
    for ch in run.text.chars() {
        let old = e.glyphs.get(&ch).copied().unwrap_or(0);
        let new = e.remap.get(old).unwrap_or(0);
        bytes.extend_from_slice(&new.to_be_bytes());
    }
    c.begin_text();
    c.set_font(Name(name.as_bytes()), run.size_pt);
    c.set_text_matrix([1.0, 0.0, 0.0, 1.0, pt(run.x), top - pt(run.baseline)]);
    c.show(Str(&bytes));
    c.end_text();
}

fn deflate(data: &[u8]) -> Vec<u8> {
    use std::io::Write as _;
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    // Writing to a Vec cannot fail.
    let _ = e.write_all(data);
    e.finish().unwrap_or_default()
}

/// Test helpers: read text positions back out of a rendered PDF.
#[cfg(test)]
pub(crate) mod read_back {
    /// The uncompressed content streams (those that set a font).
    pub(crate) fn content_streams(pdf: &[u8]) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = pdf;
        while let Some(start) = find(rest, b"stream\n") {
            let dict_start = rfind(&rest[..start], b"<<").unwrap_or(0);
            let dict = &rest[dict_start..start];
            let body = &rest[start + 7..];
            let end = find(body, b"\nendstream").unwrap_or(body.len());
            if find(dict, b"/Filter").is_none() {
                let text = String::from_utf8_lossy(&body[..end]).into_owned();
                if text.contains(" Tf") {
                    out.push(text);
                }
            }
            rest = &body[end..];
        }
        out
    }

    /// Every `Tm` origin (e, f) in points, in order.
    pub(crate) fn text_origins(stream: &str) -> Vec<(f32, f32)> {
        let toks: Vec<&str> = stream.split_ascii_whitespace().collect();
        let mut out = Vec::new();
        for (i, t) in toks.iter().enumerate() {
            if *t == "Tm" && i >= 6 {
                let e: f32 = toks[i - 2].parse().unwrap();
                let f: f32 = toks[i - 1].parse().unwrap();
                out.push((e, f));
            }
            assert_ne!(*t, "Td", "the renderer positions with Tm only");
        }
        out
    }

    fn find(h: &[u8], n: &[u8]) -> Option<usize> {
        h.windows(n.len()).position(|w| w == n)
    }

    fn rfind(h: &[u8], n: &[u8]) -> Option<usize> {
        h.windows(n.len()).rposition(|w| w == n)
    }
}

#[cfg(test)]
mod tests {
    use super::read_back::{content_streams, text_origins};
    use super::*;
    use crate::image::tests::{jpeg_header, png_rgba};
    use crate::layout::{LETTER_H_MM, Page, Rect, TextBox};

    fn doc() -> Document {
        let mut p = Page::default();
        p.text(TextBox::new(
            "a",
            Rect::new(25.4, 25.4, 80.0, 6.0),
            Font::Sans,
            10.0,
            "Hello",
        ));
        p.text(
            TextBox::new(
                "b",
                Rect::new(100.0, 50.0, 50.0, 6.0),
                Font::Mono,
                10.0,
                "1,234.56",
            )
            .align(crate::Align::Right),
        );
        p.shape(Shape::Line {
            x1: 10.0,
            y1: 10.0,
            x2: 50.0,
            y2: 10.0,
            width_pt: 0.5,
        });
        p.shape(Shape::Box {
            rect: Rect::new(5.0, 5.0, 10.0, 10.0),
            stroke_pt: Some(0.5),
            fill_gray: Some(0.9),
        });
        Document {
            size: crate::layout::PageSize::LETTER,
            title: "Test".into(),
            pages: vec![p.clone(), p],
            logo: None,
        }
    }

    #[test]
    fn renders_a_pdf_whose_text_origins_match_the_runs() {
        let d = doc();
        let pages = resolve(&d).unwrap();
        let bytes = render_resolved(&pages, None, "Test").unwrap();
        assert!(bytes.starts_with(b"%PDF-"));
        let streams = content_streams(&bytes);
        assert_eq!(streams.len(), 2);
        for (s, page) in streams.iter().zip(&pages) {
            let origins = text_origins(s);
            assert_eq!(origins.len(), page.runs.len());
            for ((x, y), run) in origins.iter().zip(&page.runs) {
                assert!((x * MM_PER_PT - run.x).abs() < 0.01);
                assert!(((pt(LETTER_H_MM) - y) * MM_PER_PT - run.baseline).abs() < 0.01);
            }
        }
        // Fonts are subset: far smaller than the ~200 KB face files.
        assert!(bytes.len() < 60_000, "{}", bytes.len());
    }

    #[test]
    fn logos_embed_as_png_with_mask_or_jpeg() {
        for bytes in [png_rgba(4, 2), jpeg_header(40, 20)] {
            let mut d = doc();
            d.logo = Some(Logo::from_bytes(&bytes).unwrap());
            d.pages[0]
                .elements
                .push(crate::layout::Element::Logo(Rect::new(
                    150.0, 8.0, 50.0, 20.0,
                )));
            let out = render(&d).unwrap();
            let s = String::from_utf8_lossy(&out);
            assert!(s.contains("/Subtype /Image"));
            assert!(s.contains("/Logo Do"));
            if bytes[0] == 0x89 {
                assert!(s.contains("/SMask"));
            } else {
                assert!(s.contains("/DCTDecode"));
            }
        }
    }

    #[test]
    fn empty_page_lists_are_refused() {
        assert!(matches!(
            render_resolved(&[], None, "x"),
            Err(PrintError::NoPages)
        ));
    }
}
