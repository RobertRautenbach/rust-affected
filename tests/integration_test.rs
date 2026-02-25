use guppy::{MetadataCommand, graph::PackageGraph};
use rust_affected::{AffectedResult, compute_affected};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::OnceLock;

fn fixture_graph() -> &'static PackageGraph {
    static GRAPH: OnceLock<PackageGraph> = OnceLock::new();
    GRAPH.get_or_init(|| {
        let fixture_dir: PathBuf = [env!("CARGO_MANIFEST_DIR"), "tests", "fixtures", "workspace"]
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
        vec![
            "app-alpha",
            "app-beta",
            "lib-core",
            "lib-core-ext",
            "lib-utils",
            "tool-alpha"
        ]
    );
    assert_eq!(
        result.affected_binary_members,
        vec!["app-alpha", "app-beta", "tool-alpha"]
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
        vec!["app-alpha", "app-beta", "lib-core", "lib-core-ext"]
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
        vec![
            "app-alpha",
            "app-beta",
            "lib-core",
            "lib-core-ext",
            "lib-standalone",
            "lib-utils",
            "lib-with-tests",
            "tool-alpha"
        ]
    );
    assert_eq!(
        result.affected_binary_members,
        vec!["app-alpha", "app-beta", "tool-alpha"]
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
        vec!["app-alpha", "app-beta", "tool-alpha"]
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
    assert!(
        !result
            .affected_library_members
            .contains(&"lib-core".to_string())
    );
    assert!(
        result
            .affected_library_members
            .contains(&"lib-utils".to_string())
    );
    assert!(
        result
            .affected_library_members
            .contains(&"lib-core-ext".to_string())
    );
    assert!(
        result
            .affected_library_members
            .contains(&"app-alpha".to_string())
    );
    assert!(
        result
            .affected_library_members
            .contains(&"app-beta".to_string())
    );
}

#[test]
fn excluded_binary_removed_from_binary_list() {
    let graph = fixture_graph();
    let changed = s(&["lib-core/src/lib.rs"]);
    let excluded = excludes(&["app-alpha"]);
    let result = compute_affected(graph, &changed, &[], &excluded);

    assert!(
        !result
            .affected_binary_members
            .contains(&"app-alpha".to_string())
    );
    assert_eq!(result.affected_binary_members, vec!["app-beta"]);
}

#[test]
fn excluded_changed_crate_removed_from_changed_list() {
    let graph = fixture_graph();
    let changed = s(&["lib-utils/src/lib.rs"]);
    let excluded = excludes(&["lib-utils"]);
    let result = compute_affected(graph, &changed, &[], &excluded);

    assert!(result.changed_crates.is_empty());
    assert!(
        !result
            .affected_library_members
            .contains(&"lib-utils".to_string())
    );
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
    assert!(
        !result
            .affected_library_members
            .contains(&"app-alpha".to_string())
    );
    assert!(
        !result
            .affected_library_members
            .contains(&"lib-standalone".to_string())
    );
    assert_eq!(
        result.affected_binary_members,
        vec!["app-beta", "tool-alpha"]
    );
}

// ── Transitive dependency chain fully resolved ──────────────────────

#[test]
fn transitive_chain_fully_resolved() {
    let graph = fixture_graph();
    let changed = s(&["lib-utils/src/lib.rs"]);
    let result = compute_affected(graph, &changed, &[], &no_excludes());

    // lib-utils → lib-core → lib-core-ext, app-alpha, app-beta, tool-alpha
    // All transitive dependents must appear, not just direct ones.
    assert!(
        result
            .affected_library_members
            .contains(&"lib-utils".to_string())
    );
    assert!(
        result
            .affected_library_members
            .contains(&"lib-core".to_string())
    );
    assert!(
        result
            .affected_library_members
            .contains(&"lib-core-ext".to_string())
    );
    assert!(
        result
            .affected_library_members
            .contains(&"app-alpha".to_string())
    );
    assert!(
        result
            .affected_library_members
            .contains(&"app-beta".to_string())
    );
    assert!(
        result
            .affected_library_members
            .contains(&"tool-alpha".to_string())
    );
    assert_eq!(result.affected_library_members.len(), 6);
}

// ── Nested file path within a crate ─────────────────────────────────

#[test]
fn nested_file_path_matches_crate() {
    let graph = fixture_graph();
    let changed = s(&["lib-core/src/submodule/deep/file.rs"]);
    let result = compute_affected(graph, &changed, &[], &no_excludes());

    assert_eq!(result.changed_crates, vec!["lib-core"]);
    assert_eq!(
        result.affected_library_members,
        vec!["app-alpha", "app-beta", "lib-core", "lib-core-ext"]
    );
    assert_eq!(
        result.affected_binary_members,
        vec!["app-alpha", "app-beta"]
    );
}

// ── Leaf binary does not pull unrelated crates ──────────────────────

#[test]
fn leaf_binary_does_not_pull_unrelated() {
    let graph = fixture_graph();
    let changed = s(&["app-alpha/src/main.rs"]);
    let result = compute_affected(graph, &changed, &[], &no_excludes());

    assert_eq!(result.changed_crates, vec!["app-alpha"]);
    assert_eq!(result.affected_library_members, vec!["app-alpha"]);
    assert_eq!(result.affected_binary_members, vec!["app-alpha"]);
    // Ensure unrelated crates are absent
    assert!(
        !result
            .affected_library_members
            .contains(&"lib-core".to_string())
    );
    assert!(
        !result
            .affected_library_members
            .contains(&"lib-utils".to_string())
    );
    assert!(
        !result
            .affected_library_members
            .contains(&"lib-standalone".to_string())
    );
    assert!(
        !result
            .affected_library_members
            .contains(&"lib-core-ext".to_string())
    );
}

// ── All crates changed at once — no duplicates ──────────────────────

#[test]
fn all_crates_changed_no_duplicates() {
    let graph = fixture_graph();
    let changed = s(&[
        "lib-utils/src/lib.rs",
        "lib-core/src/lib.rs",
        "lib-core-ext/src/lib.rs",
        "lib-standalone/src/lib.rs",
        "app-alpha/src/main.rs",
        "app-beta/src/main.rs",
    ]);
    let result = compute_affected(graph, &changed, &[], &no_excludes());

    assert_eq!(
        result.changed_crates,
        vec![
            "app-alpha",
            "app-beta",
            "lib-core",
            "lib-core-ext",
            "lib-standalone",
            "lib-utils"
        ]
    );
    // Each name appears exactly once (sorted vec guarantees no adjacent dups)
    let mut deduped = result.changed_crates.clone();
    deduped.dedup();
    assert_eq!(result.changed_crates, deduped);

    assert_eq!(
        result.affected_binary_members,
        vec!["app-alpha", "app-beta", "tool-alpha"]
    );
}

// ── Force trigger overlapping with a normal crate change ────────────

#[test]
fn force_trigger_with_normal_change_overlap() {
    let graph = fixture_graph();
    let changed = s(&["lib-core/src/lib.rs", "infra/deploy.yml"]);
    let triggers = s(&["infra/"]);
    let result = compute_affected(graph, &changed, &triggers, &no_excludes());

    assert!(result.force_all);
    // Direct change to lib-core is still tracked
    assert_eq!(result.changed_crates, vec!["lib-core"]);
    // But all members appear in affected (force_all)
    assert_eq!(
        result.affected_library_members,
        vec![
            "app-alpha",
            "app-beta",
            "lib-core",
            "lib-core-ext",
            "lib-standalone",
            "lib-utils",
            "lib-with-tests",
            "tool-alpha"
        ]
    );
}

// ── Excluded binary absent from all lists ───────────────────────────

#[test]
fn excluded_binary_not_in_any_list() {
    let graph = fixture_graph();
    let changed = s(&["lib-utils/src/lib.rs"]);
    let excluded = excludes(&["app-alpha"]);
    let result = compute_affected(graph, &changed, &[], &excluded);

    assert!(
        !result
            .affected_library_members
            .contains(&"app-alpha".to_string())
    );
    assert!(
        !result
            .affected_binary_members
            .contains(&"app-alpha".to_string())
    );
    assert!(
        result
            .affected_library_members
            .contains(&"app-beta".to_string())
    );
    assert!(
        result
            .affected_binary_members
            .contains(&"app-beta".to_string())
    );
}

// ── Excluded mid-graph crate still traversed ────────────────────────

#[test]
fn excluded_mid_graph_crate_still_traversed() {
    let graph = fixture_graph();
    let changed = s(&["lib-utils/src/lib.rs"]);
    let excluded = excludes(&["lib-core"]);
    let result = compute_affected(graph, &changed, &[], &excluded);

    // lib-core is excluded from output…
    assert!(!result.changed_crates.contains(&"lib-core".to_string()));
    assert!(
        !result
            .affected_library_members
            .contains(&"lib-core".to_string())
    );
    // …but dependents of lib-core are still reachable via graph traversal
    assert!(
        result
            .affected_library_members
            .contains(&"app-alpha".to_string())
    );
    assert!(
        result
            .affected_library_members
            .contains(&"app-beta".to_string())
    );
    assert!(
        result
            .affected_library_members
            .contains(&"lib-core-ext".to_string())
    );
    assert!(
        result
            .affected_binary_members
            .contains(&"app-alpha".to_string())
    );
    assert!(
        result
            .affected_binary_members
            .contains(&"app-beta".to_string())
    );
}

// ── Build script / non-src file in a crate directory ────────────────

#[test]
fn build_script_in_crate_dir_detected() {
    let graph = fixture_graph();
    let changed = s(&["lib-core/build.rs"]);
    let result = compute_affected(graph, &changed, &[], &no_excludes());

    assert_eq!(result.changed_crates, vec!["lib-core"]);
    assert_eq!(
        result.affected_binary_members,
        vec!["app-alpha", "app-beta"]
    );
}

// ── Path prefix false positive prevention ───────────────────────────

#[test]
fn path_prefix_no_false_positive() {
    let graph = fixture_graph();
    let changed = s(&["lib-core-ext/src/lib.rs"]);
    let result = compute_affected(graph, &changed, &[], &no_excludes());

    // Only lib-core-ext itself should be in changed_crates, NOT lib-core
    assert_eq!(result.changed_crates, vec!["lib-core-ext"]);
    assert!(!result.changed_crates.contains(&"lib-core".to_string()));
    // lib-core-ext has no reverse dependents, so affected is just itself
    assert_eq!(result.affected_library_members, vec!["lib-core-ext"]);
    assert!(result.affected_binary_members.is_empty());
}

// ── Path-based exclusion ────────────────────────────────────────────

#[test]
fn path_prefix_excludes_nested_crate() {
    let graph = fixture_graph();
    // tool-alpha lives under tools/ and depends on lib-utils.
    // Changing lib-utils makes tool-alpha affected, but excluding "tools/"
    // should remove it from all output lists.
    let changed = s(&["lib-utils/src/lib.rs"]);
    let excluded = excludes(&["tools/"]);
    let result = compute_affected(graph, &changed, &[], &excluded);

    assert!(
        !result
            .affected_library_members
            .contains(&"tool-alpha".to_string())
    );
    assert!(
        !result
            .affected_binary_members
            .contains(&"tool-alpha".to_string())
    );
    // Other dependents of lib-utils are still present
    assert!(
        result
            .affected_library_members
            .contains(&"app-alpha".to_string())
    );
    assert!(
        result
            .affected_library_members
            .contains(&"app-beta".to_string())
    );
}

#[test]
fn path_prefix_excludes_nested_crate_from_force_all() {
    let graph = fixture_graph();
    let changed = s(&["infra/deploy.yml"]);
    let triggers = s(&["infra/"]);
    let excluded = excludes(&["tools/"]);
    let result = compute_affected(graph, &changed, &triggers, &excluded);

    assert!(result.force_all);
    assert!(
        !result
            .affected_library_members
            .contains(&"tool-alpha".to_string())
    );
    assert!(
        !result
            .affected_binary_members
            .contains(&"tool-alpha".to_string())
    );
    // Other members still appear
    assert!(
        result
            .affected_library_members
            .contains(&"app-alpha".to_string())
    );
}

#[test]
fn path_prefix_excludes_direct_change_in_nested_crate() {
    let graph = fixture_graph();
    let changed = s(&["tools/tool-alpha/src/main.rs"]);
    let excluded = excludes(&["tools/"]);
    let result = compute_affected(graph, &changed, &[], &excluded);

    assert!(result.changed_crates.is_empty());
    assert!(result.affected_library_members.is_empty());
    assert!(result.affected_binary_members.is_empty());
}

#[test]
fn exact_path_excludes_specific_nested_crate() {
    let graph = fixture_graph();
    // Exclude by exact relative directory path (no trailing slash)
    let changed = s(&["lib-utils/src/lib.rs"]);
    let excluded = excludes(&["tools/tool-alpha"]);
    let result = compute_affected(graph, &changed, &[], &excluded);

    assert!(
        !result
            .affected_library_members
            .contains(&"tool-alpha".to_string())
    );
    assert!(
        !result
            .affected_binary_members
            .contains(&"tool-alpha".to_string())
    );
}

#[test]
fn nested_crate_detected_when_directly_changed() {
    let graph = fixture_graph();
    let changed = s(&["tools/tool-alpha/src/main.rs"]);
    let result = compute_affected(graph, &changed, &[], &no_excludes());

    assert_eq!(result.changed_crates, vec!["tool-alpha"]);
    assert_eq!(result.affected_library_members, vec!["tool-alpha"]);
    assert_eq!(result.affected_binary_members, vec!["tool-alpha"]);
}

// ── Library crate with integration tests is not a binary ──────────────

#[test]
fn library_with_tests_is_not_binary() {
    let graph = fixture_graph();
    let changed = s(&["lib-with-tests/src/lib.rs"]);
    let result = compute_affected(graph, &changed, &[], &no_excludes());

    assert_eq!(result.changed_crates, vec!["lib-with-tests"]);
    assert_eq!(result.affected_library_members, vec!["lib-with-tests"]);
    // lib-with-tests has integration tests but no main.rs — it must NOT
    // appear in affected_binary_members.
    assert!(result.affected_binary_members.is_empty());
}

#[test]
fn library_with_tests_excluded_from_binaries_on_force_all() {
    let graph = fixture_graph();
    let changed = s(&["infra/deploy.yml"]);
    let triggers = s(&["infra/"]);
    let result = compute_affected(graph, &changed, &triggers, &no_excludes());

    assert!(result.force_all);
    // lib-with-tests should appear in library members but NOT binary members
    assert!(
        result
            .affected_library_members
            .contains(&"lib-with-tests".to_string())
    );
    assert!(
        !result
            .affected_binary_members
            .contains(&"lib-with-tests".to_string())
    );
}
