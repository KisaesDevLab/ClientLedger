//! `cargo xtask unsafe-check`: `ClientLedger`'s own code has exactly one
//! `unsafe` boundary (A03, Q-A03s): the Windows job limits for the reading
//! worker and tesseract (memory cap, one process, no crash reporting) in
//! [`ALLOWED`], one `SetInformationJobObject` call with a `// SAFETY:`
//! comment. Adding limits to that call must not add a second block.
//!
//! The workspace lint `unsafe_code = "forbid"` can't be relaxed inside a
//! crate, so cl-host denies it instead and that one module allows it. This
//! check keeps the boundary that small:
//!
//! 1. no `unsafe` token (blocks, functions, impls, traits, extern blocks,
//!    macro bodies) in any `.rs` file under `crates/`, `apps/` or `xtask/`
//!    except [`ALLOWED`]; comments and strings are not tokens;
//! 2. no `allow(unsafe_code)` / `expect(unsafe_code)` outside it;
//! 3. [`ALLOWED`] holds exactly one `unsafe` block, preceded by a
//!    `// SAFETY:` comment;
//! 4. every package keeps `[lints] workspace = true`, except cl-host, which
//!    must say `unsafe_code = "deny"`.

use anyhow::{Context, Result};
use proc_macro2::{TokenStream, TokenTree};
use std::path::{Path, PathBuf};

/// The one file that may contain `unsafe`.
pub const ALLOWED: &str = "crates/cl-host/src/procs/win_job_memory.rs";

/// The package that replaces the workspace lints (and must deny unsafe).
const OWN_LINTS: &str = "crates/cl-host/Cargo.toml";

/// Where the `unsafe` and `allow(unsafe_code)` tokens are in `src`
/// (1-based lines).
#[must_use]
pub fn unsafe_tokens(src: &str) -> (Vec<usize>, Vec<usize>) {
    let Ok(ts) = src.parse::<TokenStream>() else {
        // Unparsable: report line 0 so it is never silently skipped.
        return (vec![0], Vec::new());
    };
    let mut unsafe_lines = Vec::new();
    let mut allow_lines = Vec::new();
    walk(ts, &mut unsafe_lines, &mut allow_lines);
    (unsafe_lines, allow_lines)
}

