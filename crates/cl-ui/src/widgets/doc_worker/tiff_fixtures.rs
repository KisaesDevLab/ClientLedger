//! The synthetic TIFF fixtures (A02, plan §3 step 1) and the hostile TIFFs
//! the worker tests feed it. Everything is drawn here, nothing comes from a
//! real scan.
//!
//! The committed files in `fixtures/docs/` are this module's output:
//! `committed_tiff_fixtures_match_their_generator` fails when they differ,
//! and writes them again when run with `CL_WRITE_FIXTURES=1`.
//!
//! The writer is deliberately small: little-endian TIFF 6.0, one strip a
//! page, uncompressed, PackBits or CCITT G4 (the `fax` crate's encoder). It
//! writes whatever a test asks for, including headers that lie.

use std::path::{Path, PathBuf};

/// Compression tag values.
const NONE: u16 = 1;
const G4: u16 = 4;
const PACKBITS: u16 = 32773;

/// Photometric tag values.
const WHITE_IS_ZERO: u16 = 0;
const BLACK_IS_ZERO: u16 = 1;
const RGB: u16 = 2;
const CMYK: u16 = 5;
const CIELAB: u16 = 8;

/// One page as written.
#[derive(Clone)]
pub(crate) struct Page {
    pub w: u32,
    pub h: u32,
    pub photometric: u16,
    /// Bits of each sample (its length is the samples a pixel).
    pub bits: Vec<u16>,
    pub compression: u16,
    /// The strip as stored.
    pub data: Vec<u8>,
    /// The strip byte count written (normally `data.len()`).
    pub claimed_len: Option<u32>,
}

impl Page {
    fn raw(w: u32, h: u32, photometric: u16, bits: &[u16], data: Vec<u8>) -> Page {
        Page {
            w,
            h,
            photometric,
            bits: bits.to_vec(),
            compression: NONE,
            data,
            claimed_len: None,
        }
    }
}

/// PackBits, one row at a time (TIFF 6.0 section 9).
fn packbits(data: &[u8], row_len: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for row in data.chunks(row_len) {
        let n = row.len();
        let mut i = 0;
        while i < n {
            let mut run = 1;
            while i + run < n && run < 128 && row[i + run] == row[i] {
                run += 1;
            }
            if run >= 2 {
                out.push(u8::try_from(257 - run).expect("2..=128"));
                out.push(row[i]);
                i += run;
            } else {
                let start = i;
                i += 1;
                while i < n && i - start < 128 && !(i + 1 < n && row[i] == row[i + 1]) {
                    i += 1;
                }
                out.push(u8::try_from(i - start - 1).expect("0..128"));
                out.extend_from_slice(&row[start..i]);
            }
        }
    }
    out
}

/// One IFD entry: tag, type (3 SHORT, 4 LONG) and values.
struct Entry {
    tag: u16,
    long: bool,
    values: Vec<u32>,
}

fn short(tag: u16, values: &[u16]) -> Entry {
    Entry {
        tag,
        long: false,
        values: values.iter().map(|v| u32::from(*v)).collect(),
    }
}

fn long(tag: u16, v: u32) -> Entry {
    Entry {
        tag,
        long: true,
        values: vec![v],
    }
}

