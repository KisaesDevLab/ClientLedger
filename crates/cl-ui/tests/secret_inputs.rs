//! Lint: a text field that takes a secret must be a password field, so egui
//! masks it on screen and in the accessibility tree (P07 review M1).
//!
//! Any `TextEdit::` in `cl-ui/src` whose surrounding lines mention a secret
//! word must also say `.password(true)`.

use std::path::Path;

const SECRET_WORDS: [&str; 7] = [
    "password",
    "passphrase",
    "secret",
    "api_key",
    "apikey",
    "recovery_key",
    "token",
];

fn scan(dir: &Path, out: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            scan(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let text = std::fs::read_to_string(&path).unwrap();
            let lines: Vec<&str> = text.lines().collect();
            for (i, line) in lines.iter().enumerate() {
                if !line.contains("TextEdit::") {
                    continue;
                }
                let from = i.saturating_sub(3);
                let to = (i + 7).min(lines.len());
                let window = lines[from..to]
                    .join("\n")
                    .to_ascii_lowercase()
                    .replace("tokens", "");
                let mentions = SECRET_WORDS.iter().any(|w| window.contains(w));
                if mentions && !window.contains("password(true)") {
                    out.push(format!("{}:{}", path.display(), i + 1));
                }
            }
        }
    }
}

#[test]
fn secret_text_fields_are_password_fields() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut bad = Vec::new();
    scan(&src, &mut bad);
    assert!(
        bad.is_empty(),
        "these text fields look like secret inputs but are not `.password(true)`: {bad:?}"
    );
}
