//! Saving a PDF (Q-P19y): the default folder and file name the Save dialog
//! offers. Pure helpers: the UI owns the dialog and the writing.

use std::path::{Path, PathBuf};

/// `Documents\ClientLedger\<Company>`: `documents` is the owner's
/// Documents folder (found by the caller).
#[must_use]
pub fn default_save_dir(documents: &Path, company: &str) -> PathBuf {
    let company = safe_component(company);
    let base = documents.join("ClientLedger");
    if company.is_empty() {
        base
    } else {
        base.join(company)
    }
}

/// `Invoice 1042 - Acme.pdf`: document type, number and name, with
/// characters that file systems refuse replaced. Blank parts are left out.
#[must_use]
pub fn pdf_file_name(kind: &str, number: &str, name: &str) -> String {
    let head = [kind.trim(), number.trim()]
        .iter()
        .filter(|s| !s.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
    let full = match (head.is_empty(), name.trim().is_empty()) {
        (false, false) => format!("{head} - {}", name.trim()),
        (false, true) => head,
        (true, false) => name.trim().to_string(),
        (true, true) => "Document".to_string(),
    };
    let safe = safe_component(&full);
    format!("{}.pdf", if safe.is_empty() { "Document" } else { &safe })
}

/// Longest file-name stem kept (well under every platform's limit).
const MAX_STEM: usize = 120;

/// One path component safe on Windows, macOS and Linux: `< > : " / \ | ? *`
/// and control characters become `_`; leading and trailing dots and spaces
/// are dropped; Windows device names get a trailing `_`.
#[must_use]
pub fn safe_component(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect();
    out = out
        .trim_matches(|c: char| c == '.' || c.is_whitespace())
        .to_string();
    if out.chars().count() > MAX_STEM {
        out = out
            .chars()
            .take(MAX_STEM)
            .collect::<String>()
            .trim_end()
            .to_string();
    }
    let stem = out.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit());
    if reserved {
        out.push('_');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_combine_kind_number_and_name_safely() {
        assert_eq!(
            pdf_file_name("Invoice", "1042", "Acme"),
            "Invoice 1042 - Acme.pdf"
        );
        assert_eq!(
            pdf_file_name("Invoice", "1042", "A/B: \"Co\" <x>?"),
            "Invoice 1042 - A_B_ _Co_ _x__.pdf"
        );
        assert_eq!(
            pdf_file_name("Statement", "", "Pat Doe"),
            "Statement - Pat Doe.pdf"
        );
        assert_eq!(pdf_file_name("Checks", "1001", ""), "Checks 1001.pdf");
        assert_eq!(pdf_file_name("", "", ""), "Document.pdf");
        assert_eq!(pdf_file_name("", "", "..."), "Document.pdf");
        assert!(pdf_file_name("Invoice", "1", &"x".repeat(500)).len() < 140);
    }

    #[test]
    fn device_names_and_dots_are_neutralised() {
        assert_eq!(safe_component("CON"), "CON_");
        assert_eq!(safe_component("com1"), "com1_");
        assert_eq!(safe_component("Comfort"), "Comfort");
        assert_eq!(safe_component(" .hidden. "), "hidden");
        assert_eq!(safe_component("tab\there"), "tab_here");
    }

    #[test]
    fn the_save_folder_is_under_documents() {
        let d = default_save_dir(Path::new("docs"), "Acme: West");
        assert_eq!(d, Path::new("docs").join("ClientLedger").join("Acme_ West"));
        assert_eq!(
            default_save_dir(Path::new("docs"), " "),
            Path::new("docs").join("ClientLedger")
        );
    }
}
