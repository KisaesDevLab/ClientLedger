//! Crate layering rules from CLAUDE.md, encoded as a table.
//!
//! Not covered here (cannot be seen in the dependency graph): "only `cl-pg`,
//! `cl-host`, and `cl-company` may spawn processes or touch the database
//! directory". That rule is enforced in code review.

use crate::meta::Member;

/// Crates that sit below `cl-ui`: they may not depend on `cl-ui` or on apps.
const BELOW_UI: [&str; 12] = [
    "cl-core",
    "cl-api",
    "cl-pg",
    "cl-host",
    "cl-company",
    "cl-settings",
    "cl-import",
    "cl-model",
    "cl-grid",
    "cl-automation",
    "cl-print",
    "cl-lan",
];

/// Crates allowed to depend directly on the egui family.
const EGUI_ALLOWED: [&str; 4] = ["cl-grid", "cl-ui", "clientledger", "clientledger-cli"];

fn is_egui_family(name: &str) -> bool {
    name == "eframe"
        || name == "egui"
        || name.starts_with("egui_")
        || name.starts_with("egui-")
        || name == "epaint"
        || name == "emath"
}

/// Which workspace crates `name` may depend on. `None` means "unknown member".
fn allowed_internal(name: &str) -> Option<Vec<&'static str>> {
    let below = |names: &[&'static str]| Some(names.to_vec());
    match name {
        "cl-model" | "cl-api" | "cl-pg" | "cl-settings" | "cl-import" => below(&["cl-core"]),
        "cl-grid" => below(&["cl-core", "cl-model"]),
        // Everything except apps (and itself).
        "cl-ui" => Some(BELOW_UI.to_vec()),
        // Remaining library crates may use anything below the UI.
        "cl-host" | "cl-company" | "cl-print" | "cl-automation" | "cl-lan" => {
            Some(BELOW_UI.iter().copied().filter(|c| *c != name).collect())
        }
        // Apps may use any library crate.
        "clientledger" | "clientledger-cli" => {
            let mut v = BELOW_UI.to_vec();
            v.push("cl-ui");
            Some(v)
        }
        // cl-core and build tooling depend on nothing internal.
        "cl-core" | "xtask" => below(&[]),
        _ => None,
    }
}

/// Extra workspace crates a member may use through `[dev-dependencies]` only
/// (CLAUDE.md, Q-P05b): `cl-api` reaches `cl-host` and `cl-pg` for its
/// contract harness, while its normal dependencies stay `cl-core` only.
fn allowed_dev_internal(name: &str) -> &'static [&'static str] {
    match name {
        "cl-api" => &["cl-host", "cl-pg"],
        _ => &[],
    }
}

/// Cargo features that exist only for tests. They must never be switched on
/// by a normal dependency edge, by an app, or by a default feature.
const TEST_FEATURES: [&str; 2] = ["test-support", "testing"];

/// Apps are release artefacts: no edge of theirs may enable a test feature.
const APPS: [&str; 2] = ["clientledger", "clientledger-cli"];

fn is_test_feature(f: &str) -> bool {
    // Accept `test-support`, `dep/test-support`, `dep?/test-support` and the
    // `dep:` forwarding form.
    let f = f.strip_prefix("dep:").unwrap_or(f);
    let leaf = f.rsplit('/').next().unwrap_or(f);
    TEST_FEATURES.contains(&leaf)
}

