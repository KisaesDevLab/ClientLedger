//! `cargo xtask nofloat`: money crates must not use floating point (B5).
//!
//! A `syn` scan of `crates/cl-core/src`, `crates/cl-api/src`, `crates/cl-model/src`
//! and `crates/cl-grid/src/core` (not `tests/`; the last two since P10, Q-P10b).
//! It reports, one finding per source line:
//!
//! 1. an `f32`/`f64` identifier (types, paths, `as f64` casts),
//! 2. a float literal (`1.5`, `1e3`, `2f64`),
//! 3. an identifier like `to_f64`, `from_f64`, `as_f64`, `from_f32_retain`,
//! 4. `ToPrimitive` / `FromPrimitive`.
//!
//! Comments and strings are never tokens, so prose such as "12.50" is fine.
//! Macro bodies are opaque to `syn`, so their token streams are walked too.
//! Exceptions live in `xtask/nofloat-allow.txt` as `path:line:reason`.

use anyhow::{Context, Result, bail};
use proc_macro2::{TokenStream, TokenTree};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use syn::visit::Visit;

/// Directories scanned, relative to the repo root.
pub const SCAN_DIRS: [&str; 4] = [
    "crates/cl-core/src",
    "crates/cl-api/src",
    // Entry rules (amounts, calculator, dates) and the grid core (Q-P10b).
    "crates/cl-model/src",
    "crates/cl-grid/src/core",
];

/// One offending line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub line: usize,
    pub message: String,
}

fn is_float_ident(s: &str) -> Option<&'static str> {
    if s == "f32" || s == "f64" {
        return Some("floating-point type");
    }
    if s == "ToPrimitive" || s == "FromPrimitive" {
        return Some("float conversion trait");
    }
    for prefix in ["to_", "from_", "as_"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            let rest = rest.strip_suffix("_retain").unwrap_or(rest);
            if rest == "f32" || rest == "f64" {
                return Some("float conversion method");
            }
        }
    }
    None
}

/// Integer literal suffixes.
const INT_SUFFIXES: [&str; 12] = [
    "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize",
];

/// Does the literal token text denote a float (`1.5`, `1e3`, `2f64`)?
fn is_float_literal_text(t: &str) -> bool {
    let b = t.as_bytes();
    if b.first().is_none_or(|c| !c.is_ascii_digit()) {
        return false; // strings, chars, byte strings
    }
    if t.ends_with("f32") || t.ends_with("f64") {
        return true;
    }
    if t.starts_with("0x") || t.starts_with("0b") || t.starts_with("0o") {
        return false;
    }
    // Integer suffixes (`1usize` has an `e` but is no float).
    if INT_SUFFIXES.iter().any(|s| t.ends_with(s)) {
        return false;
    }
    t.contains('.') || t.contains('e') || t.contains('E')
}

#[derive(Default)]
struct Scan {
    found: BTreeMap<usize, String>,
}

impl Scan {
    fn add(&mut self, line: usize, msg: String) {
        self.found.entry(line).or_insert(msg);
    }

    fn tokens(&mut self, ts: TokenStream) {
        for tt in ts {
            match tt {
                TokenTree::Group(g) => self.tokens(g.stream()),
                TokenTree::Ident(i) => {
                    if let Some(what) = is_float_ident(&i.to_string()) {
                        self.add(i.span().start().line, format!("{what} `{i}` in a macro"));
                    }
                }
                TokenTree::Literal(l) => {
                    let text = l.to_string();
                    if is_float_literal_text(&text) {
                        self.add(
                            l.span().start().line,
                            format!("float literal `{text}` in a macro"),
                        );
                    }
                }
                TokenTree::Punct(_) => {}
            }
        }
    }
}

impl<'ast> Visit<'ast> for Scan {
    fn visit_ident(&mut self, i: &'ast proc_macro2::Ident) {
        if let Some(what) = is_float_ident(&i.to_string()) {
            self.add(i.span().start().line, format!("{what} `{i}`"));
        }
    }

