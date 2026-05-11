use guppy::{MetadataCommand, graph::PackageGraph};
use rust_affected::{compute_affected, git_show, lockfile};
use std::collections::HashSet;
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

    let force_triggers: Vec<String> = env::var("FORCE_TRIGGERS")
        .map(|v| v.split_whitespace().map(String::from).collect())
        .unwrap_or_default();

    let excluded: HashSet<String> = env::var("EXCLUDED_MEMBERS")
        .map(|v| v.split_whitespace().map(String::from).collect())
        .unwrap_or_default();

    let base_sha = env::var("BASE_SHA").ok().filter(|s| !s.trim().is_empty());

    // Snapshot Cargo.lock from disk BEFORE invoking `cargo metadata`. Cargo
    // will silently rewrite a lockfile it considers inconsistent with the
    // workspace's `Cargo.toml`s, which would defeat the diff. The committed
    // state is what we care about.
    let new_lockfile_snapshot = std::fs::read_to_string("Cargo.lock").ok();

    let mut cmd = MetadataCommand::new();
    let graph = PackageGraph::from_command(&mut cmd)
        .expect("Failed to load package graph. Is this a Cargo workspace?");

    let (lockfile_affected, force_all_override) = lockfile_aware_diff(
        &graph,
        &changed_files,
        &force_triggers,
        base_sha.as_deref(),
        new_lockfile_snapshot.as_deref(),
    );

    let result = compute_affected(
        &graph,
        &changed_files,
        &force_triggers,
        &excluded,
        &lockfile_affected,
        force_all_override,
    );

    emit_output(
        result.force_all,
        result.changed_crates,
        result.affected_library_members,
        result.affected_binary_members,
    );
}

/// If `Cargo.lock` is among the changed files and is NOT explicitly listed in
/// `force_triggers`, diff the old and new lockfile to determine which
/// workspace members are actually affected. Returns:
///
///   - the set of workspace member names affected by the lockfile change
///     (empty if no Cargo.lock change is in scope, or if the diff produced no
///     differences)
///   - `force_all_override`: `true` when the diff could not be computed
///     (missing BASE_SHA, missing old lockfile, parse error). In that case we
///     fall back to the conservative behavior and rebuild everything.
///
/// Users who want the old behavior unconditionally can keep `Cargo.lock` in
/// their `force_triggers`; the new diff is only attempted when it isn't.
fn lockfile_aware_diff(
    graph: &PackageGraph,
    changed_files: &[String],
    force_triggers: &[String],
    base_sha: Option<&str>,
    new_lockfile_snapshot: Option<&str>,
) -> (HashSet<String>, bool) {
    let lockfile_changed = changed_files.iter().any(|f| f == "Cargo.lock");
    if !lockfile_changed {
        return (HashSet::new(), false);
    }
    if rust_affected::check_force_triggers(&["Cargo.lock".to_string()], force_triggers) {
        // User opted into the old behavior by listing Cargo.lock as a force
        // trigger; respect that and skip the diff.
        return (HashSet::new(), false);
    }

    let Some(base_sha) = base_sha else {
        eprintln!(
            "rust-affected: Cargo.lock changed but BASE_SHA is unset; \
             falling back to force_all=true"
        );
        return (HashSet::new(), true);
    };
    let Some(new_lockfile) = new_lockfile_snapshot else {
        eprintln!(
            "rust-affected: failed to read current Cargo.lock; \
             falling back to force_all=true"
        );
        return (HashSet::new(), true);
    };

    let workspace_root = graph.workspace().root().as_std_path();
    let old_lockfile = match git_show::git_show(Path::new(workspace_root), base_sha, "Cargo.lock") {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "rust-affected: could not load old Cargo.lock at {base_sha}: {e}; \
                 falling back to force_all=true"
            );
            return (HashSet::new(), true);
        }
    };

    let member_names: Vec<&str> = graph.workspace().iter().map(|p| p.name()).collect();
    let diff = lockfile::compute_lockfile_diff(&old_lockfile, new_lockfile, &member_names);
    if let Some(reason) = diff.fallback_reason {
        eprintln!(
            "rust-affected: lockfile diff failed ({reason}); \
             falling back to force_all=true"
        );
        return (HashSet::new(), true);
    }
    (diff.affected_members, false)
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
