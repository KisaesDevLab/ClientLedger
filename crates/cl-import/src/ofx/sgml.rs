//! The OFX tokenizer and tree builder, shared by OFX 1.x (SGML) and 2.x
//! (XML) (plan §3, review S12).
//!
//! One iterative pass over the text, tracking lines:
//! - SGML leaf elements have no closing tag (`<TRNAMT>-12.50`); an element
//!   that holds text closes at the next tag. Closing tags that close
//!   several open elements are allowed; in SGML a stray closing tag is
//!   ignored, in XML it is an error.
//! - Depth is capped at [`MAX_DEPTH`] and every tag and value at
//!   [`MAX_TOKEN_BYTES`]; every scan stops at the next `<` or `>`, so the
//!   work is linear in the input.
//! - `<!DOCTYPE` and `<!ENTITY` are refused with their line. Only the five
//!   standard entities and numeric references are expanded; a raw `&` is
//!   kept as text ("AT&T"). An unknown `&name;` is kept as text in SGML and
//!   refused in XML.
//! - Comments, processing instructions and CDATA are handled.
//! - In SGML, a known OFX leaf ([`LEAVES`]) closes at the next tag even
//!   when it is empty (`<FITID><NAME>SHOP`), so it never swallows the
//!   fields after it.
//! - At most [`MAX_NODES`] elements are built.

use super::{MAX_DEPTH, MAX_NODES, MAX_TOKEN_BYTES, Node};
use crate::error::ImportError;
use crate::normalize::trim_ws;

/// OFX elements that only ever hold a value (never other elements).
const LEAVES: &[&str] = &[
    "ACCTID",
    "ACCTKEY",
    "ACCTTYPE",
    "BALAMT",
    "BANKID",
    "BRANCHID",
    "CHECKNUM",
    "CODE",
    "CORRECTACTION",
    "CORRECTFITID",
    "CURDEF",
    "CURRATE",
    "CURSYM",
    "DTACCTUP",
    "DTASOF",
    "DTAVAIL",
    "DTEND",
    "DTPOSTED",
    "DTPROFUP",
    "DTSERVER",
    "DTSTART",
    "DTUSER",
    "FID",
    "FITID",
    "INTU.BID",
    "INTU.USERID",
    "LANGUAGE",
    "MEMO",
    "MESSAGE",
    "NAME",
    "ORG",
    "PAYEEID",
    "REFNUM",
    "SEVERITY",
    "SIC",
    "SRVRTID",
    "TRNAMT",
    "TRNTYPE",
    "TRNUID",
];

/// Builds the element tree of an OFX body. `xml` selects the stricter XML
/// rules. Returns a nameless root holding the top-level elements.
///
/// # Errors
/// The [`ImportError`] markup variants, each with its line.
pub(crate) fn build_tree(text: &str, xml: bool, first_line: usize) -> Result<Node, ImportError> {
    let mut b = Builder {
        text,
        pos: 0,
        line: first_line,
        xml,
        stack: vec![Node::new(String::new(), first_line)],
        nodes: 0,
    };
    b.run()?;
    b.finish()
}

struct Builder<'a> {
    text: &'a str,
    pos: usize,
    line: usize,
    xml: bool,
    /// Open elements; `stack[0]` is the nameless root.
    stack: Vec<Node>,
    /// Elements opened so far.
    nodes: usize,
}

