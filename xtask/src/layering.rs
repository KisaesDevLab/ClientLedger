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
            if d.workspace && !allowed.contains(&d.name.as_str()) {
                out.push(format!(
                    "forbidden crate edge: {} -> {} (see layering rules in CLAUDE.md)",
                    m.name, d.name
                ));
            }
            if is_egui_family(&d.name) && !EGUI_ALLOWED.contains(&m.name.as_str()) {
                out.push(format!(
                    "forbidden crate edge: {} -> {} (only cl-grid, cl-ui, and apps may use egui)",
                    m.name, d.name
                ));
            }
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
                })
                .collect(),
        }
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
}
