//! Every stale-checked write sends its stamp (P33, engine U-291/U-292).
//!
//! In LAN mode the engine refuses a write the route table classifies as
//! `x-stale: stale(*)` or `bulk` when it carries no stamp (428
//! `STALE_REQUIRED`). `tests/fixtures/stale_routes.txt` lists the covered
//! ones (generated from the pinned spec by `route_classes.rs`). This test
//! reads cl-api's own source and fails when:
//!
//! - a listed route has no client method at all (its operation constant
//!   is never used outside `operations.rs`);
//! - a function that sends to a listed route neither takes a stamp from
//!   its caller (a `RecordVersion`, `RecordStamp`, `Timestamp`, `version`
//!   or write-options parameter) nor puts one in the request itself;
//! - a stamp goes where the route does not read it: a `DELETE` whose
//!   handler reads the body (the engine's `expectStamp`, and `DELETE
//!   /transactions/{id}`, which reads only the body; review S3) must not
//!   send `expectedUpdatedAt` in the query, and the routes whose handler
//!   parses the query must send it there.
//!
//! The wire placement of each stamp helper is tested against a recording
//! mock in the lib tests (`resources::stale_guard_tests`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const STALE_ROUTES: &str = include_str!("fixtures/stale_routes.txt");
const OPERATIONS_RS: &str = include_str!("../src/resources/operations.rs");

/// `DELETE` routes whose handler parses the stamp from the query string.
const QUERY_READERS: &[&str] = &[
    "/api/v1/vendor-layout-templates/{id}",
    "/api/v1/budgets/{id}",
    "/api/v1/report-builder/definitions/{id}",
    "/api/v1/report-builder/definition-groups/{id}",
    "/api/v1/templates/{id}",
];

/// Functions that send to a stale route but get their stamp inside an
/// argument their callers build (checked by name: every caller is listed
/// here with the reason).
const STAMPED_BY_CALLER: &[(&str, &str)] = &[
    (
        "document_drafts.rs::put",
        "private; its callers put `expectedUpdatedAt` (their `expected_updated_at`) in the body",
    ),
    (
        "vendor_templates.rs::put",
        "`TemplateUpsert.expected_updated_at` (null only on create: engine follow-up U-292b)",
    ),
];

/// Signature words that mean the caller hands the stamp in.
const STAMP_PARAMS: &[&str] = &[
    "RecordVersion",
    "RecordStamp",
    "Timestamp",
    "version: u",
    "WriteOptions",
    "expected_updated_at",
    "expected:",
    "Versioned<",
];

/// Body words that mean the function stamps the request itself.
const STAMP_BODY: &[&str] = &[
    "stamp_body(",
    "stamp_delete(",
    "stamp_items(",
    "update_guarded",
    "company_row_body(",
    "company_row_stamp(",
    "\"expectedUpdatedAt\"",
    "\"version\"",
    "with_version(",
    "ap_body(",
    "r.stamp()",
];

/// Operation constant name -> (METHOD, route).
fn operations() -> BTreeMap<String, (String, String)> {
    let mut out = BTreeMap::new();
    for chunk in OPERATIONS_RS.split("const ").skip(1) {
        let Some((name, rest)) = chunk.split_once(": Operation =") else {
            continue;
        };
        let Some(def) = rest.split(';').next() else {
            continue;
        };
        let Some(route) = def.split('"').nth(1).filter(|r| r.starts_with('/')) else {
            continue;
        };
        let method = def.split("Method::").nth(1).map_or("GET".to_string(), |m| {
            m.chars()
                .take_while(char::is_ascii_alphabetic)
                .collect::<String>()
                .to_ascii_uppercase()
        });
        out.insert(name.trim().to_string(), (method, route.to_string()));
    }
    out
}

fn sources() -> Vec<(String, String)> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).expect("read src").flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    let mut files = Vec::new();
    walk(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    files
        .into_iter()
        .filter_map(|p| {
            let name = p.file_name()?.to_string_lossy().to_string();
            if name.ends_with("_tests.rs") || name == "operations.rs" || name == "testkit.rs" {
                return None;
            }
            let text = std::fs::read_to_string(&p).ok()?.replace("\r\n", "\n");
            // Inline test modules are not client code.
            let code = text
                .split("\n#[cfg(test)]\nmod tests")
                .next()
                .and_then(|t| t.split("\n#[cfg(test)]\npub(crate) mod tests").next())
                .unwrap_or("")
                .to_string();
            Some((name, code))
        })
        .collect()
}

fn is_word(text: &str, at: usize, word: &str) -> bool {
    let before = text[..at].chars().next_back();
    let after = text[at + word.len()..].chars().next();
    let ident = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
    !ident(before) && !ident(after)
}

/// The function around byte `at`: (name, signature, body).
fn enclosing_fn(text: &str, at: usize) -> Option<(String, String, String)> {
    let head = &text[..at];
    let start = head
        .match_indices("fn ")
        .filter(|(i, _)| {
            let line_start = head[..*i].rfind('\n').map_or(0, |n| n + 1);
            let lead = head[line_start..*i].trim();
            lead.is_empty() || lead.ends_with("async") || lead.starts_with("pub") || lead == "const"
        })
        .map(|(i, _)| i)
        .last()?;
    let name: String = text[start + 3..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    let open = start + text[start..].find(" {\n")? + 1;
    let mut depth = 0usize;
    let mut end = text.len();
    for (i, c) in text[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = open + i + 1;
                    break;
                }
            }
            _ => {}
        }
    }
    if end < at {
        return None;
    }
    Some((
        name,
        text[start..open].to_string(),
        text[open..end].to_string(),
    ))
}

