use guppy::{graph::PackageGraph, MetadataCommand};
use rust_affected::{compute_affected, AffectedResult};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::OnceLock;

fn fixture_graph() -> &'static PackageGraph {
    static GRAPH: OnceLock<PackageGraph> = OnceLock::new();
    GRAPH.get_or_init(|| {
        let fixture_dir: PathBuf =
            [env!("CARGO_MANIFEST_DIR"), "tests", "fixtures", "workspace"]
                .iter()
                .collect();
        let mut cmd = MetadataCommand::new();
        cmd.current_dir(&fixture_dir);
        PackageGraph::from_command(&mut cmd).expect("Failed to load fixture package graph")
    })
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn no_excludes() -> HashSet<String> {
    HashSet::new()
}

fn excludes(names: &[&str]) -> HashSet<String> {
    names.iter().map(|s| s.to_string()).collect()
}

// ── No changed files ────────────────────────────────────────────────

#[test]
fn empty_changed_files_produces_empty_result() {
    let graph = fixture_graph();
    let result = compute_affected(graph, &[], &[], &no_excludes());
    assert_eq!(
        result,
        AffectedResult {
            force_all: false,
            changed_crates: vec![],
            affected_library_members: vec![],
            affected_binary_members: vec![],
        }
    );
}

// ── Leaf library change (lib-utils) ─────────────────────────────────

#[test]
fn change_leaf_lib_affects_all_dependents() {
    let graph = fixture_graph();
    let changed = s(&["lib-utils/src/lib.rs"]);
    let result = compute_affected(graph, &changed, &[], &no_excludes());

    assert_eq!(result.changed_crates, vec!["lib-utils"]);
    assert_eq!(
        result.affected_library_members,
        vec!["app-alpha", "app-beta", "lib-core", "lib-utils"]
    );
    assert_eq!(
        result.affected_binary_members,
        vec!["app-alpha", "app-beta"]
    );
    assert!(!result.force_all);
}

// ── Mid-tree library change (lib-core) ──────────────────────────────

#[test]
fn change_mid_tree_lib_affects_its_dependents() {
    let graph = fixture_graph();
    let changed = s(&["lib-core/src/lib.rs"]);
    let result = compute_affected(graph, &changed, &[], &no_excludes());

    assert_eq!(result.changed_crates, vec!["lib-core"]);
    assert_eq!(
        result.affected_library_members,
        vec!["app-alpha", "app-beta", "lib-core"]
    );
    assert_eq!(
        result.affected_binary_members,
        vec!["app-alpha", "app-beta"]
    );
}

// ── Isolated library change (lib-standalone) ────────────────────────

#[test]
fn change_isolated_lib_only_affects_its_dependent() {
    let graph = fixture_graph();
    let changed = s(&["lib-standalone/src/lib.rs"]);
    let result = compute_affected(graph, &changed, &[], &no_excludes());

    assert_eq!(result.changed_crates, vec!["lib-standalone"]);
    assert_eq!(
        result.affected_library_members,
        vec!["app-beta", "lib-standalone"]
    );
    assert_eq!(result.affected_binary_members, vec!["app-beta"]);
}

// ── Binary-only change ──────────────────────────────────────────────

#[test]
fn change_binary_only_affects_itself() {
    let graph = fixture_graph();
    let changed = s(&["app-alpha/src/main.rs"]);
    let result = compute_affected(graph, &changed, &[], &no_excludes());

    assert_eq!(result.changed_crates, vec!["app-alpha"]);
    assert_eq!(result.affected_library_members, vec!["app-alpha"]);
    assert_eq!(result.affected_binary_members, vec!["app-alpha"]);
}

// ── Multi-crate change ──────────────────────────────────────────────

#[test]
fn change_multiple_crates_unions_affected() {
    let graph = fixture_graph();
    let changed = s(&["lib-standalone/src/lib.rs", "app-alpha/src/main.rs"]);
    let result = compute_affected(graph, &changed, &[], &no_excludes());

    assert_eq!(result.changed_crates, vec!["app-alpha", "lib-standalone"]);
    assert_eq!(
        result.affected_library_members,
        vec!["app-alpha", "app-beta", "lib-standalone"]
    );
    assert_eq!(
        result.affected_binary_members,
        vec!["app-alpha", "app-beta"]
    );
}

// ── Cargo.toml change in a crate ────────────────────────────────────