    fn visit_lit_float(&mut self, l: &'ast syn::LitFloat) {
        self.add(
            l.span().start().line,
            format!("float literal `{}`", l.base10_digits()),
        );
    }

    fn visit_lit_int(&mut self, l: &'ast syn::LitInt) {
        if matches!(l.suffix(), "f32" | "f64") {
            self.add(
                l.span().start().line,
                format!("float literal `{}`", l.base10_digits()),
            );
        }
    }

    fn visit_macro(&mut self, m: &'ast syn::Macro) {
        syn::visit::visit_path(self, &m.path);
        self.tokens(m.tokens.clone());
    }
}

/// Scan one file's text.
///
/// # Errors
/// The source does not parse as Rust.
pub fn scan_source(src: &str) -> Result<Vec<Finding>> {
    let file = syn::parse_file(src).context("could not parse the file")?;
    let mut s = Scan::default();
    s.visit_file(&file);
    Ok(s.found
        .into_iter()
        .map(|(line, message)| Finding { line, message })
        .collect())
}

/// Parsed `nofloat-allow.txt`: `(path, line)` pairs.
///
/// # Errors
/// A line is malformed or has no reason.
pub fn parse_allowlist(text: &str) -> Result<Vec<(String, usize)>> {
    let mut out = Vec::new();
    for (n, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(3, ':');
        let (Some(path), Some(num), Some(reason)) = (parts.next(), parts.next(), parts.next())
        else {
            bail!(
                "nofloat-allow.txt line {}: expected path:line:reason",
                n + 1
            );
        };
        let num: usize = num
            .trim()
            .parse()
            .with_context(|| format!("nofloat-allow.txt line {}: bad line number", n + 1))?;
        if reason.trim().is_empty() {
            bail!(
                "nofloat-allow.txt line {}: every entry needs a reason",
                n + 1
            );
        }
        out.push((path.trim().to_string(), num));
    }
    Ok(out)
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let p = entry?.path();
        if p.is_dir() {
            rust_files(&p, out)?;
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
    Ok(())
}

/// Scan the repo at `root` and return one message per violation.
///
/// # Errors
/// I/O, parse, or allowlist-format errors.
pub fn check(root: &Path) -> Result<Vec<String>> {
    let allow_path = root.join("xtask/nofloat-allow.txt");
    let allow = match std::fs::read_to_string(&allow_path) {
        Ok(t) => parse_allowlist(&t)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e).context("reading nofloat-allow.txt"),
    };
    let mut out = Vec::new();
    for dir in SCAN_DIRS {
        let base = root.join(dir);
        if !base.is_dir() {
            continue;
        }
        let mut files = Vec::new();
        rust_files(&base, &mut files)?;
        files.sort();
        for f in files {
            let rel = f
                .strip_prefix(root)
                .unwrap_or(&f)
                .to_string_lossy()
                .replace('\\', "/");
            let src = std::fs::read_to_string(&f).with_context(|| format!("reading {rel}"))?;
            let found = scan_source(&src).with_context(|| format!("scanning {rel}"))?;
            for fi in found {
                if !allow.iter().any(|(p, l)| *p == rel && *l == fi.line) {
                    out.push(format!(
                        "{rel}:{}: {} (money must be exact)",
                        fi.line, fi.message
                    ));
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(src: &str) -> Vec<usize> {
        scan_source(src)
            .unwrap()
            .into_iter()
            .map(|f| f.line)
            .collect()
    }

    #[test]
    fn bad_fixture_reports_exactly_the_five_lines() {
        let found = scan_source(include_str!("../tests/fixtures/nofloat_bad.rs")).unwrap();
        assert_eq!(
            found.iter().map(|f| f.line).collect::<Vec<_>>(),
            [2, 5, 6, 7, 8],
            "{found:?}"
        );
    }

    #[test]
    fn good_fixture_is_clean() {
        let found = scan_source(include_str!("../tests/fixtures/nofloat_good.rs")).unwrap();
        assert_eq!(found, Vec::<Finding>::new());
    }

    #[test]
    fn single_constructs_are_caught() {
        assert_eq!(lines("fn f(d: D) { d.to_f64(); }"), [1]);
        assert_eq!(lines("fn f() { let x = 1.5; }"), [1]);
        assert_eq!(lines("fn f(x: i64) { let _ = x as f64; }"), [1]);
        assert_eq!(lines("fn f(x: i64) { let _ = x as f32; }"), [1]);
        assert_eq!(lines("fn f() { let x = 2f64; }"), [1]);
        assert_eq!(lines("fn f() { let x = 1e3; }"), [1]);
        assert_eq!(lines("struct S { v: f64 }"), [1]);
        assert_eq!(lines("fn f() { D::from_f32_retain(1); }"), [1]);
        assert_eq!(lines("use num_traits::FromPrimitive;"), [1]);
    }

    #[test]
    fn floats_in_macro_bodies_are_caught() {
        assert_eq!(
            lines("fn f(d: D) {\n let _ = vec![\n d.to_f64(),\n ];\n}"),
            [3]
        );
        assert_eq!(lines("fn f() { let _ = json!({ \"a\": 1.5 }); }"), [1]);
        assert_eq!(lines("fn f() { let _ = format!(\"{}\", x as f64); }"), [1]);
        assert_eq!(lines("fn f() { assert_eq!(a, 3f32); }"), [1]);
        assert_eq!(
            lines("fn f() { let _ = vec![1usize, 2isize, 3e2]; assert_eq!(n, 1usize); }"),
            [1],
            "only 3e2 is a float"
        );
        assert_eq!(
            lines("fn f() { assert_eq!(n, 1usize); }"),
            Vec::<usize>::new()
        );
    }

    #[test]
    fn comments_strings_and_other_numbers_are_ignored() {
        let src = "//! to_f64 12.50\n/// 12.50 and f64\nfn f() {\n // x as f64\n let _ = \"1.5 f64 to_f64\";\n let _ = 0x1e3; let _ = 12; let _ = 1_000u64;\n let _ = format!(\"{}\", \"2.5\");\n}\n";
        assert_eq!(scan_source(src).unwrap(), Vec::<Finding>::new());
    }

    #[test]
    fn similar_names_are_not_flagged() {
        assert_eq!(
            lines("fn to_f6() {} fn from_f640() {} fn tof64() {}"),
            Vec::<usize>::new()
        );
    }

    #[test]
    fn allowlist_needs_a_reason() {
        assert_eq!(
            parse_allowlist("# c\n\ncrates/cl-core/src/a.rs:3:egui boundary\n").unwrap(),
            [("crates/cl-core/src/a.rs".to_string(), 3)]
        );
        assert!(parse_allowlist("a.rs:3:").is_err());
        assert!(parse_allowlist("a.rs:3").is_err());
        assert!(parse_allowlist("a.rs:x:why").is_err());
    }

    #[test]
    fn check_honours_the_allowlist() {
        let dir = std::env::temp_dir().join(format!("nofloat-test-{}", std::process::id()));
        let src = dir.join("crates/cl-core/src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(dir.join("xtask")).unwrap();
        std::fs::write(src.join("a.rs"), "fn f() {\n let x = 1.5;\n}\n").unwrap();
        let v = check(&dir).unwrap();
        assert_eq!(v.len(), 1);
        assert!(v[0].starts_with("crates/cl-core/src/a.rs:2:"), "{v:?}");
        std::fs::write(
            dir.join("xtask/nofloat-allow.txt"),
            "crates/cl-core/src/a.rs:2:test only\n",
        )
        .unwrap();
        assert_eq!(check(&dir).unwrap(), Vec::<String>::new());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn entry_rules_and_grid_core_are_scanned() {
        assert!(SCAN_DIRS.contains(&"crates/cl-model/src"));
        assert!(SCAN_DIRS.contains(&"crates/cl-grid/src/core"));
    }

    #[test]
    fn the_real_money_crates_are_clean() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        assert_eq!(check(&root).unwrap(), Vec::<String>::new());
    }
}
