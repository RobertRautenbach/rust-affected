use rust_affected::WorkspaceGraph;
use rust_affected::{AffectedResult, compute_affected};
use std::process::Command;
use std::{collections::HashSet, path::PathBuf, sync::OnceLock};

fn graph(name: &str) -> &'static WorkspaceGraph {
    static EDGES: OnceLock<WorkspaceGraph> = OnceLock::new();
    static ROOT: OnceLock<WorkspaceGraph> = OnceLock::new();
    static NESTED: OnceLock<WorkspaceGraph> = OnceLock::new();
    static VIRTUAL: OnceLock<WorkspaceGraph> = OnceLock::new();
    static COLLISION: OnceLock<WorkspaceGraph> = OnceLock::new();
    let slot = match name {
        "edge-kinds" => &EDGES,
        "root-package" => &ROOT,
        "nested-package" => &NESTED,
        "workspace" => &VIRTUAL,
        "name-collision/workspace" => &COLLISION,
        _ => panic!("unknown fixture"),
    };
    slot.get_or_init(|| {
        let output = Command::new(env!("CARGO"))
            .args([
                "metadata",
                "--format-version",
                "1",
                "--all-features",
                "--locked",
                "--offline",
            ])
            .env("CARGO_CACHE_RUSTC_INFO", "0")
            .current_dir(
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures")
                    .join(name),
            )
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        WorkspaceGraph::from_json(std::str::from_utf8(&output.stdout).unwrap()).unwrap()
    })
}

#[test]
fn force_all_filters_external_packages_by_identity() {
    let result = compute(
        "name-collision/workspace",
        &["Cargo.toml"],
        &["Cargo.toml"],
        &[],
    );
    assert!(result.force_all);
    assert_eq!(result.changed_crates, Vec::<String>::new());
    assert_eq!(result.affected_library_members, ["consumer", "shared"]);
    assert_eq!(result.affected_binary_members, Vec::<String>::new());
    assert_eq!(result.warnings, Vec::<rust_affected::Warning>::new());
}

#[test]
fn excluding_a_member_does_not_leak_an_external_namesake() {
    let result = compute(
        "name-collision/workspace",
        &["Cargo.toml"],
        &["Cargo.toml"],
        &["shared"],
    );
    assert!(result.force_all);
    assert_eq!(result.changed_crates, Vec::<String>::new());
    assert_eq!(result.affected_library_members, ["consumer"]);
    assert_eq!(result.affected_binary_members, Vec::<String>::new());
    assert_eq!(result.warnings, Vec::<rust_affected::Warning>::new());
}

#[test]
fn ordinary_changes_with_external_namesakes_keep_reverse_dependents() {
    let result = compute("name-collision/workspace", &["member/src/lib.rs"], &[], &[]);
    assert_lists(&result, &["shared"], &["consumer", "shared"], &[]);
}

#[test]
fn default_result_is_empty_without_warnings_or_force() {
    assert_eq!(
        AffectedResult::default(),
        AffectedResult {
            warnings: vec![],
            force_all: false,
            changed_crates: vec![],
            affected_library_members: vec![],
            affected_binary_members: vec![],
        }
    );
}

#[test]
fn serialized_result_has_only_the_four_output_fields() {
    let result = compute(
        "root-package",
        &["sub-lib/src/lib.rs", "../bad"],
        &["sub-lib/**"],
        &["unknown"],
    );
    assert_eq!(result.warnings.len(), 2);
    assert_eq!(
        serde_json::to_value(&result).unwrap(),
        serde_json::json!({
            "changed_crates": ["sub-lib"],
            "affected_library_members": ["independent-lib", "sub-lib"],
            "affected_binary_members": ["root-app"],
            "force_all": true,
        })
    );
}

#[test]
fn exclusions_support_a_non_default_hasher() {
    use std::hash::{BuildHasherDefault, DefaultHasher};
    let excluded: HashSet<String, BuildHasherDefault<DefaultHasher>> =
        ["shared".to_string()].into_iter().collect();
    let result = compute_affected(
        graph("name-collision/workspace"),
        &["member/src/lib.rs".into()],
        &[],
        &excluded,
    )
    .unwrap();
    assert_lists(&result, &[], &["consumer"], &[]);
    assert_eq!(result.warnings, Vec::<rust_affected::Warning>::new());
}

