//! Tiny synthetic PDFs for the worker's tests (Helvetica text, any page
//! box and rotation), with a correct cross-reference table.

use std::fmt::Write as _;

/// One page.
pub struct Page {
    pub media: [i32; 4],
    pub crop: Option<[i32; 4]>,
    pub rotate: i32,
    pub content: String,
}

impl Page {
    /// A US letter page with `content`.
    pub fn letter(content: String) -> Page {
        Page {
            media: [0, 0, 612, 792],
            crop: None,
            rotate: 0,
            content,
        }
    }
}

/// `s` in 12 pt Helvetica with its baseline at (`x`, `y`) points. `s` must
/// not contain parentheses or backslashes.
pub fn text(x: i32, y: i32, size: i32, s: &str) -> String {
    format!("BT /F1 {size} Tf {x} {y} Td ({s}) Tj ET\n")
}

/// The PDF file for `pages`.
pub fn build(pages: &[Page]) -> Vec<u8> {
    let mut objects: Vec<String> = Vec::new();
    let kids: Vec<String> = (0..pages.len())
        .map(|i| format!("{} 0 R", 4 + i * 2))
        .collect();
    objects.push("<< /Type /Catalog /Pages 2 0 R >>".into());
    objects.push(format!(
        "<< /Type /Pages /Kids [{}] /Count {} >>",
        kids.join(" "),
        pages.len()
    ));
    objects.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into());
    for (i, p) in pages.iter().enumerate() {
        let b = |r: [i32; 4]| format!("[{} {} {} {}]", r[0], r[1], r[2], r[3]);
        let crop = p
            .crop
            .map(|c| format!(" /CropBox {}", b(c)))
            .unwrap_or_default();
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox {}{crop} /Rotate {} /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>",
            b(p.media),
            p.rotate,
            5 + i * 2
        ));
        objects.push(format!(
            "<< /Length {} >>\nstream\n{}endstream",
            p.content.len(),
            p.content
        ));
    }
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    let mut table = format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1);
    for off in offsets {
        let _ = writeln!(table, "{off:010} 00000 n ");
    }
    out.extend_from_slice(table.as_bytes());
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}
