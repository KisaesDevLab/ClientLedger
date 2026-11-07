//! Format detection from the content, never the file name (plan §3).

use crate::error::ImportError;
use crate::normalize::{Declared, MAX_FILE_BYTES, TextEncoding, decode};
use serde::{Deserialize, Serialize};

/// How far into a file detection looks, in bytes.
const SNIFF_BYTES: usize = 64 * 1024;

/// A bank file format (the batch's `format`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileFormat {
    /// Open Financial Exchange, 1.x (SGML) or 2.x (XML).
    Ofx,
    /// Quicken's OFX variant (an `INTU.BID` in the sign-on block).
    Qfx,
    /// Quicken Interchange Format.
    Qif,
    /// Comma-, semicolon- or tab-separated values.
    Csv,
}

impl FileFormat {
    /// The engine's wire name: `ofx`, `qfx`, `qif` or `csv`.
    #[must_use]
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Ofx => "ofx",
            Self::Qfx => "qfx",
            Self::Qif => "qif",
            Self::Csv => "csv",
        }
    }
}

/// OFX flavour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfxDialect {
    /// OFX 1.x: a `KEY:VALUE` header, SGML body (leaf tags unclosed).
    Sgml,
    /// OFX 2.x: an XML prolog and `<?OFX …?>`, well-formed XML body.
    Xml,
}

/// What [`detect`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Detected {
    /// The format.
    pub format: FileFormat,
    /// For OFX and QFX.
    pub dialect: Option<OfxDialect>,
    /// For CSV: `,`, `;` or tab.
    pub delimiter: Option<char>,
    /// How the start of the file decodes (CSV and QIF; OFX re-decodes with
    /// its declared character set).
    pub encoding: TextEncoding,
}

/// Finds the format of a bank file from its content.
///
/// # Errors
/// [`ImportError::Empty`], [`ImportError::TooLarge`], or
/// [`ImportError::UnknownFormat`] for anything that is not OFX, QFX, QIF or
/// delimited text (binary files, spreadsheets, PDFs).
pub fn detect(bytes: &[u8]) -> Result<Detected, ImportError> {
    if bytes.len() > MAX_FILE_BYTES {
        return Err(ImportError::TooLarge {
            limit_bytes: MAX_FILE_BYTES,
        });
    }
    let head = &bytes[..bytes.len().min(SNIFF_BYTES)];
    // A cut UTF-16 head may end mid-pair; detection only needs a prefix.
    let head = if head.len() < bytes.len() && head.len() % 2 == 1 {
        &head[..head.len() - 1]
    } else {
        head
    };
    let decoded = decode(head, Declared::Nothing).unwrap_or_else(|_| crate::normalize::Decoded {
        text: String::new(),
        encoding: TextEncoding::Windows1252,
        note: None,
    });
    let text = decoded.text.trim_start_matches(crate::normalize::is_ws);
    if text.trim_matches(crate::normalize::is_ws).is_empty() {
        return Err(ImportError::Empty);
    }
    let upper = text.to_ascii_uppercase();
    if let Some(dialect) = ofx_dialect(&upper) {
        let format = if upper.contains("<INTU.BID>") {
            FileFormat::Qfx
        } else {
            FileFormat::Ofx
        };
        return Ok(Detected {
            format,
            dialect: Some(dialect),
            delimiter: None,
            encoding: decoded.encoding,
        });
    }
    if ["!TYPE", "!ACCOUNT", "!OPTION", "!CLEAR"]
        .iter()
        .any(|p| upper.starts_with(p))
    {
        return Ok(Detected {
            format: FileFormat::Qif,
            dialect: None,
            delimiter: None,
            encoding: decoded.encoding,
        });
    }
    if looks_delimited(text) {
        return Ok(Detected {
            format: FileFormat::Csv,
            dialect: None,
            delimiter: Some(crate::csv::mapping::sniff_bank_separator(text)),
            encoding: decoded.encoding,
        });
    }
    Err(ImportError::UnknownFormat)
}

