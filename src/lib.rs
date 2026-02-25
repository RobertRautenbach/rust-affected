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

/// Check whether a package should be excluded from results.
///
/// Entries that contain a `/` are treated as **path prefixes** and matched against
/// the package's directory path relative to the workspace root.
///   - `tools/` matches every crate whose relative directory starts with `tools/`
///     (e.g. `tools/resource-clone`).
///   - `tools/resource-clone` matches that exact relative directory.
///
/// Entries without a `/` are compared against the **crate name** directly
/// (e.g. `resource-clone` excludes a crate named `resource-clone` regardless of
/// where it lives in the workspace).
fn is_excluded(pkg_name: &str, pkg_relative_dir: &Path, excluded: &HashSet<String>) -> bool {
    for entry in excluded {
        if entry.contains('/') {
            // Path-based exclusion
            let prefix = entry.strip_suffix('/').unwrap_or(entry.as_str());
            let dir_str = pkg_relative_dir.to_str().unwrap_or("");
            if dir_str == prefix || dir_str.starts_with(&format!("{prefix}/")) {
                return true;
            }
        } else if pkg_name == entry {
            return true;
        }
    }
    false
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
///
/// Exclusion entries that contain `/` are matched as path prefixes against each
/// package's directory relative to the workspace root (e.g. `tools/` excludes
/// every crate under `tools/`). Entries without `/` are matched against the crate
/// name directly.
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

    // Helper: compute the relative directory for a package.
    let relative_dir = |pkg: &guppy::graph::PackageMetadata| -> std::path::PathBuf {
        let dir = pkg
            .manifest_path()
            .parent()
            .expect("manifest has no parent")
            .as_std_path();
        dir.strip_prefix(workspace_root)
            .unwrap_or(dir)
            .to_path_buf()
    };

    let mut direct_ids = Vec::new();
    for pkg in graph.workspace().iter() {
        let pkg_dir = relative_dir(&pkg);

        if changed_files
            .iter()
            .any(|f| Path::new(f).starts_with(&pkg_dir))
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
        .filter(|pkg| {
            workspace.contains_name(pkg.name())
                && !is_excluded(pkg.name(), &relative_dir(pkg), excluded)
        })
        .map(|pkg| pkg.name().to_string())
        .collect();
    changed_crates.sort();

    let mut affected_library_members: Vec<String> = affected_set
        .packages(guppy::graph::DependencyDirection::Forward)
        .filter(|pkg| {
            workspace.contains_name(pkg.name())
                && !is_excluded(pkg.name(), &relative_dir(pkg), excluded)
        })
        .map(|pkg| pkg.name().to_string())
        .collect();
    affected_library_members.sort();

    let mut affected_binary_members: Vec<String> = affected_set
        .packages(guppy::graph::DependencyDirection::Forward)
        .filter(|pkg| {
            workspace.contains_name(pkg.name())
                && !is_excluded(pkg.name(), &relative_dir(pkg), excluded)
                && pkg
                    .build_targets()
                    .any(|t| matches!(t.id(), guppy::graph::BuildTargetId::Binary(_)))
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