#[test]
fn change_cargo_toml_detects_crate() {
    let graph = fixture_graph();
    let changed = s(&["lib-core/Cargo.toml"]);
    let result = compute_affected(graph, &changed, &[], &no_excludes());

    assert_eq!(result.changed_crates, vec!["lib-core"]);
    assert_eq!(
        result.affected_binary_members,
        vec!["app-alpha", "app-beta"]
    );
}

// ── Force triggers: matching ─────────────────────────────────────────

#[test]
fn force_trigger_match_returns_all_members() {
    let graph = fixture_graph();
    let changed = s(&["infra/deploy.yml"]);
    let triggers = s(&["infra/"]);
    let result = compute_affected(graph, &changed, &triggers, &no_excludes());

    assert!(result.force_all);
    assert_eq!(
        result.affected_library_members,
        vec!["app-alpha", "app-beta", "lib-core", "lib-standalone", "lib-utils"]
    );
    assert_eq!(
        result.affected_binary_members,
        vec!["app-alpha", "app-beta"]
    );
}

#[test]
fn force_trigger_glob_pattern_match() {
    let graph = fixture_graph();
    let changed = s(&["ci/workflow.yml"]);
    let triggers = s(&["ci/*.yml"]);
    let result = compute_affected(graph, &changed, &triggers, &no_excludes());

    assert!(result.force_all);
    assert_eq!(
        result.affected_binary_members,
        vec!["app-alpha", "app-beta"]
    );
}

// ── Force triggers: non-matching ─────────────────────────────────────

#[test]
fn force_trigger_no_match_behaves_normally() {
    let graph = fixture_graph();
    let changed = s(&["app-alpha/src/main.rs"]);
    let triggers = s(&["infra/"]);
    let result = compute_affected(graph, &changed, &triggers, &no_excludes());

    assert!(!result.force_all);
    assert_eq!(result.changed_crates, vec!["app-alpha"]);
    assert_eq!(result.affected_binary_members, vec!["app-alpha"]);
}

// ── Excluded members ────────────────────────────────────────────────

#[test]
fn excluded_member_removed_from_all_lists() {
    let graph = fixture_graph();
    let changed = s(&["lib-utils/src/lib.rs"]);
    let excluded = excludes(&["lib-core"]);
    let result = compute_affected(graph, &changed, &[], &excluded);

    assert_eq!(result.changed_crates, vec!["lib-utils"]);
    assert!(!result.affected_library_members.contains(&"lib-core".to_string()));
    assert!(result.affected_library_members.contains(&"lib-utils".to_string()));
    assert!(result.affected_library_members.contains(&"app-alpha".to_string()));
    assert!(result.affected_library_members.contains(&"app-beta".to_string()));
}

#[test]
fn excluded_binary_removed_from_binary_list() {
    let graph = fixture_graph();
    let changed = s(&["lib-core/src/lib.rs"]);
    let excluded = excludes(&["app-alpha"]);
    let result = compute_affected(graph, &changed, &[], &excluded);

    assert!(!result.affected_binary_members.contains(&"app-alpha".to_string()));
    assert_eq!(result.affected_binary_members, vec!["app-beta"]);
}

#[test]
fn excluded_changed_crate_removed_from_changed_list() {
    let graph = fixture_graph();
    let changed = s(&["lib-utils/src/lib.rs"]);
    let excluded = excludes(&["lib-utils"]);
    let result = compute_affected(graph, &changed, &[], &excluded);

    assert!(result.changed_crates.is_empty());
    assert!(!result.affected_library_members.contains(&"lib-utils".to_string()));
}

// ── File outside any crate ──────────────────────────────────────────

#[test]
fn file_outside_any_crate_produces_no_changes() {
    let graph = fixture_graph();
    let changed = s(&["README.md"]);
    let result = compute_affected(graph, &changed, &[], &no_excludes());

    assert!(result.changed_crates.is_empty());
    assert!(result.affected_library_members.is_empty());
    assert!(result.affected_binary_members.is_empty());
}

// ── Force trigger with excluded members ─────────────────────────────

#[test]
fn force_all_respects_exclusions() {
    let graph = fixture_graph();
    let changed = s(&["infra/deploy.yml"]);
    let triggers = s(&["infra/"]);
    let excluded = excludes(&["app-alpha", "lib-standalone"]);
    let result = compute_affected(graph, &changed, &triggers, &excluded);

    assert!(result.force_all);
    assert!(!result.affected_library_members.contains(&"app-alpha".to_string()));
    assert!(!result.affected_library_members.contains(&"lib-standalone".to_string()));
    assert_eq!(
        result.affected_binary_members,
        vec!["app-beta"]
    );
}