/// OFX when the text starts with an OFX header or prolog, or opens `<OFX>`
/// early on.
fn ofx_dialect(upper: &str) -> Option<OfxDialect> {
    if upper.starts_with("<?XML") || upper.starts_with("<?OFX") {
        return upper.contains("<?OFX").then_some(OfxDialect::Xml);
    }
    if upper.starts_with("OFXHEADER") {
        return Some(
            if upper.starts_with("OFXHEADER=") || upper.contains("<?OFX") {
                OfxDialect::Xml
            } else {
                OfxDialect::Sgml
            },
        );
    }
    // Some exporters leave the header out entirely.
    if upper.starts_with("<OFX>") {
        return Some(OfxDialect::Sgml);
    }
    None
}

/// Delimited text: no NUL or other binary control characters, and the first
/// line holds a separator.
fn looks_delimited(text: &str) -> bool {
    if text
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\t' | '\n' | '\r' | '\u{c}'))
    {
        return false;
    }
    // Bank exports may start with preamble lines ("Account: …"), so any of
    // the first 20 non-blank lines may hold the separator.
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .take(20)
        .any(|l| l.contains([',', ';', '\t']))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_by_content() {
        let sgml = b"OFXHEADER:100\r\nDATA:OFXSGML\r\n\r\n<OFX><SIGNONMSGSRSV1>";
        assert_eq!(
            detect(sgml).map(|d| (d.format, d.dialect)),
            Ok((FileFormat::Ofx, Some(OfxDialect::Sgml)))
        );
        let xml = b"<?xml version=\"1.0\"?>\n<?OFX OFXHEADER=\"200\"?>\n<OFX>";
        assert_eq!(detect(xml).map(|d| d.dialect), Ok(Some(OfxDialect::Xml)));
        let qfx = b"OFXHEADER:100\n<OFX><SIGNONMSGSRSV1><SONRS><INTU.BID>3000";
        assert_eq!(detect(qfx).map(|d| d.format), Ok(FileFormat::Qfx));
        assert_eq!(
            detect(b"!Type:Bank\nD1/2/26").map(|d| d.format),
            Ok(FileFormat::Qif)
        );
        let csv = b"Date;Amount;Description\n";
        assert_eq!(
            detect(csv).map(|d| (d.format, d.delimiter)),
            Ok((FileFormat::Csv, Some(';')))
        );
    }

    #[test]
    fn refuses_empty_binary_and_large() {
        assert_eq!(detect(b"  \r\n"), Err(ImportError::Empty));
        assert_eq!(
            detect(b"%PDF-1.7\n\x00\x01"),
            Err(ImportError::UnknownFormat)
        );
        assert_eq!(detect(b"just words"), Err(ImportError::UnknownFormat));
        let big = vec![b','; MAX_FILE_BYTES + 1];
        assert!(matches!(detect(&big), Err(ImportError::TooLarge { .. })));
    }

    /// Q-P25h: UTF-16 is read only with a byte-order mark (Excel's
    /// "Unicode text"); without one its NUL bytes make it unreadable, and
    /// it is refused rather than guessed.
    #[test]
    fn utf16_needs_a_byte_order_mark() {
        let text = "Date	Description	Amount
11/01/2026	COFFEE	-4.50
";
        let le: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let be: Vec<u8> = text.encode_utf16().flat_map(u16::to_be_bytes).collect();
        let with_bom = |bom: &[u8], body: &[u8]| [bom, body].concat();
        assert_eq!(
            detect(&with_bom(&[0xFF, 0xFE], &le)).map(|d| (d.format, d.encoding)),
            Ok((FileFormat::Csv, TextEncoding::Utf16Le))
        );
        assert_eq!(
            detect(&with_bom(&[0xFE, 0xFF], &be)).map(|d| (d.format, d.encoding)),
            Ok((FileFormat::Csv, TextEncoding::Utf16Be))
        );
        assert_eq!(detect(&le), Err(ImportError::UnknownFormat));
        assert_eq!(detect(&be), Err(ImportError::UnknownFormat));
        let ofx: Vec<u8> = "OFXHEADER:100
<OFX>"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert_eq!(detect(&ofx), Err(ImportError::UnknownFormat));
    }
}
