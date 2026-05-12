use guppy::{MetadataCommand, graph::PackageGraph};
use rust_affected::{compute_affected, git_show, lockfile, manifest};
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

    // Snapshot Cargo.lock and Cargo.toml from disk BEFORE invoking
    // `cargo metadata`. Cargo can silently rewrite either when it considers
    // the manifest/lockfile inconsistent during resolution; the committed
    // state is what we want to diff.
    let new_lockfile_snapshot = std::fs::read_to_string("Cargo.lock").ok();
    let new_manifest_snapshot = std::fs::read_to_string("Cargo.toml").ok();

    let mut cmd = MetadataCommand::new();
    let graph = PackageGraph::from_command(&mut cmd)
        .expect("Failed to load package graph. Is this a Cargo workspace?");

    let workspace_root = graph.workspace().root().as_std_path().to_path_buf();
    let member_names: Vec<String> = graph
        .workspace()
        .iter()
        .map(|p| p.name().to_string())
        .collect();

    let (lockfile_affected, lockfile_force_all) = lockfile_aware_diff(
        &workspace_root,
        &member_names,
        &changed_files,
        &force_triggers,
        base_sha.as_deref(),
        new_lockfile_snapshot.as_deref(),
    );

    let manifest_force_all = manifest_aware_force_all(
        &workspace_root,
        &changed_files,
        &force_triggers,
        base_sha.as_deref(),
        new_manifest_snapshot.as_deref(),
    );

    let force_all_override = lockfile_force_all || manifest_force_all;

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
    workspace_root: &Path,
    member_names: &[String],
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

    let old_lockfile = match git_show::git_show(workspace_root, base_sha, "Cargo.lock") {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "rust-affected: could not load old Cargo.lock at {base_sha}: {e}; \
                 falling back to force_all=true"
            );
            return (HashSet::new(), true);
        }
    };

    let member_refs: Vec<&str> = member_names.iter().map(String::as_str).collect();
    let diff = lockfile::compute_lockfile_diff(&old_lockfile, new_lockfile, &member_refs);
    if let Some(reason) = diff.fallback_reason {
        eprintln!(
            "rust-affected: lockfile diff failed ({reason}); \
             falling back to force_all=true"
        );
        return (HashSet::new(), true);
    }
    (diff.affected_members, false)
}

fn manifest_aware_force_all(
    workspace_root: &Path,
    changed_files: &[String],
    force_triggers: &[String],
    base_sha: Option<&str>,
    new_manifest_snapshot: Option<&str>,
) -> bool {
    let manifest_changed = changed_files.iter().any(|f| f == "Cargo.toml");
    if !manifest_changed {
        return false;
    }
    if rust_affected::check_force_triggers(&["Cargo.toml".to_string()], force_triggers) {
        // User opted into the old behavior by listing Cargo.toml as a force
        // trigger; respect that and skip the diff. The existing
        // check_force_triggers result will set force_all elsewhere.
        return false;
    }

    let Some(base_sha) = base_sha else {
        eprintln!(
            "rust-affected: Cargo.toml changed but BASE_SHA is unset; \
             falling back to force_all=true"
        );
        return true;
    };
    let Some(new_manifest) = new_manifest_snapshot else {
        eprintln!(
            "rust-affected: failed to read current Cargo.toml; \
             falling back to force_all=true"
        );
        return true;
    };

    let old_manifest = match git_show::git_show(workspace_root, base_sha, "Cargo.toml") {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "rust-affected: could not load old Cargo.toml at {base_sha}: {e}; \
                 falling back to force_all=true"
            );
            return true;
        }
    };

    let diff = manifest::compute_manifest_diff(&old_manifest, new_manifest);
    if let Some(reason) = diff.fallback_reason {
        eprintln!(
            "rust-affected: manifest diff failed ({reason}); \
             falling back to force_all=true"
        );
        return true;
    }
    diff.force_all
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

    // Write a job summary when running inside GitHub Actions.
    if let Ok(path) = env::var("GITHUB_STEP_SUMMARY") {
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("Failed to open GITHUB_STEP_SUMMARY");

        let fmt_inline = |items: &[String]| -> String {
            if items.is_empty() {
                String::new()
            } else {
                items
                    .iter()
                    .map(|s| format!("`{s}`"))
                    .collect::<Vec<_>>()
                    .join(" ")
            }
        };

        let fmt_list = |items: &[String]| -> String {
            if items.is_empty() {
                "_none_".to_string()
            } else {
                items
                    .iter()
                    .map(|s| format!("- `{s}`"))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        };

        if force {
            writeln!(
                file,
                "> [!WARNING]\n> **Force all** — a global trigger file changed, entire workspace affected.\n"
            )
            .unwrap();
        }

        writeln!(file, "## rust-affected\n").unwrap();
        writeln!(file, "| | Crates |").unwrap();
        writeln!(file, "|---|---|").unwrap();
        writeln!(file, "| **Changed** | {} |", fmt_inline(&changed)).unwrap();
        writeln!(
            file,
            "| **Affected libraries** | {} |",
            fmt_inline(&affected)
        )
        .unwrap();
        writeln!(
            file,
            "| **Affected binaries** | {} |",
            fmt_inline(&binaries)
        )
        .unwrap();
        writeln!(file).unwrap();
        writeln!(file, "### Changed crates\n{}", fmt_list(&changed)).unwrap();
        writeln!(
            file,
            "\n### Affected library members\n{}",
            fmt_list(&affected)
        )
        .unwrap();
        writeln!(
            file,
            "\n### Affected binary members\n{}",
            fmt_list(&binaries)
        )
        .unwrap();
    }
}
