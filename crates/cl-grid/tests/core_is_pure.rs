//! `cl-grid::core` must stay egui-free (P10 plan §1, Q-P10b): the state
//! machine, viewport maths and key mapping are tested without a renderer,
//! and the egui view only adapts them.

use std::path::{Path, PathBuf};

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read core dir") {
        let p = entry.expect("dir entry").path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
}

/// Code with `//` comments removed (strings are kept; a string naming egui
/// would be flagged too, which is fine for this crate).
fn code_only(src: &str) -> String {
    src.lines()
        .map(|l| l.split_once("//").map_or(l, |(code, _)| code))
        .collect::<Vec<_>>()
        .join("\n")
}

fn names_egui(code: &str) -> bool {
    code.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .any(|w| matches!(w, "egui" | "epaint" | "emath" | "eframe" | "accesskit"))
}

#[test]
fn core_never_names_egui() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/core");
    let mut files = Vec::new();
    rust_files(&dir, &mut files);
    assert!(!files.is_empty(), "src/core has no Rust files");
    let offenders: Vec<String> = files
        .iter()
        .filter(|f| names_egui(&code_only(&std::fs::read_to_string(f).expect("read"))))
        .map(|f| f.display().to_string())
        .collect();
    assert!(offenders.is_empty(), "egui named in core: {offenders:?}");
}

#[test]
fn the_check_itself_works() {
    assert!(names_egui("use egui::Ui;"));
    assert!(names_egui("fn f(x: epaint::Color32) {}"));
    assert!(!names_egui(&code_only(
        "// egui is not used here\nfn f() {}"
    )));
    assert!(!names_egui("fn regui() {} let egui_like = 1;"));
}