/// Test-only features in `cargo tree -e features,no-dev` output for the
/// apps (lines like `cl-api feature "test-support"`): what a normal
/// (release) build of an app would switch on.
#[must_use]
pub fn test_features_in_tree(tree: &str) -> Vec<String> {
    let mut out: Vec<String> = tree
        .lines()
        .filter_map(|l| {
            let (krate, rest) = l.trim().split_once(" feature \"")?;
            let feature = rest.strip_suffix('"')?;
            is_test_feature(feature).then(|| {
                format!("a normal build of the apps enables test-only feature `{krate}/{feature}`")
            })
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Features reachable from `default` in a member's own `[features]` table.
fn default_closure(m: &Member) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut stack = vec!["default".to_string()];
    while let Some(f) = stack.pop() {
        if seen.contains(&f) {
            continue;
        }
        if let Some(children) = m.features.get(&f) {
            stack.extend(children.iter().cloned());
        }
        seen.push(f);
    }
    seen
}

/// Returns one message per offending edge.
#[must_use]
pub fn check(members: &[Member]) -> Vec<String> {
    let mut out = Vec::new();
    for m in members {
        let Some(allowed) = allowed_internal(&m.name) else {
            out.push(format!(
                "{} is a workspace member with no entry in the layering table (xtask/src/layering.rs)",
                m.name
            ));
            continue;
        };
        for d in &m.deps {
            // A crate may list itself as a dev-dependency to switch on its own
            // test-only features; that is not a layering edge.
            if d.workspace && d.name != m.name {
                let normal_ok = allowed.contains(&d.name.as_str());
                let dev_ok = d.dev_only && allowed_dev_internal(&m.name).contains(&d.name.as_str());
                if !normal_ok && !dev_ok {
                    out.push(format!(
                        "forbidden crate edge: {} -> {} (see layering rules in CLAUDE.md)",
                        m.name, d.name
                    ));
                }
            }
            if d.workspace {
                let is_app = APPS.contains(&m.name.as_str());
                let bad = d
                    .normal_features
                    .iter()
                    .chain(d.dev_features.iter().filter(|_| is_app))
                    .find(|f| is_test_feature(f));
                if let Some(f) = bad {
                    out.push(format!(
                        "test-only feature `{f}` enabled by {} -> {} (allowed only on dev-dependencies of library crates)",
                        m.name, d.name
                    ));
                }
            }
            if is_egui_family(&d.name) && !EGUI_ALLOWED.contains(&m.name.as_str()) {
                out.push(format!(
                    "forbidden crate edge: {} -> {} (only cl-grid, cl-ui, and apps may use egui)",
                    m.name, d.name
                ));
            }
        }
        // Only a test feature may forward a test feature: `dev =
        // ["cl-api/test-support"]` would let any edge switch it on by name.
        for (name, values) in &m.features {
            if is_test_feature(name) {
                continue;
            }
            if let Some(f) = values.iter().find(|f| is_test_feature(f)) {
                out.push(format!(
                    "feature `{name}` of {} forwards test-only feature `{f}` (only a test feature may)",
                    m.name
                ));
            }
        }
        if let Some(f) = default_closure(m).iter().find(|f| is_test_feature(f)) {
            out.push(format!(
                "{} enables test-only feature `{f}` by default (it would reach release builds)",
                m.name
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::Dep;

    fn m(name: &str, deps: &[(&str, bool)]) -> Member {
        Member {
            name: name.into(),
            deps: deps
                .iter()
                .map(|(n, ws)| Dep {
                    name: (*n).into(),
                    license: Some("MIT".into()),
                    workspace: *ws,
                    ..Dep::default()
                })
                .collect(),
            ..Member::default()
        }
    }

    fn dev(name: &str, dep: &str, features: &[&str]) -> Member {
        let mut mm = m(name, &[(dep, true)]);
        mm.deps[0].dev_only = true;
        mm.deps[0].dev_features = features.iter().map(|f| (*f).to_string()).collect();
        mm
    }

    #[test]
    fn valid_graph_passes() {
        let ms = vec![
            m("cl-core", &[]),
            m("cl-model", &[("cl-core", true)]),
            m(
                "cl-grid",
                &[("cl-core", true), ("cl-model", true), ("egui", false)],
            ),
            m(
                "cl-ui",
                &[("cl-grid", true), ("cl-api", true), ("eframe", false)],
            ),
            m("clientledger", &[("cl-ui", true), ("eframe", false)]),
        ];
        assert_eq!(check(&ms), Vec::<String>::new());
    }

    #[test]
    fn a_crate_may_dev_depend_on_itself_to_enable_test_features() {
        assert_eq!(
            check(&[m("cl-host", &[("cl-host", true)])]),
            Vec::<String>::new()
        );
        // ... but that does not open any other edge.
        assert_eq!(
            check(&[m("cl-core", &[("cl-core", true), ("cl-model", true)])]).len(),
            1
        );
    }

    #[test]
    fn core_depends_on_nothing_internal() {
        let v = check(&[m("cl-core", &[("cl-model", true)])]);
        assert_eq!(v.len(), 1);
        assert!(v[0].contains("cl-core -> cl-model"));
    }

    #[test]
    fn leaf_crates_may_only_use_core() {
        for leaf in ["cl-model", "cl-api", "cl-pg", "cl-settings", "cl-import"] {
            assert!(check(&[m(leaf, &[("cl-core", true)])]).is_empty(), "{leaf}");
            assert_eq!(check(&[m(leaf, &[("cl-host", true)])]).len(), 1, "{leaf}");
        }
    }

    #[test]
    fn grid_cannot_use_api() {
        assert_eq!(check(&[m("cl-grid", &[("cl-api", true)])]).len(), 1);
    }

    #[test]
    fn nothing_below_ui_depends_on_ui_or_apps() {
        for c in [
            "cl-host",
            "cl-company",
            "cl-print",
            "cl-automation",
            "cl-lan",
            "cl-grid",
        ] {
            assert_eq!(check(&[m(c, &[("cl-ui", true)])]).len(), 1, "{c}");
            assert_eq!(check(&[m(c, &[("clientledger", true)])]).len(), 1, "{c}");
        }
    }

    #[test]
    fn egui_only_in_grid_ui_and_apps() {
        assert_eq!(check(&[m("cl-model", &[("egui", false)])]).len(), 1);
        assert_eq!(
            check(&[m("cl-automation", &[("egui_extras", false)])]).len(),
            1
        );
        assert_eq!(
            check(&[m("cl-ui", &[("egui_extras", false)])]),
            Vec::<String>::new()
        );
    }

    #[test]
    fn unknown_member_is_reported() {
        assert_eq!(check(&[m("cl-mystery", &[])]).len(), 1);
    }

    #[test]
    fn cl_api_may_dev_depend_on_host_and_pg() {
        for dep in ["cl-host", "cl-pg"] {
            assert_eq!(
                check(&[dev("cl-api", dep, &[])]),
                Vec::<String>::new(),
                "{dep}"
            );
        }
    }

    #[test]
    fn cl_api_normal_edge_to_host_or_pg_is_still_refused() {
        for dep in ["cl-host", "cl-pg"] {
            assert_eq!(check(&[m("cl-api", &[(dep, true)])]).len(), 1, "{dep}");
        }
    }

    #[test]
    fn cl_api_dev_edge_is_limited_to_host_and_pg() {
        for dep in ["cl-model", "cl-ui", "cl-company", "cl-grid", "clientledger"] {
            assert_eq!(check(&[dev("cl-api", dep, &[])]).len(), 1, "{dep}");
        }
    }

    #[test]
    fn no_other_crate_gets_a_dev_edge_exemption() {
        for from in [
            "cl-core",
            "cl-model",
            "cl-pg",
            "cl-settings",
            "cl-import",
            "cl-grid",
        ] {
            assert_eq!(check(&[dev(from, "cl-host", &[])]).len(), 1, "{from}");
        }
        assert_eq!(check(&[dev("cl-core", "cl-api", &[])]).len(), 1);
        assert_eq!(check(&[dev("cl-pg", "cl-api", &[])]).len(), 1);
    }

    #[test]
    fn test_support_on_a_dev_edge_of_a_library_is_fine() {
        assert_eq!(
            check(&[dev("cl-api", "cl-host", &["test-support"])]),
            Vec::<String>::new()
        );
        assert_eq!(
            check(&[dev("cl-ui", "cl-api", &["test-support"])]),
            Vec::<String>::new()
        );
    }

    #[test]
    fn test_support_on_a_normal_edge_is_refused() {
        for f in ["test-support", "testing"] {
            let mut mm = m("cl-ui", &[("cl-api", true)]);
            mm.deps[0].normal_features = vec![f.into()];
            let v = check(&[mm]);
            assert_eq!(v.len(), 1, "{f}");
            assert!(v[0].contains(f));
        }
    }

    #[test]
    fn apps_may_not_enable_test_support_even_as_dev() {
        for app in ["clientledger", "clientledger-cli"] {
            assert_eq!(
                check(&[dev(app, "cl-api", &["test-support"])]).len(),
                1,
                "{app}"
            );
        }
        // Other features on an app's dev edge are fine.
        assert_eq!(
            check(&[dev("clientledger", "cl-api", &["serde"])]),
            Vec::<String>::new()
        );
    }

    #[test]
    fn default_features_must_not_reach_test_support() {
        let mut mm = m("cl-api", &[]);
        mm.features.insert("default".into(), vec!["rt".into()]);
        mm.features
            .insert("rt".into(), vec!["cl-host/test-support".into()]);
        assert_eq!(
            check(&[mm]).len(),
            2,
            "forwarded by `rt` and reached by default"
        );

        let mut direct = m("cl-host", &[]);
        direct
            .features
            .insert("default".into(), vec!["test-support".into()]);
        direct.features.insert("test-support".into(), vec![]);
        assert_eq!(check(&[direct]).len(), 2);

        let mut ok = m("cl-host", &[]);
        ok.features.insert("default".into(), vec![]);
        ok.features.insert("test-support".into(), vec![]);
        assert_eq!(check(&[ok]), Vec::<String>::new());
    }

    #[test]
    fn only_a_test_feature_may_forward_a_test_feature() {
        for bad in [
            "cl-api/test-support",
            "cl-api?/test-support",
            "dep:test-support",
            "cl-host/testing",
        ] {
            let mut mm = m("cl-ui", &[]);
            mm.features.insert("dev".into(), vec![bad.into()]);
            let v = check(&[mm]);
            assert_eq!(v.len(), 1, "{bad}: {v:?}");
            assert!(v[0].contains("feature `dev`"), "{v:?}");
        }
        let mut ok = m("cl-host", &[]);
        ok.features
            .insert("test-support".into(), vec!["cl-api?/test-support".into()]);
        ok.features.insert("dev".into(), vec!["serde/std".into()]);
        assert_eq!(check(&[ok]), Vec::<String>::new());
    }

    #[test]
    fn release_tree_with_a_test_feature_is_reported() {
        let clean = "clientledger-cli v0.0.0 (/x)\ncl-host feature \"default\"\n";
        assert_eq!(test_features_in_tree(clean), Vec::<String>::new());
        let leaked = "cl-api feature \"test-support\"\n  cl-api feature \"test-support\"\ncl-host feature \"testing\"\n";
        assert_eq!(
            test_features_in_tree(leaked),
            vec![
                "a normal build of the apps enables test-only feature `cl-api/test-support`"
                    .to_string(),
                "a normal build of the apps enables test-only feature `cl-host/testing`"
                    .to_string(),
            ]
        );
    }
}
