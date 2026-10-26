//! A minimal PDF writer for synthetic documents (Q-A03e): text in embedded
//! subset TrueType fonts (CID, Identity-H) with an optional `ToUnicode` map,
//! plus rules and shaded boxes. Coordinates are points from the page's top
//! left; page geometry is the only use of floating point here.

use super::fonts::{FontId, Fonts};
use anyhow::{Result, anyhow};
use pdf_writer::types::{CidFontType, FontFlags, SystemInfo};
use pdf_writer::{Content, Filter, Finish, Name, Pdf, Rect, Ref, Str};
use std::collections::BTreeMap;

/// One thing drawn on a page.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    /// Text with its baseline at `y` (from the top).
    Text {
        x: f32,
        y: f32,
        size: f32,
        font: FontId,
        text: String,
    },
    /// A rule from (x1, y1) to (x2, y2).
    Line {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        width: f32,
    },
    /// A filled box (`gray` 0 = black, 1 = white).
    Fill {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        gray: f32,
    },
}

/// One page.
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub width: f32,
    pub height: f32,
    pub items: Vec<Item>,
    /// A full-page picture under the items (a scanned PDF page).
    pub image: Option<PageImage>,
}

/// A grey JPEG drawn over the whole page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageImage {
    pub jpeg: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

struct Embedded {
    remap: subsetter::GlyphRemapper,
    /// char → glyph id in the original font.
    glyphs: BTreeMap<char, u16>,
}