impl Builder<'_> {
    fn run(&mut self) -> Result<(), ImportError> {
        while self.pos < self.text.len() {
            let rest = &self.text[self.pos..];
            let next = rest.find('<').unwrap_or(rest.len());
            if next > 0 {
                self.text_segment(next)?;
                continue;
            }
            let tag_line = self.line;
            if rest.starts_with("<!--") {
                self.skip_past("-->", tag_line)?;
            } else if rest.starts_with("<![CDATA[") {
                self.advance(9);
                let rest = &self.text[self.pos..];
                let end = rest
                    .find("]]>")
                    .ok_or(ImportError::Truncated { line: tag_line })?;
                if end > MAX_TOKEN_BYTES {
                    return Err(ImportError::TokenTooLong { line: tag_line });
                }
                let value = rest[..end].to_owned();
                self.advance(end + 3);
                self.add_text(&value);
            } else if let Some(decl) = rest.strip_prefix("<!") {
                let word: String = decl
                    .chars()
                    .take(7)
                    .map(|c| c.to_ascii_uppercase())
                    .collect();
                return Err(if word.starts_with("DOCTYPE") {
                    ImportError::Doctype { line: tag_line }
                } else if word.starts_with("ENTITY") {
                    ImportError::Entity { line: tag_line }
                } else {
                    ImportError::BadMarkup { line: tag_line }
                });
            } else if rest.starts_with("<?") {
                self.skip_past("?>", tag_line)?;
            } else if rest.starts_with("</") {
                let name = self.tag(2, tag_line)?;
                self.close(&name, tag_line)?;
            } else {
                let name = self.tag(1, tag_line)?;
                let self_closing = self.text[..self.pos].ends_with("/>");
                self.open(name, tag_line)?;
                if self_closing {
                    self.pop();
                }
            }
        }
        Ok(())
    }

    /// Moves `n` bytes forward, counting line breaks.
    fn advance(&mut self, n: usize) {
        let end = self.pos + n;
        self.line += self.text[self.pos..end].matches('\n').count();
        self.pos = end;
    }

    fn skip_past(&mut self, end: &str, line: usize) -> Result<(), ImportError> {
        let rest = &self.text[self.pos..];
        let at = rest.find(end).ok_or(ImportError::Truncated { line })?;
        self.advance(at + end.len());
        Ok(())
    }

    /// Reads a tag starting `skip` bytes into the rest (`<` or `</`) and
    /// moves past its `>`. Returns the upper-cased name.
    fn tag(&mut self, skip: usize, line: usize) -> Result<String, ImportError> {
        let rest = &self.text[self.pos + skip..];
        let name_len = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':')))
            .unwrap_or(rest.len());
        if name_len > MAX_TOKEN_BYTES {
            return Err(ImportError::TokenTooLong { line });
        }
        if name_len == rest.len() {
            return Err(ImportError::Truncated { line });
        }
        if name_len == 0 {
            return Err(ImportError::BadMarkup { line });
        }
        let name = rest[..name_len].to_ascii_uppercase();
        // Attributes (XML only in practice) up to `>`; a `<` first is broken
        // markup, the end of the text is a cut file.
        let after = &rest[name_len..];
        let close = after.find(['>', '<']);
        match close {
            Some(i) if after.as_bytes()[i] == b'>' => {
                if i > MAX_TOKEN_BYTES {
                    return Err(ImportError::TokenTooLong { line });
                }
                self.advance(skip + name_len + i + 1);
                Ok(name)
            }
            Some(_) => Err(ImportError::BadMarkup { line }),
            None => Err(ImportError::Truncated { line }),
        }
    }

    /// Text up to the next `<` (`len` bytes).
    fn text_segment(&mut self, len: usize) -> Result<(), ImportError> {
        let line = self.line;
        let raw = &self.text[self.pos..self.pos + len];
        if trim_ws(raw).is_empty() {
            self.advance(len);
            return Ok(());
        }
        if len > MAX_TOKEN_BYTES {
            return Err(ImportError::TokenTooLong { line });
        }
        let value = decode_entities(raw, self.xml, line)?;
        self.advance(len);
        self.add_text(&value);
        Ok(())
    }

    fn add_text(&mut self, value: &str) {
        let v = trim_ws(value);
        if v.is_empty() || self.stack.len() == 1 {
            // Text outside any element (an SGML header remnant) is ignored.
            return;
        }
        if let Some(top) = self.stack.last_mut() {
            match &mut top.text {
                Some(t) => {
                    t.push(' ');
                    t.push_str(v);
                }
                None => top.text = Some(v.to_owned()),
            }
        }
    }

    /// The innermost element holds a value, or (SGML) is a known leaf that
    /// is still empty: it closes at the next tag.
    fn top_is_leaf(&self) -> bool {
        self.stack.len() > 1
            && self.stack.last().is_some_and(|n| {
                n.text.is_some()
                    || (!self.xml && n.children.is_empty() && LEAVES.contains(&n.name.as_str()))
            })
    }

    fn open(&mut self, name: String, line: usize) -> Result<(), ImportError> {
        self.nodes += 1;
        if self.nodes > MAX_NODES {
            return Err(ImportError::TooManyElements { line });
        }
        if self.top_is_leaf() {
            self.pop();
        }
        // The root does not count toward the depth.
        if self.stack.len() > MAX_DEPTH {
            return Err(ImportError::TooDeep { line });
        }
        self.stack.push(Node::new(name, line));
        Ok(())
    }

    fn close(&mut self, name: &str, line: usize) -> Result<(), ImportError> {
        if self.top_is_leaf() && self.stack.last().is_some_and(|n| n.name != name) {
            self.pop();
        }
        match self.stack.iter().rposition(|n| n.name == name) {
            Some(i) if i > 0 => {
                if self.xml && i != self.stack.len() - 1 {
                    return Err(ImportError::BadMarkup { line });
                }
                while self.stack.len() > i {
                    self.pop();
                }
                Ok(())
            }
            _ if self.xml => Err(ImportError::BadMarkup { line }),
            _ => Ok(()),
        }
    }

    /// Closes the innermost open element into its parent.
    fn pop(&mut self) {
        if self.stack.len() > 1
            && let Some(node) = self.stack.pop()
            && let Some(parent) = self.stack.last_mut()
        {
            parent.children.push(node);
        }
    }

    fn finish(mut self) -> Result<Node, ImportError> {
        if self.top_is_leaf() {
            self.pop();
        }
        if let Some(open) = self.stack.get(1..).and_then(<[Node]>::last) {
            return Err(ImportError::Truncated { line: open.line });
        }
        Ok(self
            .stack
            .pop()
            .unwrap_or_else(|| Node::new(String::new(), 1)))
    }
}

