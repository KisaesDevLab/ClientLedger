//! N6: outside `core`, the grid shows money only through `Money::format`
//! and never reaches into its exact representation; and a file that names
//! `Money` never casts to a float (geometry casts live in files that do not
//! handle money).

use std::path::{Path, PathBuf};

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read dir") {
        let p = entry.expect("entry").path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
}

fn code(src: &str) -> String {
    src.split("#[cfg(test)]")
        .next()
        .unwrap_or("")
        .lines()
        .map(|l| l.split_once("//").map_or(l, |(c, _)| c))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn view_and_editors_never_touch_money_internals() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    let mut problems = Vec::new();
    for f in files {
        let rel = f
            .strip_prefix(&src)
            .expect("under src")
            .to_string_lossy()
            .replace('\\', "/");
        if rel.starts_with("core/") {
            continue;
        }
        let text = code(&std::fs::read_to_string(&f).expect("read"));
        for banned in [
            "to_ten_thousandths",
            "as_decimal",
            "to_wire",
            "mul_round",
            "from_ten_thousandths",
        ] {
            if text.contains(banned) {
                problems.push(format!("{rel}: {banned}"));
            }
        }
        let names_money = text.contains("Money") && !text.contains("MoneyStyle")
            || text.contains("Money::")
            || text.contains("Money(");
        if names_money && (text.contains("as f32") || text.contains("as f64")) {
            problems.push(format!("{rel}: a float cast in a file that handles money"));
        }
    }
    assert!(problems.is_empty(), "{problems:#?}");
}
