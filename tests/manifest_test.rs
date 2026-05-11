use rust_affected::manifest::compute_manifest_diff;

const VIRTUAL_WORKSPACE: &str = r#"
[workspace]
resolver = "2"
members = ["app-alpha", "lib-core"]

[workspace.package]
edition = "2021"
license = "MIT"

[workspace.dependencies]
serde = "1.0.200"
anyhow = { version = "1.0", features = ["std"] }

[profile.release]
opt-level = "z"
lto = true
"#;

fn diff(old: &str, new: &str) -> rust_affected::manifest::ManifestDiff {
    compute_manifest_diff(old, new)
}

// ── No-op / metadata churn ──────────────────────────────────────────

#[test]
fn identical_manifests_produce_no_force_all() {
    let d = diff(VIRTUAL_WORKSPACE, VIRTUAL_WORKSPACE);
    assert!(d.fallback_reason.is_none());
    assert!(!d.force_all);
}

#[test]
fn whitespace_only_diff_is_no_force_all() {
    let new = VIRTUAL_WORKSPACE.replace("\n[workspace]", "\n\n\n[workspace]");
    let d = diff(VIRTUAL_WORKSPACE, &new);
    assert!(d.fallback_reason.is_none());
    assert!(!d.force_all);
}

#[test]
fn comment_only_diff_is_no_force_all() {
    let new = format!("# a fresh comment\n{VIRTUAL_WORKSPACE}");
    let d = diff(VIRTUAL_WORKSPACE, &new);
    assert!(d.fallback_reason.is_none());
    assert!(!d.force_all);
}

#[test]
fn workspace_package_description_change_is_no_force_all() {
    let old = format!("{VIRTUAL_WORKSPACE}\n[workspace.package]\ndescription = \"old\"\n");
    let new = format!("{VIRTUAL_WORKSPACE}\n[workspace.package]\ndescription = \"new\"\n");
    let d = diff(&old, &new);
    assert!(!d.force_all);
}

// ── Workspace member adds/removes ───────────────────────────────────

#[test]
fn adding_workspace_member_is_no_force_all() {
    let new = VIRTUAL_WORKSPACE.replace(
        "members = [\"app-alpha\", \"lib-core\"]",
        "members = [\"app-alpha\", \"lib-core\", \"lib-new\"]",
    );
    let d = diff(VIRTUAL_WORKSPACE, &new);
    assert!(!d.force_all, "adding a workspace member must not force-all");
}

#[test]
fn removing_workspace_member_is_no_force_all() {
    let new = VIRTUAL_WORKSPACE.replace(
        "members = [\"app-alpha\", \"lib-core\"]",
        "members = [\"app-alpha\"]",
    );
    let d = diff(VIRTUAL_WORKSPACE, &new);
    assert!(!d.force_all);
}

#[test]
fn reordering_members_is_no_force_all() {
    let new = VIRTUAL_WORKSPACE.replace(
        "members = [\"app-alpha\", \"lib-core\"]",
        "members = [\"lib-core\", \"app-alpha\"]",
    );
    let d = diff(VIRTUAL_WORKSPACE, &new);
    assert!(!d.force_all);
}

// ── workspace.dependencies — lockfile-reflected fields ──────────────

#[test]
fn workspace_dep_version_bump_is_no_force_all() {
    let new = VIRTUAL_WORKSPACE.replace("serde = \"1.0.200\"", "serde = \"1.0.201\"");
    let d = diff(VIRTUAL_WORKSPACE, &new);
    assert!(!d.force_all);
}

#[test]
fn workspace_dep_added_is_no_force_all() {
    let new = VIRTUAL_WORKSPACE.replace(
        "serde = \"1.0.200\"",
        "serde = \"1.0.200\"\ntokio = \"1.40\"",
    );
    let d = diff(VIRTUAL_WORKSPACE, &new);
    assert!(!d.force_all);
}

#[test]
fn workspace_dep_path_change_is_no_force_all() {
    let old = r#"[workspace]
members = []
[workspace.dependencies]
foo = { path = "old/path" }
"#;
    let new = r#"[workspace]
members = []
[workspace.dependencies]
foo = { path = "new/path" }
"#;
    let d = diff(old, new);
    assert!(!d.force_all);
}

// ── workspace.dependencies — feature/optional/rename force-all ──────

#[test]
fn workspace_dep_features_change_forces_all() {
    // anyhow's features change but version stays — lockfile may not budge.
    let new = VIRTUAL_WORKSPACE.replace(
        "anyhow = { version = \"1.0\", features = [\"std\"] }",
        "anyhow = { version = \"1.0\", features = [\"std\", \"backtrace\"] }",
    );
    let d = diff(VIRTUAL_WORKSPACE, &new);
    assert!(
        d.force_all,
        "features change on a workspace dep must force_all (lockfile may miss it)"
    );
}

#[test]
fn workspace_dep_default_features_flip_forces_all() {
    let new = VIRTUAL_WORKSPACE.replace(
        "anyhow = { version = \"1.0\", features = [\"std\"] }",
        "anyhow = { version = \"1.0\", features = [\"std\"], default-features = false }",
    );
    let d = diff(VIRTUAL_WORKSPACE, &new);
    assert!(d.force_all);
}

#[test]
fn workspace_dep_optional_flip_forces_all() {
    let new = VIRTUAL_WORKSPACE.replace(
        "anyhow = { version = \"1.0\", features = [\"std\"] }",
        "anyhow = { version = \"1.0\", features = [\"std\"], optional = true }",
    );
    let d = diff(VIRTUAL_WORKSPACE, &new);
    assert!(d.force_all);
}

