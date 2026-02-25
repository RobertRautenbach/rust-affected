use guppy::{graph::PackageGraph, MetadataCommand};
use std::env;
use std::io::Write;
use std::path::Path;

fn main() {
    let changed_files: Vec<String> = env::var("CHANGED_FILES")
        .unwrap_or_default()
        .split_whitespace()
        .map(String::from)
        .collect();

    if changed_files.is_empty() {
        emit_output(false, vec![], vec![], vec![]);
        return;
    }

    // FORCE_TRIGGERS env var; entries are newline- or space-separated.
    // Each entry is matched as either an exact filename or a path prefix (if it ends with '/').
    let force_triggers: Vec<String> = env::var("FORCE_TRIGGERS")
        .map(|v| v.split_whitespace().map(String::from).collect())
        .unwrap_or_default();

    let force_all = changed_files.iter().any(|f| {
        force_triggers
            .iter()
            .any(|trigger| f == trigger || f.starts_with(trigger.as_str()))
    });

    let mut cmd = MetadataCommand::new();
    let graph = PackageGraph::from_command(&mut cmd)
        .expect("Failed to load package graph. Is this a Cargo workspace?");

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
        .filter(|pkg| workspace.contains_name(pkg.name()))
        .map(|pkg| pkg.name().to_string())
        .collect();
    changed_crates.sort();

    let mut affected_library_members: Vec<String> = affected_set
        .packages(guppy::graph::DependencyDirection::Forward)
        .filter(|pkg| workspace.contains_name(pkg.name()))
        .map(|pkg| pkg.name().to_string())
        .collect();
    affected_library_members.sort();

    let mut affected_binary_members: Vec<String> = affected_set
        .packages(guppy::graph::DependencyDirection::Forward)
        .filter(|pkg| {
            workspace.contains_name(pkg.name())
                && pkg.manifest_path().as_str().contains("/services/")
                && pkg
                    .build_targets()
                    .any(|t| t.kind() == guppy::graph::BuildTargetKind::Binary)
        })
        .map(|pkg| pkg.name().to_string())
        .collect();
    affected_binary_members.sort();

    emit_output(
        force_all,
        changed_crates,
        affected_library_members,
        affected_binary_members,
    );
}

fn emit_output(force: bool, changed: Vec<String>, affected: Vec<String>, binaries: Vec<String>) {
    let github_actions = env::args().any(|a| a == "--github-actions");

    let changed_json = serde_json::to_string(&changed).unwrap();
    let affected_json = serde_json::to_string(&affected).unwrap();
    let binaries_json = serde_json::to_string(&binaries).unwrap();
    let force_str = force.to_string();

    if github_actions {
        let path = env::var("GITHUB_OUTPUT").expect("GITHUB_OUTPUT not set");
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(path)
            .expect("Failed to open GITHUB_OUTPUT");
        writeln!(file, "changed_crates={changed_json}").unwrap();
        writeln!(file, "affected_library_members={affected_json}").unwrap();
        writeln!(file, "affected_binary_members={binaries_json}").unwrap();
        writeln!(file, "force_all={force_str}").unwrap();
    } else {
        println!(
            "{}",
            serde_json::json!({
                "changed_crates": changed,
                "affected_library_members": affected,
                "affected_binary_members": binaries,
                "force_all": force,
            })
        );
    }
}
