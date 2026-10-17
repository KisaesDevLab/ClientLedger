//! Direct-dependency license gate (Q-07): every direct dependency of a
//! workspace crate must be available under MIT or Apache-2.0.
//! Transitive dependencies are checked by `cargo deny` against `deny.toml`.

use crate::meta::Member;

const ALLOWED: [&str; 2] = ["MIT", "Apache-2.0"];

/// True when at least one alternative of the SPDX expression is made only of
/// allowed licenses. `A OR B` (and the legacy `A/B`) is satisfied by either
/// side; `A AND B` needs both. `WITH` exceptions are not accepted for direct
/// dependencies.
#[must_use]
pub fn expression_allowed(expr: &str) -> bool {
    let expr = expr.replace('/', " OR ").replace(['(', ')'], " ");
    expr.split(" OR ").any(|alt| {
        let terms: Vec<&str> = alt.split(" AND ").map(str::trim).collect();
        !terms.is_empty() && terms.iter().all(|t| ALLOWED.contains(t))
    })
}

/// Returns one message per offending direct dependency.
#[must_use]
pub fn check(members: &[Member]) -> Vec<String> {
    let mut out = Vec::new();
    for m in members {
        for d in m.deps.iter().filter(|d| !d.workspace) {
            match d.license.as_deref() {
                Some(l) if expression_allowed(l) => {}
                Some(l) => out.push(format!(
                    "{} depends directly on {} with license `{l}`; direct dependencies must be MIT or Apache-2.0",
                    m.name, d.name
                )),
                None => out.push(format!(
                    "{} depends directly on {} which declares no license expression",
                    m.name, d.name
                )),
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::Dep;

    fn member(deps: &[(&str, Option<&str>)]) -> Vec<Member> {
        vec![Member {
            name: "cl-fake".into(),
            deps: deps
                .iter()
                .map(|(n, l)| Dep {
                    name: (*n).into(),
                    license: l.map(Into::into),
                    workspace: false,
                    ..Dep::default()
                })
                .collect(),
            ..Member::default()
        }]
    }

    #[test]
    fn accepts_mit_apache_and_dual() {
        for l in [
            "MIT",
            "Apache-2.0",
            "MIT OR Apache-2.0",
            "MIT/Apache-2.0",
            "(MIT OR Apache-2.0)",
        ] {
            assert!(expression_allowed(l), "{l}");
        }
    }

    #[test]
    fn accepts_dual_with_extra_alternatives() {
        assert!(expression_allowed("MIT OR Apache-2.0 OR GPL-3.0-only"));
        assert!(expression_allowed("Apache-2.0 OR GPL-2.0-only"));
    }

    #[test]
    fn rejects_gpl_bsd_and_exceptions() {
        for l in [
            "GPL-3.0-only",
            "BSD-3-Clause",
            "ISC",
            "MPL-2.0",
            "MIT AND BSD-3-Clause",
            "Apache-2.0 WITH LLVM-exception",
            "BSD-2-Clause OR GPL-2.0-only",
        ] {
            assert!(!expression_allowed(l), "{l}");
        }
    }

    #[test]
    fn reports_offending_direct_dependencies() {
        let ms = member(&[
            ("good", Some("MIT")),
            ("evil", Some("GPL-3.0-only")),
            ("bsd", Some("BSD-3-Clause")),
            ("blank", None),
        ]);
        let v = check(&ms);
        assert_eq!(v.len(), 3);
        assert!(v.iter().any(|m| m.contains("evil")));
        assert!(v.iter().any(|m| m.contains("bsd")));
        assert!(v.iter().any(|m| m.contains("blank")));
    }

    #[test]
    fn workspace_members_are_skipped() {
        let mut ms = member(&[("cl-core", None)]);
        ms[0].deps[0].workspace = true;
        assert_eq!(check(&ms), Vec::<String>::new());
    }
}