#[test]
fn workspace_dep_package_rename_forces_all() {
    let new = VIRTUAL_WORKSPACE.replace(
        "anyhow = { version = \"1.0\", features = [\"std\"] }",
        "anyhow = { version = \"1.0\", features = [\"std\"], package = \"anyhow-fork\" }",
    );
    let d = diff(VIRTUAL_WORKSPACE, &new);
    assert!(d.force_all);
}

// ── Build-affecting profile / resolver / edition ────────────────────

#[test]
fn profile_release_opt_level_forces_all() {
    let new = VIRTUAL_WORKSPACE.replace("opt-level = \"z\"", "opt-level = \"3\"");
    let d = diff(VIRTUAL_WORKSPACE, &new);
    assert!(d.force_all);
}

#[test]
fn new_profile_section_forces_all() {
    let new = format!("{VIRTUAL_WORKSPACE}\n[profile.dev]\ndebug = true\n");
    let d = diff(VIRTUAL_WORKSPACE, &new);
    assert!(d.force_all);
}

#[test]
fn workspace_resolver_change_forces_all() {
    let new = VIRTUAL_WORKSPACE.replace("resolver = \"2\"", "resolver = \"3\"");
    let d = diff(VIRTUAL_WORKSPACE, &new);
    assert!(d.force_all);
}

#[test]
fn workspace_package_edition_forces_all() {
    let new = VIRTUAL_WORKSPACE.replace("edition = \"2021\"", "edition = \"2024\"");
    let d = diff(VIRTUAL_WORKSPACE, &new);
    assert!(d.force_all);
}

#[test]
fn workspace_lints_change_forces_all() {
    let old = r#"[workspace]
members = []
"#;
    let new = r#"[workspace]
members = []
[workspace.lints.rust]
unused = "warn"
"#;
    let d = diff(old, new);
    assert!(d.force_all);
}

#[test]
fn cargo_features_addition_forces_all() {
    let old = r#"[workspace]
members = []
"#;
    let new = r#"cargo-features = ["edition2024"]
[workspace]
members = []
"#;
    let d = diff(old, new);
    assert!(d.force_all);
}

#[test]
fn unknown_top_level_section_forces_all() {
    let old = r#"[workspace]
members = []
"#;
    let new = r#"[workspace]
members = []
[some-future-section]
flag = true
"#;
    let d = diff(old, new);
    assert!(
        d.force_all,
        "unrecognized top-level sections must default to force-all"
    );
}

// ── workspace.target.* ──────────────────────────────────────────────

#[test]
fn workspace_target_dependencies_change_forces_all() {
    let old = r#"[workspace]
members = []
[workspace.target.'cfg(unix)'.dependencies]
foo = "1.0"
"#;
    let new = r#"[workspace]
members = []
[workspace.target.'cfg(unix)'.dependencies]
foo = "1.1"
"#;
    let d = diff(old, new);
    assert!(d.force_all, "workspace.target.* is force-all in v1");
}

// ── Non-virtual workspace root: per-package keys allow-listed ───────

#[test]
fn root_package_dependencies_change_is_no_force_all() {
    let old = r#"[package]
name = "root"
version = "0.1.0"
edition = "2021"

[workspace]
members = []

[dependencies]
serde = "1.0.200"
"#;
    let new = old.replace("serde = \"1.0.200\"", "serde = \"1.0.201\"");
    let d = diff(old, &new);
    assert!(
        !d.force_all,
        "root package dep change is handled by file-based path"
    );
}

#[test]
fn root_package_features_change_is_no_force_all() {
    let old = r#"[package]
name = "root"
version = "0.1.0"
edition = "2021"

[workspace]
members = []

[features]
default = []
"#;
    let new = old.replace("default = []", "default = [\"foo\"]\nfoo = []");
    let d = diff(old, &new);
    assert!(!d.force_all);
}

// ── patch / replace ─────────────────────────────────────────────────

#[test]
fn patch_crates_io_added_is_no_force_all() {
    let old = r#"[workspace]
members = []
"#;
    let new = r#"[workspace]
members = []
[patch.crates-io]
serde = { git = "https://github.com/example/serde.git", branch = "fork" }
"#;
    let d = diff(old, new);
    assert!(!d.force_all, "patch redirects are lockfile-reflected");
}

#[test]
fn patch_features_field_forces_all() {
    let old = r#"[workspace]
members = []
[patch.crates-io]
serde = { git = "https://x.test/repo.git", features = ["a"] }
"#;
    let new = r#"[workspace]
members = []
[patch.crates-io]
serde = { git = "https://x.test/repo.git", features = ["a", "b"] }
"#;
    let d = diff(old, new);
    assert!(
        d.force_all,
        "patch.features change is not lockfile-reflected"
    );
}

// ── Parse failures ──────────────────────────────────────────────────

#[test]
fn malformed_old_manifest_sets_fallback_reason() {
    let d = diff("not toml = {{{", VIRTUAL_WORKSPACE);
    assert!(d.fallback_reason.is_some());
    assert!(!d.force_all);
}

#[test]
fn malformed_new_manifest_sets_fallback_reason() {
    let d = diff(VIRTUAL_WORKSPACE, "not toml = {{{");
    assert!(d.fallback_reason.is_some());
    assert!(!d.force_all);
}
