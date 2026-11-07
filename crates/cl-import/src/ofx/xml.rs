//! OFX headers: the 1.x `KEY:VALUE` block and the 2.x XML prolog, read
//! before the body is decoded so the declared character set can be used
//! (Q-P25h, review S11).

use crate::error::ImportError;
use crate::normalize::{Declared, declared_charset};

/// What the header says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Header {
    /// XML (OFX 2.x) rules for the body.
    pub xml: bool,
    /// How to decode the body.
    pub declared: Declared,
}

/// Reads the header from the start of the file. `head` is the first bytes
/// mapped one-to-one to characters (headers are ASCII).
///
/// # Errors
/// [`ImportError::UnsupportedCharset`] for a character set other than the
/// accepted ones, with the line that declares it.
pub(crate) fn read_header(head: &str) -> Result<Header, ImportError> {
    if is_xml(head) {
        return xml_header(head);
    }
    if head
        .trim_start()
        .get(..9)
        .is_some_and(|s| s.eq_ignore_ascii_case("OFXHEADER"))
    {
        return sgml_header(head);
    }
    Ok(Header {
        xml: false,
        declared: Declared::Nothing,
    })
}

/// Whether the text starts with an XML prolog or the OFX 2 processing
/// instruction.
pub(crate) fn is_xml(text: &str) -> bool {
    let start: String = text
        .trim_start()
        .chars()
        .take(5)
        .map(|c| c.to_ascii_uppercase())
        .collect();
    start == "<?XML" || start == "<?OFX"
}

fn xml_header(head: &str) -> Result<Header, ImportError> {
    // `<?xml version="1.0" encoding="UTF-8" standalone="no"?>`
    let declared = match head.find("?>") {
        Some(end) if head.trim_start().to_ascii_lowercase().starts_with("<?xml") => {
            let prolog = &head[..end];
            let line = 1 + head[..end].matches('\n').count();
            match attribute(prolog, "encoding") {
                Some(enc) => {
                    declared_charset(enc).ok_or_else(|| ImportError::UnsupportedCharset {
                        line,
                        charset: crate::error::charset_name(enc),
                    })?
                }
                None => Declared::Utf8,
            }
        }
        _ => Declared::Utf8,
    };
    Ok(Header {
        xml: true,
        declared,
    })
}

/// The value of `name="…"` or `name='…'` in a prolog.
fn attribute<'a>(prolog: &'a str, name: &str) -> Option<&'a str> {
    let lower = prolog.to_ascii_lowercase();
    let at = lower.find(&format!("{name}="))?;
    let rest = &prolog[at + name.len() + 1..];
    let quote = rest.chars().next().filter(|c| *c == '"' || *c == '\'')?;
    let rest = &rest[1..];
    rest.find(quote).map(|end| &rest[..end])
}

fn sgml_header(head: &str) -> Result<Header, ImportError> {
    let mut encoding: Option<(usize, String)> = None;
    let mut charset: Option<(usize, String)> = None;
    for (i, line) in head.lines().enumerate() {
        let line_no = i + 1;
        let t = line.trim();
        if t.starts_with('<') {
            break;
        }
        // Some files put the header on one line: split on spaces too.
        for part in t.split_whitespace() {
            let Some((k, v)) = part.split_once(':') else {
                continue;
            };
            match k.trim().to_ascii_uppercase().as_str() {
                "ENCODING" => encoding = Some((line_no, v.trim().to_owned())),
                "CHARSET" => charset = Some((line_no, v.trim().to_owned())),
                _ => {}
            }
        }
    }
    let declared = match (&encoding, &charset) {
        (Some((line, enc)), _) if declared_charset(enc).is_none() => {
            return Err(ImportError::UnsupportedCharset {
                line: *line,
                charset: crate::error::charset_name(enc),
            });
        }
        (Some((_, enc)), _) if declared_charset(enc) == Some(Declared::Utf8) => Declared::Utf8,
        (_, Some((line, cs))) => {
            declared_charset(cs).ok_or_else(|| ImportError::UnsupportedCharset {
                line: *line,
                charset: crate::error::charset_name(cs),
            })?
        }
        _ => Declared::SingleByte,
    };
    Ok(Header {
        xml: false,
        declared,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn charset_aliases() {
        let h = |s| read_header(s).map(|h| h.declared);
        assert_eq!(
            h("OFXHEADER:100\nENCODING:USASCII\nCHARSET:1252\n<OFX>"),
            Ok(Declared::SingleByte)
        );
        assert_eq!(
            h("OFXHEADER:100\nENCODING:USASCII\nCHARSET:NONE\n<OFX>"),
            Ok(Declared::SingleByte)
        );
        assert_eq!(
            h("OFXHEADER:100\nENCODING:USASCII\nCHARSET:ISO-8859-1\n<OFX>"),
            Ok(Declared::SingleByte)
        );
        assert_eq!(
            h("OFXHEADER:100\nENCODING:UTF-8\nCHARSET:NONE\n<OFX>"),
            Ok(Declared::Utf8)
        );
        assert_eq!(
            h("OFXHEADER:100\nENCODING:USASCII\nCHARSET:KOI8-R\n<OFX>"),
            Err(ImportError::UnsupportedCharset {
                line: 3,
                charset: "KOI8-R".into()
            })
        );
        assert_eq!(
            h("<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?>\n<?OFX ?>"),
            Ok(Declared::SingleByte)
        );
        assert_eq!(h("<?xml version=\"1.0\"?>\n<?OFX ?>"), Ok(Declared::Utf8));
        assert_eq!(
            h("<?xml version=\"1.0\" encoding=\"Shift_JIS\"?>"),
            Err(ImportError::UnsupportedCharset {
                line: 1,
                charset: "Shift_JIS".into()
            })
        );
        assert!(read_header("<?xml version=\"1.0\"?>").unwrap().xml);
    }
}
