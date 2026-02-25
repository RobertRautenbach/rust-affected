use guppy::{graph::PackageGraph, MetadataCommand};
use rust_affected::compute_affected;
use std::env;
use std::io::Write;

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

    let force_triggers: Vec<String> = env::var("FORCE_TRIGGERS")
        .map(|v| v.split_whitespace().map(String::from).collect())
        .unwrap_or_default();

    let excluded: std::collections::HashSet<String> = env::var("EXCLUDED_MEMBERS")
        .map(|v| v.split_whitespace().map(String::from).collect())
        .unwrap_or_default();

    let mut cmd = MetadataCommand::new();
    let graph = PackageGraph::from_command(&mut cmd)
        .expect("Failed to load package graph. Is this a Cargo workspace?");

    let result = compute_affected(&graph, &changed_files, &force_triggers, &excluded);

    emit_output(
        result.force_all,
        result.changed_crates,
        result.affected_library_members,
        result.affected_binary_members,
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