/// Write `pages`; with `loop_to_first` the last page's "next page" points
/// back to the first, so the page list never ends.
pub(crate) fn write(pages: &[Page], loop_to_first: bool) -> Vec<u8> {
    let mut out = b"II*\0\0\0\0\0".to_vec();
    let mut next_at = 4usize;
    let mut first_ifd = 0u32;
    for (i, p) in pages.iter().enumerate() {
        let data_at = u32::try_from(out.len()).expect("small");
        out.extend_from_slice(&p.data);
        if out.len() % 2 == 1 {
            out.push(0);
        }
        let samples = u16::try_from(p.bits.len()).expect("few");
        let mut entries = vec![
            long(256, p.w),
            long(257, p.h),
            short(258, &p.bits),
            short(259, &[p.compression]),
            short(262, &[p.photometric]),
            long(273, data_at),
            short(277, &[samples]),
            long(278, p.h),
            long(
                279,
                p.claimed_len
                    .unwrap_or_else(|| u32::try_from(p.data.len()).expect("small")),
            ),
        ];
        if samples > 1 {
            entries.push(short(284, &[1]));
        }
        // Values over 4 bytes go before the directory.
        let mut offsets = Vec::new();
        for e in &entries {
            let size = e.values.len() * if e.long { 4 } else { 2 };
            if size > 4 {
                offsets.push(Some(u32::try_from(out.len()).expect("small")));
                for v in &e.values {
                    if e.long {
                        out.extend_from_slice(&v.to_le_bytes());
                    } else {
                        let s = u16::try_from(*v).expect("short");
                        out.extend_from_slice(&s.to_le_bytes());
                    }
                }
            } else {
                offsets.push(None);
            }
        }
        let ifd_at = u32::try_from(out.len()).expect("small");
        if i == 0 {
            first_ifd = ifd_at;
        }
        out[next_at..next_at + 4].copy_from_slice(&ifd_at.to_le_bytes());
        let count = u16::try_from(entries.len()).expect("few");
        out.extend_from_slice(&count.to_le_bytes());
        for (e, off) in entries.iter().zip(&offsets) {
            out.extend_from_slice(&e.tag.to_le_bytes());
            out.extend_from_slice(&(if e.long { 4u16 } else { 3 }).to_le_bytes());
            let n = u32::try_from(e.values.len()).expect("few");
            out.extend_from_slice(&n.to_le_bytes());
            let mut field = [0u8; 4];
            if let Some(o) = off {
                field = o.to_le_bytes();
            } else if e.long {
                field = e.values[0].to_le_bytes();
            } else {
                for (k, v) in e.values.iter().enumerate() {
                    let s = u16::try_from(*v).expect("short");
                    field[k * 2..k * 2 + 2].copy_from_slice(&s.to_le_bytes());
                }
            }
            out.extend_from_slice(&field);
        }
        next_at = out.len();
        out.extend_from_slice(&[0, 0, 0, 0]);
    }
    if loop_to_first {
        out[next_at..next_at + 4].copy_from_slice(&first_ifd.to_le_bytes());
    }
    out
}

/// Page 1 of `three_pages.tif`: 8-bit grey, a dark band across the top.
fn page_gray() -> Page {
    let (w, h) = (120u32, 160u32);
    let mut px = vec![255u8; (w * h) as usize];
    for y in 10..40 {
        for x in 10..110 {
            px[(y * w + x) as usize] = 40;
        }
    }
    let mut p = Page::raw(w, h, BLACK_IS_ZERO, &[8], packbits(&px, w as usize));
    p.compression = PACKBITS;
    p
}

/// Page 2: RGB, a red square in the middle.
fn page_rgb() -> Page {
    let (w, h) = (120u32, 160u32);
    let mut px = vec![255u8; (w * h * 3) as usize];
    for y in 50..110 {
        for x in 30..90 {
            let i = ((y * w + x) * 3) as usize;
            px[i..i + 3].copy_from_slice(&[200, 30, 30]);
        }
    }
    let mut p = Page::raw(w, h, RGB, &[8, 8, 8], packbits(&px, (w * 3) as usize));
    p.compression = PACKBITS;
    p
}

/// Page 3: 1-bit, black at the bottom (1 is white).
fn page_bilevel() -> Page {
    let (w, h) = (120u32, 160u32);
    let stride = w.div_ceil(8) as usize;
    let mut px = vec![0xFFu8; stride * h as usize];
    for y in 120..150 {
        for b in 1..14 {
            px[y * stride + b] = 0;
        }
    }
    Page::raw(w, h, BLACK_IS_ZERO, &[1], px)
}

/// `three_pages.tif`: grey, colour and 1-bit pages, each drawn differently.
pub(crate) fn three_pages() -> Vec<u8> {
    write(&[page_gray(), page_rgb(), page_bilevel()], false)
}