fn walk(ts: TokenStream, unsafe_lines: &mut Vec<usize>, allow_lines: &mut Vec<usize>) {
    let mut prev_ident: Option<String> = None;
    for tt in ts {
        match tt {
            TokenTree::Ident(i) => {
                if i == "unsafe" {
                    unsafe_lines.push(i.span().start().line);
                }
                prev_ident = Some(i.to_string());
            }
            TokenTree::Group(g) => {
                if matches!(prev_ident.as_deref(), Some("allow" | "expect"))
                    && g.stream()
                        .into_iter()
                        .any(|t| matches!(&t, TokenTree::Ident(i) if i == "unsafe_code"))
                {
                    allow_lines.push(g.span().start().line);
                }
                walk(g.stream(), unsafe_lines, allow_lines);
                prev_ident = None;
            }
            _ => prev_ident = None,
        }
    }
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let p = entry?.path();
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if p.is_dir() {
            if name != "target" && !name.starts_with('.') {
                rust_files(&p, out)?;
            }
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
    Ok(())
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Check the allowed file's content: one `unsafe` block with a `SAFETY`
/// comment in the comment block right above it.
#[must_use]
pub fn allowed_file_problems(src: &str) -> Vec<String> {
    let (lines, _) = unsafe_tokens(src);
    let mut out = Vec::new();
    if lines.len() != 1 {
        out.push(format!(
            "{ALLOWED}: expected exactly one `unsafe`, found {}",
            lines.len()
        ));
    }
    let text: Vec<&str> = src.lines().collect();
    for l in lines {
        // The comment block right above the `unsafe` line (1-based `l`).
        let above = text[..l.saturating_sub(1).min(text.len())]
            .iter()
            .rev()
            .take_while(|t| t.trim_start().starts_with("//"));
        let has_safety = above
            .into_iter()
            .any(|t| t.trim_start().starts_with("// SAFETY:"));
        if !has_safety {
            out.push(format!(
                "{ALLOWED}:{l}: `unsafe` without a `// SAFETY:` comment above it"
            ));
        }
    }
    out
}

/// Every violation in the repository at `root`.
///
/// # Errors
/// A folder or file could not be read.
pub fn check(root: &Path) -> Result<Vec<String>> {
    let mut files = Vec::new();
    for d in ["crates", "apps", "xtask"] {
        rust_files(&root.join(d), &mut files)?;
    }
    let mut out = Vec::new();
    for f in &files {
        let r = rel(root, f);
        let src = std::fs::read_to_string(f).with_context(|| format!("read {r}"))?;
        if r == ALLOWED {
            out.extend(allowed_file_problems(&src));
            continue;
        }
        let (unsafe_lines, allow_lines) = unsafe_tokens(&src);
        for l in unsafe_lines {
            out.push(format!("{r}:{l}: `unsafe` outside {ALLOWED}"));
        }
        for l in allow_lines {
            out.push(format!("{r}:{l}: unsafe code allowed outside {ALLOWED}"));
        }
    }
    for d in ["crates", "apps"] {
        for entry in std::fs::read_dir(root.join(d))? {
            let manifest = entry?.path().join("Cargo.toml");
            if manifest.is_file() {
                out.extend(manifest_problem(root, &manifest)?);
            }
        }
    }
    out.extend(manifest_problem(root, &root.join("xtask/Cargo.toml"))?);
    Ok(out)
}

fn manifest_problem(root: &Path, manifest: &Path) -> Result<Option<String>> {
    let r = rel(root, manifest);
    let text = std::fs::read_to_string(manifest).with_context(|| format!("read {r}"))?;
    let compact: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if r == OWN_LINTS {
        let ok = compact.contains("[lints.rust] unsafe_code = \"deny\"");
        return Ok((!ok).then(|| format!("{r}: must set [lints.rust] unsafe_code = \"deny\"")));
    }
    let ok = compact.contains("[lints] workspace = true");
    Ok((!ok).then(|| format!("{r}: must keep [lints] workspace = true (unsafe_code = forbid)")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_unsafe_tokens_but_not_words() {
        let src = "// unsafe in a comment\nfn a() { let s = \"unsafe\"; }\nunsafe fn b() {}\nfn c() { unsafe { b() } }\nmacro_rules! m { () => { unsafe {} } }\n";
        assert_eq!(unsafe_tokens(src).0, [3, 4, 5]);
        let attrs = "#![forbid(unsafe_code)]\n#[allow(dead_code, unsafe_code)]\nmod x {}\n#![expect(unsafe_code)]\n";
        assert_eq!(unsafe_tokens(attrs).1, [2, 4]);
    }

    #[test]
    fn the_allowed_file_needs_one_commented_block() {
        let good =
            "fn f() {\n    // SAFETY: the handle is valid.\n    let ok = unsafe { g() };\n}\n";
        assert_eq!(allowed_file_problems(good), Vec::<String>::new());
        let bare = "fn f() {\n    let ok = unsafe { g() };\n}\n";
        assert_eq!(allowed_file_problems(bare).len(), 1);
        let long = "fn f() {\n    // SAFETY: a\n    // b\n    // c\n    // d\n    let ok = unsafe { g() };\n}\n";
        assert_eq!(allowed_file_problems(long), Vec::<String>::new());
        let detached = "// SAFETY: far away\nfn f() {\n    let ok = unsafe { g() };\n}\n";
        assert_eq!(allowed_file_problems(detached).len(), 1);
        let two = "fn f() {\n    // SAFETY: x\n    unsafe { g() };\n    // SAFETY: y\n    unsafe { g() };\n}\n";
        assert_eq!(allowed_file_problems(two).len(), 1);
    }

    #[test]
    fn the_repository_has_only_the_one_boundary() {
        let root = crate::engine::repo_root().unwrap();
        assert_eq!(check(&root).unwrap(), Vec::<String>::new());
    }
}
