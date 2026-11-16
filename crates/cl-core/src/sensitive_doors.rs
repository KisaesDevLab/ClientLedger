//! Which engine routes still refuse a viewer from whom sensitive accounts
//! are hidden (U-33, P29 review B2; capability `sensitive-accounts-doors-v1`).
//!
//! The engine answers `GET /api/v1/sensitive-doors` with
//! `{hidesSensitive, refusePrefixes[], converted[{method, path}]}`. A request
//! is refused (403 `SENSITIVE_HIDDEN`) when `hidesSensitive` is true, its
//! lower-cased path lies under a prefix (equal to it or below it) and no
//! converted entry has the same method (HEAD counts as GET) and a matching
//! path (`:x` matches one segment). This module is that rule, pure, so the
//! app and the contract test ask the same question.

use serde::{Deserialize, Serialize};

/// One route the engine serves to such a viewer (`converted`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoorRoute {
    /// `GET`, `POST`, …
    pub method: String,
    /// The engine's path pattern (`:id` is one segment).
    pub path: String,
}

/// The engine's answer.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SensitiveDoors {
    /// Some account is hidden from the caller.
    #[serde(default)]
    pub hides_sensitive: bool,
    /// Path prefixes that refuse unless converted.
    #[serde(default)]
    pub refuse_prefixes: Vec<String>,
    /// Routes under those prefixes that are served.
    #[serde(default)]
    pub converted: Vec<DoorRoute>,
}

fn segments(p: &str) -> impl Iterator<Item = &str> {
    p.split('/').filter(|s| !s.is_empty())
}

/// A desktop route template segment that stands for a value (`{id}`, `:id`).
fn is_placeholder(s: &str) -> bool {
    s.starts_with(':') || (s.starts_with('{') && s.ends_with('}'))
}

/// Does the engine pattern match the route? A `:x` pattern segment matches
/// any one segment; a placeholder in the route matches only a pattern
/// placeholder (a value we can't know matches no literal).
fn matches(pattern: &str, route: &str) -> bool {
    let ps: Vec<&str> = segments(pattern).collect();
    let rs: Vec<&str> = segments(route).collect();
    ps.len() == rs.len()
        && ps.iter().zip(&rs).all(|(p, r)| {
            if p.starts_with(':') {
                true
            } else {
                !is_placeholder(r) && p.eq_ignore_ascii_case(r)
            }
        })
}

/// The pinned engine's answer to a viewer with a hidden account (copied
/// from its `services/visibility.ts`; the live contract test checks the
/// engine still answers exactly this). The demo answers the same.
pub const ENGINE_ANSWER_JSON: &str = include_str!("../fixtures/sensitive_doors.json");

impl SensitiveDoors {
    /// Nothing refused (an older engine, an Admin, nothing marked).
    #[must_use]
    pub fn none() -> SensitiveDoors {
        SensitiveDoors::default()
    }

    /// Would the engine refuse `method route` for this viewer? `route` may
    /// be a template (`{id}` / `:id`) or a concrete path; a query string is
    /// ignored.
    #[must_use]
    pub fn refuses(&self, method: &str, route: &str) -> bool {
        if !self.hides_sensitive {
            return false;
        }
        let path = route.split('?').next().unwrap_or(route);
        // The engine compares on the lower-cased path; placeholders keep
        // their case only for the `{..}` test, which is case-free anyway.
        let normal = format!("/{}", segments(path).collect::<Vec<_>>().join("/"));
        let lower = normal.to_ascii_lowercase();
        let under = self.refuse_prefixes.iter().any(|p| {
            let p = p.to_ascii_lowercase();
            lower == p || lower.starts_with(&format!("{p}/"))
        });
        if !under {
            return false;
        }
        let m = method.to_ascii_uppercase();
        let m = if m == "HEAD" { "GET".to_owned() } else { m };
        !self
            .converted
            .iter()
            .any(|c| c.method.eq_ignore_ascii_case(&m) && matches(&c.path, &normal))
    }