/// `g4_fax.tif`: a CCITT G4 page (white is zero, as faxes are): a black
/// box and three "lines of text".
pub(crate) fn g4_fax() -> Vec<u8> {
    use fax::{Color, VecWriter, encoder::Encoder};
    let (w, h) = (200u16, 100u16);
    let black = |x: u16, y: u16| {
        let across = (10..190).contains(&x) && matches!(y, 10 | 11 | 88 | 89);
        let down = (10..90).contains(&y) && matches!(x, 10 | 11 | 188 | 189);
        // Three lines of "words": short dashes, the last line shorter.
        let text = [(30u16, 170u16), (50, 170), (70, 110)]
            .iter()
            .any(|&(top, end)| {
                (top..top + 6).contains(&y) && (20..end).contains(&x) && !x.is_multiple_of(9)
            });
        across || down || text
    };
    let mut e = Encoder::new(VecWriter::new());
    for y in 0..h {
        let line = (0..w).map(|x| {
            if black(x, y) {
                Color::Black
            } else {
                Color::White
            }
        });
        e.encode_line(line, w).expect("infallible");
    }
    let data = match e.finish() {
        Ok(v) => v.finish(),
        Err(never) => match never {},
    };
    let mut p = Page::raw(u32::from(w), u32::from(h), WHITE_IS_ZERO, &[1], data);
    p.compression = G4;
    write(&[p], false)
}

/// `bomb_tiff.tif`: a header claiming 30,000 × 30,000 grey pixels over a
/// few bytes of data.
pub(crate) fn bomb() -> Vec<u8> {
    let mut p = Page::raw(30_000, 30_000, BLACK_IS_ZERO, &[8], vec![0; 16]);
    p.claimed_len = Some(900_000_000);
    write(&[p], false)
}

/// Two pages whose list loops back to the first.
pub(crate) fn ifd_loop() -> Vec<u8> {
    write(&[page_gray(), page_rgb()], true)
}

/// A page whose strip claims more bytes than the file has.
pub(crate) fn truncated_strip() -> Vec<u8> {
    let (w, h) = (64u32, 64u32);
    let mut p = Page::raw(w, h, BLACK_IS_ZERO, &[8], vec![128; 100]);
    p.claimed_len = Some(w * h);
    write(&[p], false)
}

/// A CIE L*a*b* page (not taken).
fn page_lab() -> Page {
    Page::raw(16, 16, CIELAB, &[8, 8, 8], vec![100; 16 * 16 * 3])
}

/// A Lab page alone.
pub(crate) fn lab_only() -> Vec<u8> {
    write(&[page_lab()], false)
}

/// A good grey page, then a Lab one.
pub(crate) fn gray_then_lab() -> Vec<u8> {
    write(&[page_gray(), page_lab()], false)
}

/// A CMYK page: pure cyan.
pub(crate) fn cmyk() -> Vec<u8> {
    let px: Vec<u8> = std::iter::repeat_n([255u8, 0, 0, 0], 8 * 8)
        .flatten()
        .collect();
    write(&[Page::raw(8, 8, CMYK, &[8, 8, 8, 8], px)], false)
}

/// `n` small grey pages.
pub(crate) fn many_pages(n: usize) -> Vec<u8> {
    let p = Page::raw(4, 4, BLACK_IS_ZERO, &[8], vec![200; 16]);
    write(&vec![p; n], false)
}

/// The committed fixtures by file name.
pub(crate) fn committed() -> [(&'static str, Vec<u8>); 3] {
    [
        ("three_pages.tif", three_pages()),
        ("g4_fax.tif", g4_fax()),
        ("bomb_tiff.tif", bomb()),
    ]
}

/// `fixtures/docs/`.
pub(crate) fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/docs")
}

#[test]
fn committed_tiff_fixtures_match_their_generator() {
    let write = std::env::var("CL_WRITE_FIXTURES").as_deref() == Ok("1");
    for (name, bytes) in committed() {
        let path = dir().join(name);
        if write {
            std::fs::write(&path, &bytes).expect("write the fixture");
        }
        let on_disk = std::fs::read(&path).expect("the fixture (CL_WRITE_FIXTURES=1 writes it)");
        assert!(
            on_disk == bytes,
            "{name} differs from its generator: run with CL_WRITE_FIXTURES=1"
        );
    }
}

#[test]
fn packbits_round_trips_runs_and_literals() {
    let row = [1u8, 1, 1, 2, 3, 4, 4, 5];
    assert_eq!(
        packbits(&row, row.len()),
        vec![254, 1, 1, 2, 3, 255, 4, 0, 5]
    );
    // Long runs split at 128.
    let long = vec![7u8; 300];
    let p = packbits(&long, 300);
    assert_eq!(p, vec![129, 7, 129, 7, 213, 7]);
}