/// Every function that sends to `op`, by `file::fn`: direct uses of the
/// constant, plus (for operations reached through a resource `Spec`) the
/// functions of that file calling the shared `common::` writers.
fn senders(op: &str, files: &[(String, String)]) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for (file, text) in files {
        for (at, _) in text.match_indices(op) {
            if !is_word(text, at, op) {
                continue;
            }
            let line_start = text[..at].rfind('\n').map_or(0, |n| n + 1);
            let line = text[line_start..].lines().next().unwrap_or("");
            // Inside a `use` statement (possibly a multi-line list).
            let in_use = text[..at]
                .rfind("use ")
                .is_some_and(|u| is_word(text, u, "use") && !text[u..at].contains(';'));
            // Not the operation: a declaration of the same name (the
            // capability `Capabilities::TXN_DELETE`), a use of that
            // capability, or text in a string.
            let declared = text[at + op.len()..].starts_with(':');
            let other = text[..at].ends_with("Capabilities::") || text[..at].ends_with('"');
            if in_use || declared || other || line.trim_start().starts_with("//") {
                continue;
            }
            if let Some((n, sig, body)) = enclosing_fn(text, at) {
                out.push((format!("{file}::{n}"), sig, body));
                continue;
            }
            // `put: &ACCOUNT_PUT,` in a resource `Spec` constant: the
            // writers are the file's `common::` calls.
            let spec_field = line.trim_start().split(':').next().unwrap_or("");
            if matches!(spec_field, "put" | "delete") && line.contains(": &") {
                let helpers: &[&str] = if spec_field == "put" {
                    &["common::update(", "SPEC.put", "update_doc("]
                } else {
                    &[
                        "common::delete(",
                        "common::deactivate(",
                        "common::delete_record(",
                        "SPEC.delete",
                    ]
                };
                for h in helpers {
                    for (hat, _) in text.match_indices(h) {
                        if let Some((n, sig, body)) = enclosing_fn(text, hat) {
                            out.push((format!("{file}::{n}"), sig, body));
                        }
                    }
                }
                continue;
            }
            // Not inside a function: a table or a constant (none expected).
            out.push((
                format!("{file}::<item at byte {at}>"),
                String::new(),
                String::new(),
            ));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out.dedup_by(|a, b| a.0 == b.0);
    out
}

#[test]
fn every_stale_route_has_a_method_and_every_method_sends_the_stamp_where_it_is_read() {
    let ops = operations();
    let files = sources();
    let mut problems = Vec::new();
    let mut checked = 0;
    for line in STALE_ROUTES
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let mut parts = line.split_whitespace();
        let (Some(method), Some(route), Some(class)) = (parts.next(), parts.next(), parts.next())
        else {
            problems.push(format!("bad fixture line: {line}"));
            continue;
        };
        let consts: Vec<&String> = ops
            .iter()
            .filter(|(_, (m, r))| m == method && r == route)
            .map(|(n, _)| n)
            .collect();
        if consts.is_empty() {
            problems.push(format!("{method} {route}: no Operation constant"));
            continue;
        }
        let mut any = false;
        for c in consts {
            for (who, sig, body) in senders(c, &files) {
                any = true;
                checked += 1;
                if STAMPED_BY_CALLER.iter().any(|(n, _)| *n == who) {
                    continue;
                }
                // The parameters only (a `Versioned<_>` answer is no stamp).
                let params = sig.split("->").next().unwrap_or("");
                let by_caller = STAMP_PARAMS.iter().any(|w| params.contains(w));
                let by_itself = if class == "bulk" {
                    body.contains("stamp_items(")
                } else {
                    STAMP_BODY.iter().any(|w| body.contains(w))
                };
                if !(by_caller || by_itself) {
                    problems.push(format!("{method} {route}: {who} sends no stamp"));
                }
                if method == "DELETE" && class == "stale(updatedAt)" {
                    let reads_query = QUERY_READERS.contains(&route);
                    let in_query = body.contains("query")
                        && body.contains("expectedUpdatedAt")
                        && !body.contains("stamp_delete(");
                    if reads_query && !in_query {
                        problems.push(format!(
                            "{method} {route}: {who} must send expectedUpdatedAt in the query (its handler reads req.query)"
                        ));
                    }
                    if !reads_query && in_query {
                        problems.push(format!(
                            "{method} {route}: {who} sends expectedUpdatedAt in the query; this route reads the body"
                        ));
                    }
                }
            }
        }
        if !any {
            problems.push(format!("{method} {route}: no client method sends to it"));
        }
    }
    assert!(
        checked > 60,
        "only {checked} senders found: the scan is broken"
    );
    assert!(
        problems.is_empty(),
        "stale-checked writes without a stamp (LAN mode answers 428 STALE_REQUIRED):\n{}",
        problems.join("\n")
    );
}

#[test]
fn the_scan_finds_a_function_that_sends_no_stamp() {
    let text = "impl X {\n    pub async fn delete(&self, id: Id) -> Result<(), E> {\n        let call = Call::new(Method::Delete, THING_DELETE.route(), p);\n        Ok(())\n    }\n}\n";
    let files = vec![("thing.rs".to_string(), text.to_string())];
    let found = senders("THING_DELETE", &files);
    assert_eq!(found.len(), 1);
    let (who, sig, body) = &found[0];
    assert_eq!(who, "thing.rs::delete");
    assert!(!STAMP_PARAMS.iter().any(|w| sig.contains(w)));
    assert!(!STAMP_BODY.iter().any(|w| body.contains(w)));
}
