//! A small, test-friendly view of `cargo metadata`.

use anyhow::{Context, Result};
use cargo_metadata::{CargoOpt, MetadataCommand, PackageId};
use std::collections::{BTreeMap, BTreeSet};

/// A direct dependency of a workspace member.
#[derive(Debug, Clone)]
pub struct Dep {
    pub name: String,
    /// SPDX expression from the dependency's manifest, if it has one.
    pub license: Option<String>,
    /// True when the dependency is another member of this workspace.
    pub workspace: bool,
}

/// A workspace member and its direct dependencies (normal, build, and dev).
#[derive(Debug, Clone)]
pub struct Member {
    pub name: String,
    pub deps: Vec<Dep>,
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
            deps.push(Dep {
                name: dep_pkg.name.to_string(),
                license: dep_pkg.license.clone(),
                workspace: workspace_ids.contains(&d.pkg),
            });
        }
        deps.sort_by(|a, b| a.name.cmp(&b.name));
        members.push(Member {
            name: pkg.name.to_string(),
            deps,
        });
    }
    members.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(members)
}
