use globset::{Glob, GlobSetBuilder};
use guppy::graph::PackageGraph;
use std::collections::HashSet;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AffectedResult {
    pub force_all: bool,
    pub changed_crates: Vec<String>,
    pub affected_library_members: Vec<String>,
    pub affected_binary_members: Vec<String>,
}

pub fn check_force_triggers(changed_files: &[String], force_triggers: &[String]) -> bool {
    if force_triggers.is_empty() {
        return false;
    }

    let mut builder = GlobSetBuilder::new();
    for trigger in force_triggers {
        let pattern = if trigger.ends_with('/') {
            format!("{}**", trigger)
        } else {
            trigger.clone()
        };
        builder.add(
            Glob::new(&pattern)
                .unwrap_or_else(|e| panic!("Invalid force_trigger glob pattern {pattern:?}: {e}")),
        );
    }
    let globset = builder
        .build()
        .expect("Failed to build force_triggers glob set");
    changed_files.iter().any(|f| globset.is_match(f))
}

/// Compute which workspace crates are affected by a set of changed files.
///
/// `excluded` filters crate names from all three output lists (changed_crates,
/// affected_library_members, affected_binary_members) but does **not** prune the
/// dependency graph traversal. An excluded crate is still traversed when resolving
/// transitive dependents — it simply won't appear in the results.
pub fn compute_affected(
    graph: &PackageGraph,
    changed_files: &[String],
    force_triggers: &[String],
    excluded: &HashSet<String>,
) -> AffectedResult {
    if changed_files.is_empty() {
        return AffectedResult {
            force_all: false,
            changed_crates: vec![],
            affected_library_members: vec![],
            affected_binary_members: vec![],
        };
    }

    let force_all = check_force_triggers(changed_files, force_triggers);

    let workspace_root = graph.workspace().root().as_std_path();

    let mut direct_ids = Vec::new();
    for pkg in graph.workspace().iter() {
        let pkg_dir = pkg
            .manifest_path()
            .parent()
            .expect("manifest has no parent")
            .as_std_path();

        let pkg_dir = pkg_dir.strip_prefix(workspace_root).unwrap_or(pkg_dir);

        if changed_files
            .iter()
            .any(|f| Path::new(f).starts_with(pkg_dir))
        {
            direct_ids.push(pkg.id().clone());
        }
    }

    let affected_set = if force_all {
        graph.query_workspace().resolve()
    } else {
        graph
            .query_reverse(direct_ids.iter())
            .expect("reverse query failed")
            .resolve()
    };

    let workspace = graph.workspace();

    let mut changed_crates: Vec<String> = direct_ids
        .iter()
        .filter_map(|id| graph.metadata(id).ok())
        .filter(|pkg| workspace.contains_name(pkg.name()) && !excluded.contains(pkg.name()))
        .map(|pkg| pkg.name().to_string())
        .collect();
    changed_crates.sort();

    let mut affected_library_members: Vec<String> = affected_set
        .packages(guppy::graph::DependencyDirection::Forward)
        .filter(|pkg| workspace.contains_name(pkg.name()) && !excluded.contains(pkg.name()))
        .map(|pkg| pkg.name().to_string())
        .collect();
    affected_library_members.sort();

    let mut affected_binary_members: Vec<String> = affected_set
        .packages(guppy::graph::DependencyDirection::Forward)
        .filter(|pkg| {
            workspace.contains_name(pkg.name())
                && !excluded.contains(pkg.name())
                && pkg
                    .build_targets()
                    .any(|t| t.kind() == guppy::graph::BuildTargetKind::Binary)
        })
        .map(|pkg| pkg.name().to_string())
        .collect();
    affected_binary_members.sort();

    AffectedResult {
        force_all,
        changed_crates,
        affected_library_members,
        affected_binary_members,
    }
}