fn compute(name: &str, files: &[&str], triggers: &[&str], excluded: &[&str]) -> AffectedResult {
    compute_affected(
        graph(name),
        &files.iter().map(ToString::to_string).collect::<Vec<_>>(),
        &triggers.iter().map(ToString::to_string).collect::<Vec<_>>(),
        &excluded
            .iter()
            .map(ToString::to_string)
            .collect::<HashSet<_>>(),
    )
    .unwrap()
}

fn assert_lists(result: &AffectedResult, changed: &[&str], libraries: &[&str], binaries: &[&str]) {
    assert_eq!(result.changed_crates, changed);
    assert_eq!(result.affected_library_members, libraries);
    assert_eq!(result.affected_binary_members, binaries);
    assert!(!result.force_all);
}

#[test]
fn deepest_member_owns_files_and_directories() {
    for file in [
        "sub-lib/src/lib.rs",
        "sub-lib/Cargo.toml",
        "sub-lib",
        "sub-lib/",
    ] {
        assert_lists(
            &compute("root-package", &[file], &[], &[]),
            &["sub-lib"],
            &["sub-lib"],
            &["root-app"],
        );
    }
    for file in [
        "parent/nested/src/lib.rs",
        "parent/nested/Cargo.toml",
        "parent/nested",
        "parent/nested/",
    ] {
        assert_lists(
            &compute("nested-package", &[file], &[], &[]),
            &["nested"],
            &["nested", "parent"],
            &[],
        );
    }
}

#[test]
fn directory_ancestry_is_not_a_dependency() {
    assert_lists(
        &compute("root-package", &["independent-lib/src/lib.rs"], &[], &[]),
        &["independent-lib"],
        &["independent-lib"],
        &[],
    );
    assert_lists(
        &compute(
            "nested-package",
            &["parent/independent/src/lib.rs"],
            &[],
            &[],
        ),
        &["independent"],
        &["independent"],
        &[],
    );
}

#[test]
fn parent_and_root_files_keep_their_owner() {
    for file in ["src/main.rs", "Cargo.lock", "README.md"] {
        assert_lists(
            &compute("root-package", &[file], &[], &[]),
            &["root-app"],
            &[],
            &["root-app"],
        );
    }
    for file in [
        "parent/src/lib.rs",
        "parent/Cargo.toml",
        "parent/nested-extra/deleted.rs",
    ] {
        assert_lists(
            &compute("nested-package", &[file], &[], &[]),
            &["parent"],
            &["parent"],
            &[],
        );
    }
}

#[test]
fn exclusions_never_reassign_ownership_or_prune_traversal() {
    assert_lists(
        &compute("root-package", &["sub-lib/src/lib.rs"], &[], &["sub-lib"]),
        &[],
        &[],
        &["root-app"],
    );
    assert_lists(
        &compute(
            "root-package",
            &["independent-lib/src/lib.rs"],
            &[],
            &["independent-lib"],
        ),
        &[],
        &[],
        &[],
    );
    assert_lists(
        &compute(
            "nested-package",
            &["parent/independent/src/lib.rs"],
            &[],
            &["independent"],
        ),
        &[],
        &[],
        &[],
    );
    assert_lists(
        &compute("root-package", &["sub-lib/src/lib.rs"], &[], &["root-app"]),
        &["sub-lib"],
        &["sub-lib"],
        &[],
    );
}

#[test]
fn equivalent_paths_produce_identical_results() {
    let absolute = graph("workspace").root().join("lib-core/src/lib.rs");
    let expected = compute("workspace", &["lib-core/src/lib.rs"], &[], &[]);
    for file in [
        "./lib-core/src/lib.rs",
        "././lib-core/src/lib.rs",
        "lib-core//src/./lib.rs",
        absolute.to_str().unwrap(),
    ] {
        assert_eq!(compute("workspace", &[file], &[], &[]), expected, "{file}");
    }
}

#[test]
fn invalid_paths_cannot_own_files_or_force_the_workspace() {
    for file in ["", ".", "./", "/", "/elsewhere/x", "../x", "sub-lib/../x"] {
        assert_lists(
            &compute("root-package", &[file], &["**"], &[]),
            &[],
            &[],
            &[],
        );
    }
}

#[test]
fn normalized_paths_and_patterns_activate_force_triggers() {
    let absolute = graph("workspace").root().join("Cargo.lock");
    for (file, trigger) in [
        ("./Cargo.lock", "Cargo.lock"),
        ("Cargo.lock", "././Cargo.lock"),
        (absolute.to_str().unwrap(), "Cargo.lock"),
        ("./infra/deploy.yml", "././infra/"),
    ] {
        assert!(compute("workspace", &[file], &[trigger], &[]).force_all);
    }
}

