//! Text rules of an IIF file: field sanitizing, Windows-1252 encoding and
//! length limits (Q-P32j).
//!
//! An IIF file is tab-separated text, one record per line. A field must
//! never hold a tab, a line break or a double quote (QuickBooks reads a
//! quote as the start of a quoted field). List names must not hold `:`,
//! which separates a parent from its sub-account.
//!
//! The file is written in Windows-1252 by this hand-written encoder (no
//! new dependency). Characters it cannot hold become `?`, and so do the
//! C1 control characters U+0080–U+009F. The printable characters that
//! Windows-1252 places at bytes 0x80–0x9F (`€`, curly quotes, dashes, `…`)
//! are kept.

#![allow(
    clippy::doc_markdown,
    reason = "QuickBooks and ClientLedger are product names, not code"
)]
/// The Windows-1252 byte for a character, or `None` when it has none.
/// U+0080–U+009F (C1 controls) have none, by rule.
#[must_use]
pub fn cp1252_byte(c: char) -> Option<u8> {
    let u = u32::from(c);
    match u {
        0x20..=0x7E | 0xA0..=0xFF => u8::try_from(u).ok(),
        _ => cp1252_high(c),
    }
}

/// The characters Windows-1252 places at bytes 0x80–0x9F.
fn cp1252_high(c: char) -> Option<u8> {
    Some(match c {
        '\u{20AC}' => 0x80,
        '\u{201A}' => 0x82,
        '\u{0192}' => 0x83,
        '\u{201E}' => 0x84,
        '\u{2026}' => 0x85,
        '\u{2020}' => 0x86,
        '\u{2021}' => 0x87,
        '\u{02C6}' => 0x88,
        '\u{2030}' => 0x89,
        '\u{0160}' => 0x8A,
        '\u{2039}' => 0x8B,
        '\u{0152}' => 0x8C,
        '\u{017D}' => 0x8E,
        '\u{2018}' => 0x91,
        '\u{2019}' => 0x92,
        '\u{201C}' => 0x93,
        '\u{201D}' => 0x94,
        '\u{2022}' => 0x95,
        '\u{2013}' => 0x96,
        '\u{2014}' => 0x97,
        '\u{02DC}' => 0x98,
        '\u{2122}' => 0x99,
        '\u{0161}' => 0x9A,
        '\u{203A}' => 0x9B,
        '\u{0153}' => 0x9C,
        '\u{017E}' => 0x9E,
        '\u{0178}' => 0x9F,
        _ => return None,
    })
}

/// The character for a Windows-1252 byte (the reader's side, for tests
/// and the parser test).
#[must_use]
pub fn cp1252_char(b: u8) -> Option<char> {
    match b {
        0x20..=0x7E | 0xA0..=0xFF => Some(char::from(b)),
        0x09 => Some('\t'),
        0x0D => Some('\r'),
        0x0A => Some('\n'),
        _ => (0x80u32..=0x9F)
            .contains(&u32::from(b))
            .then(|| {
                (0x80u32..=0xFFFF)
                    .filter_map(char::from_u32)
                    .find(|&c| cp1252_high(c) == Some(b))
            })
            .flatten(),
    }
}

/// Encodes text already made safe by [`Fields::clean`]; any character
/// without a byte becomes `?` (never happens for cleaned text).
#[must_use]
pub fn encode(s: &str) -> Vec<u8> {
    s.chars().map(|c| cp1252_byte(c).unwrap_or(b'?')).collect()
}

/// What a field is, for its rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    /// A list name segment (account segment, client, vendor, item): `:`
    /// becomes `-`.
    ListName,
    /// Free text (memo, description, number).
    Text,
}

/// Cleans field text and counts the characters that became `?`.
#[derive(Debug, Default)]
pub struct Fields {
    /// Characters replaced by `?` so far.
    pub unmappable: u64,
}

impl Fields {
    /// Tabs, line breaks and `"` become a space (and `:` becomes `-` in a
    /// list name); characters Windows-1252 can't hold become `?`
    /// (counted); the result is trimmed and cut to `max` characters, then
    /// trimmed again.
    pub fn clean(&mut self, s: &str, kind: FieldKind, max: usize) -> String {
        let mut out = String::with_capacity(s.len());
        for c in s.chars() {
            let c = match c {
                '\t' | '\r' | '\n' | '"' => ' ',
                ':' if kind == FieldKind::ListName => '-',
                c if cp1252_byte(c).is_some() => c,
                _ => {
                    self.unmappable += 1;
                    '?'
                }
            };
            out.push(c);
        }
        let cut: String = out.trim().chars().take(max).collect();
        cut.trim_end().to_string()
    }
}

/// Makes `name` unique among `taken` (compared without regard to case,
/// as QuickBooks compares names), adding ` 2`, ` 3`, … and cutting the
/// name so the result stays within `max` characters. `name` is already
/// clean.
#[must_use]
pub fn unique(name: &str, max: usize, taken: &std::collections::BTreeSet<String>) -> String {
    let key = |s: &str| s.to_lowercase();
    let base = if name.is_empty() { "?" } else { name };
    if !taken.contains(&key(base)) {
        return base.to_string();
    }
    for n in 2u64.. {
        let suffix = format!(" {n}");
        let keep = max.saturating_sub(suffix.chars().count());
        let stem: String = base.chars().take(keep).collect();
        let candidate = format!("{}{suffix}", stem.trim_end());
        if !taken.contains(&key(&candidate)) {
            return candidate;
        }
    }
    base.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printable_cp1252_high_characters_survive_and_c1_controls_do_not() {
        let mut f = Fields::default();
        let s = f.clean("€ “quoted” – … ’", FieldKind::Text, 100);
        assert_eq!(f.unmappable, 0, "{s}");
        assert_eq!(
            encode(&s),
            vec![
                0x80, b' ', 0x93, b'q', b'u', b'o', b't', b'e', b'd', 0x94, b' ', 0x96, b' ', 0x85,
                b' ', 0x92
            ]
        );
        let s = f.clean("a\u{0085}b\u{0080}c", FieldKind::Text, 100);
        assert_eq!(s, "a?b?c");
        assert_eq!(f.unmappable, 2);
        let s = f.clean("café 😀 Ω", FieldKind::Text, 100);
        assert_eq!(s, "café ? ?");
        assert_eq!(f.unmappable, 4);
        assert_eq!(encode("é"), vec![0xE9]);
    }

    #[test]
    fn every_high_byte_reads_back() {
        for b in 0x80u8..=0x9F {
            if let Some(c) = cp1252_char(b) {
                assert_eq!(cp1252_byte(c), Some(b));
            }
        }
        assert_eq!(cp1252_char(0x81), None);
        assert_eq!(cp1252_char(0x41), Some('A'));
    }

    #[test]
    fn separators_and_quotes_never_reach_a_field() {
        let mut f = Fields::default();
        assert_eq!(
            f.clean(" Tab\there\r\nquote\"colon:x ", FieldKind::ListName, 100),
            "Tab here  quote colon-x"
        );
        assert_eq!(f.clean("colon:kept", FieldKind::Text, 100), "colon:kept");
        assert_eq!(f.clean("abcdef  gh", FieldKind::Text, 7), "abcdef");
    }

    #[test]
    fn names_are_made_unique_within_the_limit() {
        let taken: std::collections::BTreeSet<String> =
            ["acme".into(), "acme 2".into(), "abc".into()].into();
        assert_eq!(unique("ACME", 41, &taken), "ACME 3");
        assert_eq!(unique("Other", 41, &taken), "Other");
        assert_eq!(unique("abc", 4, &taken), "ab 2");
    }
}
