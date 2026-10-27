//! A small, test-friendly view of `cargo metadata`.

use anyhow::{Context, Result};
use cargo_metadata::{CargoOpt, DependencyKind, MetadataCommand, PackageId};
use std::collections::{BTreeMap, BTreeSet};

/// A direct dependency of a workspace member.
#[derive(Debug, Clone, Default)]
pub struct Dep {
    pub name: String,
    /// SPDX expression from the dependency's manifest, if it has one.
    pub license: Option<String>,
    /// True when the dependency is another member of this workspace.
    pub workspace: bool,
    /// True when every use of this dependency is a `[dev-dependencies]` edge.
    pub dev_only: bool,
    /// Features the manifest requests on the normal/build edge.
    pub normal_features: Vec<String>,
    /// Features the manifest requests on the dev edge.
    pub dev_features: Vec<String>,
}

/// A workspace member and its direct dependencies (normal, build, and dev).
#[derive(Debug, Clone, Default)]
pub struct Member {
    pub name: String,
    pub deps: Vec<Dep>,
    /// The member's own `[features]` table.
    pub features: BTreeMap<String, Vec<String>>,
}

/// Read the workspace from `cargo metadata` with all features enabled.
pub fn load() -> Result<Vec<Member>> {
    let md = MetadataCommand::new()
        .features(CargoOpt::AllFeatures)
        .exec()
        .context("running `cargo metadata`")?;
    let resolve = md
        .resolve
        .as_ref()
        .context("cargo metadata returned no resolve graph")?;
    let by_id: BTreeMap<&PackageId, &cargo_metadata::Package> =
        md.packages.iter().map(|p| (&p.id, p)).collect();
    let workspace_ids: BTreeSet<&PackageId> = md.workspace_members.iter().collect();

    let mut members = Vec::new();
    for id in &md.workspace_members {
        let pkg = by_id
            .get(id)
            .context("workspace member missing from packages")?;
        let node = resolve
            .nodes
            .iter()
            .find(|n| &n.id == id)
            .context("workspace member missing from resolve graph")?;
        let mut deps = Vec::new();
        for d in &node.deps {
            let dep_pkg = by_id
                .get(&d.pkg)
                .context("dependency missing from packages")?;
            let dev_only = !d.dep_kinds.is_empty()
                && d.dep_kinds
                    .iter()
                    .all(|k| k.kind == DependencyKind::Development);
            let mut normal_features = Vec::new();
            let mut dev_features = Vec::new();
            for md in pkg.dependencies.iter().filter(|x| *x.name == *dep_pkg.name) {
                let target = if md.kind == DependencyKind::Development {
                    &mut dev_features
                } else {
                    &mut normal_features
                };
                target.extend(md.features.iter().cloned());
            }
            deps.push(Dep {
                name: dep_pkg.name.to_string(),
                license: dep_pkg.license.clone(),
                workspace: workspace_ids.contains(&d.pkg),
                dev_only,
                normal_features,
                dev_features,
            });
        }
        deps.sort_by(|a, b| a.name.cmp(&b.name));
        members.push(Member {
            name: pkg.name.to_string(),
            deps,
            features: pkg.features.clone(),
        });
    }
    members.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(members)
}

/// The build directory cargo really uses: `CARGO_TARGET_DIR`, else the
/// `build.target-dir` from any cargo config (a machine may share one across
/// worktrees), else `<root>/target`.
pub fn target_dir(root: &std::path::Path) -> std::path::PathBuf {
    if let Some(dir) = std::env::var_os("CARGO_TARGET_DIR").filter(|v| !v.is_empty()) {
        return dir.into();
    }
    MetadataCommand::new()
        .manifest_path(root.join("Cargo.toml"))
        .no_deps()
        .exec()
        .map_or_else(
            |_| root.join("target"),
            |m| m.target_directory.into_std_path_buf(),
        )
}