#[test]
fn exclusions_are_normalized_component_prefixes() {
    for exclude in ["./parent/nested", "parent//nested/", "././parent/nested/"] {
        assert_lists(
            &compute(
                "nested-package",
                &["parent/nested/src/lib.rs"],
                &[],
                &[exclude],
            ),
            &[],
            &["parent"],
            &[],
        );
    }
    for exclude in ["/", ".", "./", "parent/../", "/parent/"] {
        assert_lists(
            &compute("root-package", &["src/main.rs"], &[], &[exclude]),
            &["root-app"],
            &[],
            &["root-app"],
        );
    }
    // The directory is "parent/independent", but the crate name is "independent".
    assert_lists(
        &compute(
            "nested-package",
            &["parent/independent/gone.rs"],
            &[],
            &["./independent"],
        ),
        &["independent"],
        &["independent"],
        &[],
    );
}

#[test]
fn directory_force_triggers_match_the_directory_and_descendants_only() {
    for trigger in ["infra/", "./infra/", "././infra/"] {
        for file in [
            "infra",
            "infra/",
            "./infra",
            "infra/deploy.yml",
            "infra/nested/deploy.yml",
        ] {
            let result = compute("workspace", &[file], &[trigger], &["app-alpha"]);
            assert!(result.force_all, "{file} with {trigger}");
            assert_eq!(result.changed_crates, Vec::<String>::new());
            assert_eq!(result.affected_binary_members, ["app-beta", "tool-alpha"]);
        }
        for file in [
            "infra-extra",
            "infra-extra/deploy.yml",
            "other/infra",
            "other/infra/deploy.yml",
        ] {
            assert!(
                !compute("workspace", &[file], &[trigger], &[]).force_all,
                "{file} with {trigger}"
            );
        }
    }
}

#[test]
fn deleted_files_renames_and_duplicates_are_lexical() {
    assert_lists(
        &compute(
            "nested-package",
            &[
                "parent/independent/deleted.rs",
                "parent/nested/new.rs",
                "parent/nested/new.rs",
                "parent/nested/another.rs",
            ],
            &[],
            &[],
        ),
        &["independent", "nested"],
        &["independent", "nested", "parent"],
        &[],
    );
}

#[test]
fn warnings_are_deterministic_and_checked_against_all_members() {
    use rust_affected::Warning;
    let result = compute(
        "workspace",
        &["../z", "../a", "../z"],
        &[],
        &["tools", "unknown"],
    );
    assert_eq!(
        result.warnings,
        vec![
            Warning::UnusablePath("../a".into()),
            Warning::UnusablePath("../z".into()),
            Warning::UnmatchedExclusion("tools".into()),
            Warning::UnmatchedExclusion("unknown".into()),
        ]
    );
    assert_eq!(
        compute("workspace", &[], &[], &["tools/", "./tools/", "lib-core"]).warnings,
        Vec::<rust_affected::Warning>::new()
    );
    assert_eq!(
        compute("workspace", &["lib-core/src/lib.rs"], &[], &[]).warnings,
        Vec::<rust_affected::Warning>::new()
    );
}

#[test]
fn library_reports_invalid_globs_even_for_empty_input() {
    use std::error::Error as _;
    let error = compute_affected(
        graph("workspace"),
        &[],
        &["[abc".into()],
        &HashSet::<String>::new(),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        rust_affected::Error::InvalidForceTrigger(_)
    ));
    assert!(error.source().is_some());
    assert!(error.to_string().contains("[abc"));
    assert!(rust_affected::check_force_triggers(&[], &["[abc".into()]).is_err());
}

#[test]
fn every_declared_local_dependency_kind_is_followed() {
    let result = compute("edge-kinds", &["graph-core/src/lib.rs"], &[], &[]);
    assert_lists(
        &result,
        &["graph-core"],
        &["build", "dev", "graph-core", "normal", "optional", "target"],
        &["top"],
    );
    let excluded = compute(
        "edge-kinds",
        &["graph-core/src/lib.rs"],
        &[],
        &["normal", "optional"],
    );
    assert_lists(
        &excluded,
        &["graph-core"],
        &["build", "dev", "graph-core", "target"],
        &["top"],
    );
}