/// Expands `&amp; &lt; &gt; &quot; &apos;` and `&#N;`/`&#xH;`. A raw `&`
/// stays as text. An unknown `&name;` stays in SGML and is refused in XML.
fn decode_entities(raw: &str, xml: bool, line: usize) -> Result<String, ImportError> {
    if !raw.contains('&') {
        return Ok(raw.to_owned());
    }
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        // An entity name is short; look no further than 12 bytes.
        let after = &rest[1..];
        let semi = after
            .char_indices()
            .take(12)
            .find(|(_, c)| *c == ';')
            .map(|(i, _)| i);
        let candidate = semi.map(|s| &after[..s]);
        let expanded = candidate.and_then(|name| match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            n if n.starts_with("#x") || n.starts_with("#X") => u32::from_str_radix(&n[2..], 16)
                .ok()
                .and_then(char::from_u32),
            n if n.starts_with('#') => n[1..].parse::<u32>().ok().and_then(char::from_u32),
            _ => None,
        });
        match (expanded, candidate) {
            (Some(c), Some(name)) => {
                out.push(c);
                rest = &rest[name.len() + 2..];
            }
            (None, Some(name))
                if xml
                    && !name.is_empty()
                    && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '#') =>
            {
                return Err(ImportError::UnknownEntity { line });
            }
            _ => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sgml_leaves_close_at_next_tag() {
        let t = "<OFX>\n<A><B>one\n<C>two &amp; AT&T\n</A>\n</OFX>\n";
        let root = build_tree(t, false, 1).unwrap();
        let ofx = &root.children[0];
        let a = &ofx.children[0];
        assert_eq!(a.children.len(), 2);
        assert_eq!(a.children[0].text.as_deref(), Some("one"));
        assert_eq!(a.children[1].text.as_deref(), Some("two & AT&T"));
        assert_eq!(a.children[1].line, 3);
    }

    #[test]
    fn refusals_carry_lines() {
        assert_eq!(
            build_tree("\n<!DOCTYPE x>", false, 1).unwrap_err(),
            ImportError::Doctype { line: 2 }
        );
        assert_eq!(
            build_tree("<OFX>\n\n<!ENTITY a \"b\">", true, 1).unwrap_err(),
            ImportError::Entity { line: 3 }
        );
        assert_eq!(
            build_tree("<OFX>\n<A>&lol;</A></OFX>", true, 1).unwrap_err(),
            ImportError::UnknownEntity { line: 2 }
        );
        assert_eq!(
            build_tree("<OFX>\n<A>\n<B>1", false, 1).unwrap_err(),
            ImportError::Truncated { line: 2 }
        );
        assert_eq!(
            build_tree("<OFX><A", false, 1).unwrap_err(),
            ImportError::Truncated { line: 1 }
        );
        let deep = "<A>".repeat(100_000);
        assert_eq!(
            build_tree(&deep, false, 1).unwrap_err(),
            ImportError::TooDeep { line: 1 }
        );
        let long = format!("<OFX><A>{}</A></OFX>", "9".repeat(MAX_TOKEN_BYTES + 1));
        assert_eq!(
            build_tree(&long, false, 1).unwrap_err(),
            ImportError::TokenTooLong { line: 1 }
        );
        assert_eq!(
            build_tree("<OFX></B></OFX>", true, 1).unwrap_err(),
            ImportError::BadMarkup { line: 1 }
        );
        assert!(build_tree("<OFX></B></OFX>", false, 1).is_ok());
    }

    #[test]
    fn empty_sgml_leaves_do_not_swallow_the_next_field() {
        let t = "<OFX><STMTTRN><TRNAMT>-5.00<FITID><NAME>SHOP<MEMO></STMTTRN></OFX>";
        let root = build_tree(t, false, 1).unwrap();
        let trn = &root.children[0].children[0];
        let names: Vec<&str> = trn.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["TRNAMT", "FITID", "NAME", "MEMO"]);
        assert_eq!(trn.children[1].text, None);
        assert_eq!(trn.children[2].text.as_deref(), Some("SHOP"));
        // An unknown empty element still nests (an aggregate).
        let root = build_tree("<OFX><AGG><X>1</AGG></OFX>", false, 1).unwrap();
        assert_eq!(root.children[0].children[0].children[0].name, "X");
    }

    #[test]
    fn the_element_count_is_capped() {
        let many = format!("<OFX>{}</OFX>", "<A/>".repeat(MAX_NODES));
        assert_eq!(
            build_tree(&many, false, 1).unwrap_err(),
            ImportError::TooManyElements { line: 1 }
        );
        let fine = format!("<OFX>{}</OFX>", "<A/>".repeat(1000));
        assert!(build_tree(&fine, false, 1).is_ok());
    }

    #[test]
    fn xml_forms() {
        let t = "<?xml version=\"1.0\"?><!-- hi --><OFX><A x=\"1\">a&#65;&#x42;<![CDATA[<c>]]></A><E/></OFX>";
        let root = build_tree(t, true, 1).unwrap();
        let ofx = &root.children[0];
        assert_eq!(ofx.children[0].text.as_deref(), Some("aAB <c>"));
        assert_eq!(ofx.children[1].name, "E");
    }
}
