use guppy::{MetadataCommand, graph::PackageGraph};
use rust_affected::{AffectedResult, compute_affected};
use std::{collections::HashSet, path::PathBuf, sync::OnceLock};

fn graph(name: &str) -> &'static PackageGraph {
    static ROOT: OnceLock<PackageGraph> = OnceLock::new();
    static NESTED: OnceLock<PackageGraph> = OnceLock::new();
    static VIRTUAL: OnceLock<PackageGraph> = OnceLock::new();
    let slot = match name {
        "root-package" => &ROOT,
        "nested-package" => &NESTED,
        "workspace" => &VIRTUAL,
        _ => panic!("unknown fixture"),
    };
    slot.get_or_init(|| {
        MetadataCommand::new()
            .current_dir(
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures")
                    .join(name),
            )
            .other_options(["--locked"])
            .build_graph()
            .unwrap()
    })
}

fn compute(name: &str, files: &[&str], triggers: &[&str], excluded: &[&str]) -> AffectedResult {
    compute_affected(
        graph(name),
        &files.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        &triggers.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        &excluded
            .iter()
            .map(|s| s.to_string())
            .collect::<HashSet<_>>(),
    )
    .unwrap()
}

fn assert_lists(result: AffectedResult, changed: &[&str], libraries: &[&str], binaries: &[&str]) {
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
            compute("root-package", &[file], &[], &[]),
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
            compute("nested-package", &[file], &[], &[]),
            &["nested"],
            &["nested", "parent"],
            &[],
        );
    }
}

#[test]
fn directory_ancestry_is_not_a_dependency() {
    assert_lists(
        compute("root-package", &["independent-lib/src/lib.rs"], &[], &[]),
        &["independent-lib"],
        &["independent-lib"],
        &[],
    );
    assert_lists(
        compute(
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
            compute("root-package", &[file], &[], &[]),
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
            compute("nested-package", &[file], &[], &[]),
            &["parent"],
            &["parent"],
            &[],
        );
    }
}

#[test]
fn exclusions_never_reassign_ownership_or_prune_traversal() {
    assert_lists(
        compute("root-package", &["sub-lib/src/lib.rs"], &[], &["sub-lib"]),
        &[],
        &[],
        &["root-app"],
    );
    assert_lists(
        compute(
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
        compute(
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
        compute("root-package", &["sub-lib/src/lib.rs"], &[], &["root-app"]),
        &["sub-lib"],
        &["sub-lib"],
        &[],
    );
}

#[test]
fn equivalent_paths_produce_identical_results() {
    let absolute = graph("workspace")
        .workspace()
        .root()
        .join("lib-core/src/lib.rs");
    let expected = compute("workspace", &["lib-core/src/lib.rs"], &[], &[]);
    for file in [
        "./lib-core/src/lib.rs",
        "././lib-core/src/lib.rs",
        "lib-core//src/./lib.rs",
        absolute.as_str(),
    ] {
        assert_eq!(compute("workspace", &[file], &[], &[]), expected, "{file}");
    }
}

#[test]
fn invalid_paths_cannot_own_files_or_force_the_workspace() {
    for file in ["", ".", "./", "/", "/elsewhere/x", "../x", "sub-lib/../x"] {
        assert_lists(
            compute("root-package", &[file], &["**"], &[]),
            &[],
            &[],
            &[],
        );
    }
}

#[test]
fn normalized_paths_and_patterns_activate_force_triggers() {
    let absolute = graph("workspace").workspace().root().join("Cargo.lock");
    for (file, trigger) in [
        ("./Cargo.lock", "Cargo.lock"),
        ("Cargo.lock", "././Cargo.lock"),
        (absolute.as_str(), "Cargo.lock"),
        ("./infra/deploy.yml", "././infra/"),
    ] {
        assert!(compute("workspace", &[file], &[trigger], &[]).force_all);
    }
}

#[test]
fn exclusions_are_normalized_component_prefixes() {
    for exclude in ["./parent/nested", "parent//nested/", "././parent/nested/"] {
        assert_lists(
            compute(
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
            compute("root-package", &["src/main.rs"], &[], &[exclude]),
            &["root-app"],
            &[],
            &["root-app"],
        );
    }
    // The directory is "parent/independent", but the crate name is "independent".
    assert_lists(
        compute(
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
fn deleted_files_renames_and_duplicates_are_lexical() {
    assert_lists(
        compute(
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
    assert!(
        compute("workspace", &[], &[], &["tools/", "./tools/", "lib-core"])
            .warnings
            .is_empty()
    );
    assert!(
        compute("workspace", &["lib-core/src/lib.rs"], &[], &[])
            .warnings
            .is_empty()
    );
}

#[test]
fn library_reports_invalid_globs_even_for_empty_input() {
    use std::error::Error as _;
    let error =
        compute_affected(graph("workspace"), &[], &["[abc".into()], &HashSet::new()).unwrap_err();
    assert!(matches!(
        error,
        rust_affected::Error::InvalidForceTrigger(_)
    ));
    assert!(error.source().is_some());
    assert!(error.to_string().contains("[abc"));
    assert!(rust_affected::check_force_triggers(&[], &["[abc".into()]).is_err());
}