    /// Does any of these `(method, route)` pairs get refused?
    #[must_use]
    pub fn refuses_any(&self, routes: &[(&str, &str)]) -> bool {
        routes.iter().any(|(m, r)| self.refuses(m, r))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doors() -> SensitiveDoors {
        SensitiveDoors {
            hides_sensitive: true,
            refuse_prefixes: vec!["/api/v1/reports".into(), "/api/v1/budgets".into()],
            converted: vec![
                DoorRoute {
                    method: "GET".into(),
                    path: "/api/v1/reports/profit-loss".into(),
                },
                DoorRoute {
                    method: "GET".into(),
                    path: "/api/v1/budgets/:id/lines".into(),
                },
            ],
        }
    }

    #[test]
    fn refuses_unconverted_routes_under_a_prefix() {
        let d = doors();
        assert!(d.refuses("GET", "/api/v1/reports/sales-tax-liability"));
        assert!(d.refuses("GET", "/api/v1/reports"), "the prefix itself");
        assert!(d.refuses("GET", "/API/v1/Reports/Sales-Tax-Liability?x=1"));
        assert!(
            d.refuses("POST", "/api/v1/reports/profit-loss"),
            "method counts"
        );
        assert!(d.refuses("GET", "/api/v1/budgets/{id}/vs-actual"));
    }

    #[test]
    fn serves_converted_and_other_routes() {
        let d = doors();
        assert!(!d.refuses("GET", "/api/v1/reports/profit-loss"));
        assert!(
            !d.refuses("HEAD", "/api/v1/reports/profit-loss"),
            "HEAD is GET"
        );
        assert!(!d.refuses("get", "/api/v1/reports/profit-loss/"));
        assert!(!d.refuses("GET", "/api/v1/budgets/{id}/lines"));
        assert!(!d.refuses("GET", "/api/v1/budgets/:id/lines"));
        assert!(!d.refuses(
            "GET",
            "/api/v1/budgets/0b8f5c1e-0000-4000-8000-000000000001/lines"
        ));
        assert!(
            !d.refuses("GET", "/api/v1/reportsx"),
            "not under the prefix"
        );
        assert!(!d.refuses("GET", "/api/v1/accounts"));
    }

    #[test]
    fn nothing_refused_when_nothing_is_hidden() {
        let mut d = doors();
        d.hides_sensitive = false;
        assert!(!d.refuses("GET", "/api/v1/reports/sales-tax-liability"));
        assert!(!SensitiveDoors::none().refuses("GET", "/api/v1/reports/x"));
    }

    #[test]
    fn a_template_value_matches_no_literal_pattern() {
        let d = SensitiveDoors {
            hides_sensitive: true,
            refuse_prefixes: vec!["/api/v1/export".into()],
            converted: vec![DoorRoute {
                method: "GET".into(),
                path: "/api/v1/export/full/accounts.csv".into(),
            }],
        };
        assert!(d.refuses("GET", "/api/v1/export/full/{fileName}"));
        assert!(!d.refuses("GET", "/api/v1/export/full/accounts.csv"));
    }

    #[test]
    fn the_pinned_answer_reads() {
        let d: SensitiveDoors = serde_json::from_str(ENGINE_ANSWER_JSON).expect("fixture");
        assert!(d.hides_sensitive);
        assert!(d.converted.len() > 10, "the converted list is read");
        assert!(d.refuses("GET", "/api/v1/reports/sales-tax-liability"));
        assert!(!d.refuses("GET", "/api/v1/reports/profit-loss"));
        assert!(!d.refuses("GET", "/api/v1/sales-tax/owed"));
    }

    #[test]
    fn reads_the_engine_answer() {
        let d: SensitiveDoors = serde_json::from_str(
            r#"{"hidesSensitive":true,"refusePrefixes":["/api/v1/ai"],
                "converted":[{"method":"GET","path":"/api/v1/ai/status"}]}"#,
        )
        .expect("json");
        assert!(d.refuses("GET", "/api/v1/ai/usage"));
        assert!(!d.refuses("GET", "/api/v1/ai/status"));
        assert_eq!(
            serde_json::from_str::<SensitiveDoors>("{}").expect("json"),
            SensitiveDoors::none()
        );
    }
}
