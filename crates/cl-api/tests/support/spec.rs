//! Engine spec fixtures (P05 plan §2.8, B6): the canonical `OpenAPI` hash,
//! the covered-operations table and the hashes of the engine's Zod sources.
//!
//! This file is the single implementation. It is compiled into the cl-api
//! tests (`#[path]`) and into `cargo xtask api-spec`, so the fixtures the
//! tool writes are exactly what the tests recompute. It only depends on
//! `serde_json` and `sha2`.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use serde_json::Value;
use sha2::{Digest, Sha256};

/// Operations that the engine serves but does not list in its `OpenAPI`
/// document: the desktop gate and the spec route itself, the desktop
/// session-secrets routes, and the documented-but-unmerged P08 contracts
/// (E5b safe delete, U-23 recovery key) that the desktop-mode export
/// (U-24) will add. Since pin 5c8e9665 (engine PRs #151-#153) the tenant
/// and archive routes, the usage summary, the closing password and
/// `DELETE /transactions/{id}` are in the spec.
pub const OUTSIDE_SPEC: [(&str, &str); 8] = [
    ("GET", "/api/v1/desktop/info"),
    ("GET", "/openapi.json"),
    ("POST", "/api/v1/tenants/{id}/delete"),
    ("POST", "/api/v1/desktop/recover-password"),
    ("POST", "/api/v1/auth/recovery-key/rotate"),
    // E6 (P09): session secrets.
    ("PUT", "/api/v1/desktop/session-secrets"),
    ("GET", "/api/v1/desktop/session-secrets"),
    ("DELETE", "/api/v1/desktop/session-secrets/{name}"),
];

/// Lowercase hex of a SHA-256.
pub fn sha_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for b in digest {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// Canonical text of a JSON value: object keys sorted, no whitespace.
pub fn canonical(v: &Value) -> String {
    let mut out = String::new();
    write_canonical(v, &mut out);
    out
}

fn write_canonical(v: &Value, out: &mut String) {
    match v {
        Value::Object(map) => {
            let sorted: BTreeMap<&String, &Value> = map.iter().collect();
            out.push('{');
            for (i, (k, val)) in sorted.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String((*k).clone()).to_string());
                out.push(':');
                write_canonical(val, out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, val) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(val, out);
            }
            out.push(']');
        }
        scalar => out.push_str(&scalar.to_string()),
    }
}

/// Hash of the canonical spec.
pub fn spec_hash(spec: &Value) -> String {
    sha_hex(canonical(spec).as_bytes())
}

/// The covered operations (`METHOD route`) from `operations.txt`.
pub fn operations(txt: &str) -> Vec<(String, String)> {
    txt.lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            Some((it.next()?.to_string(), it.next()?.to_string()))
        })
        .collect()
}

/// Engine source files behind the covered request shapes: the
/// `#! schema-source <path>` header lines of `operations.txt`.
pub fn schema_sources(txt: &str) -> Vec<String> {
    txt.lines()
        .filter_map(|l| l.strip_prefix("#! schema-source "))
        .map(|p| p.trim().to_string())
        .collect()
}

fn resolve(v: &Value, spec: &Value, depth: u32) -> Value {
    match v {
        Value::Object(map) => {
            if let Some(Value::String(r)) = map.get("$ref") {
                if depth < 12
                    && let Some(target) = r.strip_prefix('#').and_then(|p| spec.pointer(p))
                {
                    return resolve(target, spec, depth + 1);
                }
                return Value::String(format!("$ref:{r}"));
            }
            Value::Object(
                map.iter()
                    .map(|(k, val)| (k.clone(), resolve(val, spec, depth)))
                    .collect(),
            )
        }
        Value::Array(items) => {
            Value::Array(items.iter().map(|i| resolve(i, spec, depth)).collect())
        }
        other => other.clone(),
    }
}

/// The request-shaping part of an operation in the spec.
pub enum Fragment {
    /// The operation is not in the spec.
    NotInSpec,
    /// It documents neither parameters nor a request body (the
    /// money-critical operations today).
    Empty,
    /// Its parameters and request body, with `$ref`s inlined.
    Documented(Value),
}

pub fn fragment(spec: &Value, method: &str, route: &str) -> Fragment {
    let Some(op) = spec
        .get("paths")
        .and_then(|p| p.get(route))
        .and_then(|p| p.get(method.to_ascii_lowercase()))
    else {
        return Fragment::NotInSpec;
    };
    let mut frag = serde_json::Map::new();
    for key in ["parameters", "requestBody"] {
        if let Some(v) = op.get(key) {
            let empty = v.as_array().is_some_and(Vec::is_empty);
            if !empty {
                frag.insert(key.to_string(), resolve(v, spec, 0));
            }
        }
    }
    if frag.is_empty() {
        Fragment::Empty
    } else {
        Fragment::Documented(Value::Object(frag))
    }
}

/// Whether the operation is in the spec and the hash of its fragment
/// (`None` when it documents no parameters and no request body).
pub fn covered_entry(spec: &Value, method: &str, route: &str) -> (bool, Option<String>) {
    match fragment(spec, method, route) {
        Fragment::NotInSpec => (false, None),
        Fragment::Empty => (true, None),
        Fragment::Documented(f) => (true, Some(sha_hex(canonical(&f).as_bytes()))),
    }
}

/// Text of `covered_operations.json`, one operation per line.
pub fn covered_text(spec: &Value, ops: &[(String, String)]) -> String {
    let mut text = String::from("[\n");
    for (i, (m, r)) in ops.iter().enumerate() {
        let (in_spec, hash) = covered_entry(spec, m, r);
        let hash = hash.map_or_else(|| "null".to_string(), |h| format!("\"{h}\""));
        let comma = if i + 1 < ops.len() { "," } else { "" };
        let _ = writeln!(
            text,
            "  {{\"method\":\"{m}\",\"path\":\"{r}\",\"inSpec\":{in_spec},\"fragmentHash\":{hash}}}{comma}"
        );
    }
    text.push_str("]\n");
    text
}

/// Text of `openapi.sha256`.
pub fn openapi_text(spec: &Value) -> String {
    format!("{}\n", spec_hash(spec))
}

/// Hash of one engine source file, line endings normalized to `\n` so a
/// Windows checkout hashes like Linux.
pub fn source_hash(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let text = String::from_utf8_lossy(&bytes).replace("\r\n", "\n");
    Ok(sha_hex(text.as_bytes()))
}

/// Text of `engine_schemas.sha256` (`<hash>  <path>` per source file).
pub fn schemas_text(repo_root: &Path, sources: &[String]) -> Result<String, String> {
    let mut text = String::new();
    for s in sources {
        let h = source_hash(&repo_root.join(s))?;
        let _ = writeln!(text, "{h}  {s}");
    }
    Ok(text)
}

/// Everything the fixtures contain, computed from the engine checkout.
pub struct Fixtures {
    pub openapi: String,
    pub covered: String,
    pub schemas: String,
}

/// Recompute the three fixtures from `<repo_root>/engine`.
pub fn compute(repo_root: &Path, operations_txt: &str) -> Result<Fixtures, String> {
    let spec_path = repo_root.join("engine").join("openapi.json");
    let raw = std::fs::read(&spec_path).map_err(|e| format!("{}: {e}", spec_path.display()))?;
    let spec: Value = serde_json::from_slice(&raw).map_err(|e| format!("openapi.json: {e}"))?;
    Ok(Fixtures {
        openapi: openapi_text(&spec),
        covered: covered_text(&spec, &operations(operations_txt)),
        schemas: schemas_text(repo_root, &schema_sources(operations_txt))?,
    })
}