/// Write `pages` as a PDF. Without `to_unicode` the fonts carry no
/// `ToUnicode` map, so a reader cannot get text back from the text layer
/// (the subset fonts have no cmap either): those documents need OCR.
#[allow(clippy::too_many_lines)]
pub fn write(pages: &[Page], fonts: &Fonts, to_unicode: bool, title: &str) -> Result<Vec<u8>> {
    // Glyphs per font, in first-use order.
    let mut used: BTreeMap<FontId, Embedded> = BTreeMap::new();
    for page in pages {
        for item in &page.items {
            if let Item::Text { font, text, .. } = item {
                let face = fonts.face(*font)?;
                let e = used.entry(*font).or_insert_with(|| Embedded {
                    remap: subsetter::GlyphRemapper::new(),
                    glyphs: BTreeMap::new(),
                });
                for c in text.chars() {
                    let g = face
                        .glyph_index(c)
                        .or_else(|| face.glyph_index('?'))
                        .map_or(0, |g| g.0);
                    e.remap.remap(g);
                    e.glyphs.insert(c, g);
                }
            }
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

    // Fonts.
    let mut font_refs: BTreeMap<FontId, (Ref, String)> = BTreeMap::new();
    for (i, (id, e)) in used.iter().enumerate() {
        let face = fonts.face(*id)?;
        let upm = f32::from(face.units_per_em());
        let scale = |v: i16| f32::from(v) * 1000.0 / upm;
        let sub = subsetter::subset(fonts.bytes(*id)?, 0, &e.remap)
            .map_err(|err| anyhow!("subset {}: {err}", id.file()))?;
        let type0 = alloc();
        let cid = alloc();
        let desc = alloc();
        let file = alloc();
        let cmap = alloc();
        let tag: String = (0..6)
            .map(|k| char::from(b'A' + u8::try_from((i * 7 + k * 3) % 26).unwrap_or(0)))
            .collect();
        let base = format!("{tag}+{}", id.ps_name());
        let mut t0 = pdf.type0_font(type0);
        t0.base_font(Name(base.as_bytes()))
            .encoding_predefined(Name(b"Identity-H"))
            .descendant_font(cid);
        if to_unicode {
            t0.to_unicode(cmap);
        }
        t0.finish();
        let sys = SystemInfo {
            registry: Str(b"Adobe"),
            ordering: Str(b"Identity"),
            supplement: 0,
        };
        let widths: Vec<f32> = e
            .remap
            .remapped_gids()
            .map(|old| {
                f32::from(
                    face.glyph_hor_advance(ttf_parser::GlyphId(old))
                        .unwrap_or(0),
                ) * 1000.0
                    / upm
            })
            .collect();
        let mut c = pdf.cid_font(cid);
        c.subtype(CidFontType::Type2)
            .base_font(Name(base.as_bytes()))
            .system_info(sys)
            .font_descriptor(desc)
            .cid_to_gid_map_predefined(Name(b"Identity"));
        c.widths().consecutive(0, widths);
        c.finish();
        let bb = face.global_bounding_box();
        let mut flags = FontFlags::NON_SYMBOLIC;
        if id.monospace() {
            flags |= FontFlags::FIXED_PITCH;
        }
        if matches!(id, FontId::PtSerif | FontId::PtSerifBold) {
            flags |= FontFlags::SERIF;
        }
        pdf.font_descriptor(desc)
            .name(Name(base.as_bytes()))
            .flags(flags)
            .bbox(Rect::new(
                scale(bb.x_min),
                scale(bb.y_min),
                scale(bb.x_max),
                scale(bb.y_max),
            ))
            .italic_angle(0.0)
            .ascent(scale(face.ascender()))
            .descent(scale(face.descender()))
            .cap_height(scale(face.capital_height().unwrap_or(face.ascender())))
            .stem_v(80.0)
            .font_file2(file);
        let compressed = deflate(&sub);
        pdf.stream(file, &compressed)
            .filter(Filter::FlateDecode)
            .pair(Name(b"Length1"), i32::try_from(sub.len()).unwrap_or(0));
        if to_unicode {
            let mut u = pdf_writer::types::UnicodeCmap::new(Name(b"Custom"), sys);
            for (&ch, &old) in &e.glyphs {
                if let Some(new) = e.remap.get(old) {
                    u.pair(new, ch);
                }
            }
            pdf.cmap(cmap, &u.finish()).system_info(sys);
        }
        font_refs.insert(*id, (type0, format!("F{i}")));
    }

    // Pages.
    let page_ids: Vec<(Ref, Ref, Ref)> =
        pages.iter().map(|_| (alloc(), alloc(), alloc())).collect();
    pdf.catalog(catalog).pages(tree);
    pdf.pages(tree)
        .kids(page_ids.iter().map(|(p, _, _)| *p))
        .count(i32::try_from(pages.len()).unwrap_or(0));
    for (page, (pid, cid, iid)) in pages.iter().zip(&page_ids) {
        let mut p = pdf.page(*pid);
        p.media_box(Rect::new(0.0, 0.0, page.width, page.height))
            .parent(tree)
            .contents(*cid);
        let mut res = p.resources();
        let mut fs = res.fonts();
        for (r, name) in font_refs.values() {
            fs.pair(Name(name.as_bytes()), *r);
        }
        fs.finish();
        if page.image.is_some() {
            res.x_objects().pair(Name(b"Im0"), *iid);
        }
        res.finish();
        p.finish();
        if let Some(img) = &page.image {
            let mut x = pdf.image_xobject(*iid, &img.jpeg);
            x.filter(Filter::DctDecode);
            x.width(i32::try_from(img.width).unwrap_or(1))
                .height(i32::try_from(img.height).unwrap_or(1))
                .color_space_name(Name(b"DeviceGray"))
                .bits_per_component(8);
        }
        let content = content(page, &used, &font_refs);
        pdf.stream(*cid, &content);
    }
    pdf.document_info(info)
        .producer(pdf_writer::TextStr("ClientLedger synthetic test document"))
        .title(pdf_writer::TextStr(title));
    Ok(pdf.finish())
}

fn content(
    page: &Page,
    used: &BTreeMap<FontId, Embedded>,
    refs: &BTreeMap<FontId, (Ref, String)>,
) -> Vec<u8> {
    let mut c = Content::new();
    let h = page.height;
    if page.image.is_some() {
        c.save_state();
        c.transform([page.width, 0.0, 0.0, h, 0.0, 0.0]);
        c.x_object(Name(b"Im0"));
        c.restore_state();
    }
    for item in &page.items {
        match item {
            Item::Fill {
                x,
                y,
                w,
                h: bh,
                gray,
            } => {
                c.save_state();
                c.set_fill_gray(*gray);
                c.rect(*x, h - y - bh, *w, *bh);
                c.fill_nonzero();
                c.restore_state();
            }
            Item::Line {
                x1,
                y1,
                x2,
                y2,
                width,
            } => {
                c.set_line_width(*width);
                c.move_to(*x1, h - y1);
                c.line_to(*x2, h - y2);
                c.stroke();
            }
            Item::Text {
                x,
                y,
                size,
                font,
                text,
            } => {
                let (Some(e), Some((_, name))) = (used.get(font), refs.get(font)) else {
                    continue;
                };
                let mut bytes = Vec::with_capacity(text.len() * 2);
                for ch in text.chars() {
                    let old = e.glyphs.get(&ch).copied().unwrap_or(0);
                    let new = e.remap.get(old).unwrap_or(0);
                    bytes.extend_from_slice(&new.to_be_bytes());
                }
                c.begin_text();
                c.set_font(Name(name.as_bytes()), *size);
                c.set_text_matrix([1.0, 0.0, 0.0, 1.0, *x, h - y]);
                c.show(Str(&bytes));
                c.end_text();
            }
        }
    }
    c.finish().into_vec()
}

fn deflate(data: &[u8]) -> Vec<u8> {
    use std::io::Write as _;
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    // Writing to a Vec cannot fail.
    let _ = e.write_all(data);
    e.finish().unwrap_or_default()
}
