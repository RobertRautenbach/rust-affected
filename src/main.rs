use globset::{Glob, GlobSetBuilder};
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

    // FORCE_TRIGGERS env var; entries are newline- or space-separated glob patterns.
    // Trailing-slash entries (e.g. "infra/") are normalised to "infra/**" so they
    // match all files inside that directory. Patterns support *, **, and ? via globset.
    let force_triggers: Vec<String> = env::var("FORCE_TRIGGERS")
        .map(|v| v.split_whitespace().map(String::from).collect())
        .unwrap_or_default();

    let force_all =
        if force_triggers.is_empty() {
            false
        } else {
            let mut builder = GlobSetBuilder::new();
            for trigger in &force_triggers {
                let pattern = if trigger.ends_with('/') {
                    format!("{}**", trigger)
                } else {
                    trigger.clone()
                };
                builder.add(Glob::new(&pattern).unwrap_or_else(|e| {
                    panic!("Invalid force_trigger glob pattern {pattern:?}: {e}")
                }));
            }
            let globset = builder
                .build()
                .expect("Failed to build force_triggers glob set");
            changed_files.iter().any(|f| globset.is_match(f))
        };

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
    let changed_json = serde_json::to_string(&changed).unwrap();
    let affected_json = serde_json::to_string(&affected).unwrap();
    let binaries_json = serde_json::to_string(&binaries).unwrap();
    let force_str = force.to_string();

    // When GITHUB_OUTPUT is set (i.e. running inside a GitHub Actions runner)
    // write key=value pairs to the output file expected by the runner.
    // Otherwise fall back to printing a JSON object to stdout for local use.
    if let Ok(path) = env::var("GITHUB_OUTPUT") {
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
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
