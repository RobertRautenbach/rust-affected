use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;

fn binary_path() -> PathBuf {
    // cargo test builds binaries into the same target directory
    let path = PathBuf::from(env!("CARGO_BIN_EXE_rust-affected"));
    // Ensure the binary exists (built by cargo test)
    assert!(path.exists(), "Binary not found at {path:?}");
    path
}

fn fixture_dir() -> PathBuf {
    [env!("CARGO_MANIFEST_DIR"), "tests", "fixtures", "workspace"]
        .iter()
        .collect()
}

fn run_binary(envs: &[(&str, &str)]) -> (String, bool) {
    let mut cmd = Command::new(binary_path());
    cmd.current_dir(fixture_dir());
    // Clear env vars that could interfere
    cmd.env_remove("GITHUB_OUTPUT");
    cmd.env_remove("CHANGED_FILES");
    cmd.env_remove("FORCE_TRIGGERS");
    cmd.env_remove("EXCLUDED_MEMBERS");
    cmd.env_remove("BASE_SHA");
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let output = cmd.output().expect("Failed to run binary");
    let stdout = String::from_utf8(output.stdout).unwrap();
    (stdout, output.status.success())
}

fn parse_json(stdout: &str) -> Value {
    serde_json::from_str(stdout.trim()).expect("Failed to parse JSON output")
}

// ── CHANGED_FILES parsing ───────────────────────────────────────────

#[test]
fn env_changed_files_space_separated() {
    let (stdout, ok) = run_binary(&[(
        "CHANGED_FILES",
        "lib-utils/src/lib.rs lib-standalone/src/lib.rs",
    )]);
    assert!(ok);
    let json = parse_json(&stdout);
    let changed: Vec<String> = serde_json::from_value(json["changed_crates"].clone()).unwrap();
    assert_eq!(changed, vec!["lib-standalone", "lib-utils"]);
}

#[test]
fn env_changed_files_newline_separated() {
    // split_whitespace handles newlines as well as spaces
    let (stdout, ok) = run_binary(&[(
        "CHANGED_FILES",
        "lib-utils/src/lib.rs\nlib-standalone/src/lib.rs",
    )]);
    assert!(ok);
    let json = parse_json(&stdout);
    let changed: Vec<String> = serde_json::from_value(json["changed_crates"].clone()).unwrap();
    assert_eq!(changed, vec!["lib-standalone", "lib-utils"]);
}

#[test]
fn env_changed_files_empty_string() {
    let (stdout, ok) = run_binary(&[("CHANGED_FILES", "")]);
    assert!(ok);
    let json = parse_json(&stdout);
    assert_eq!(json["changed_crates"], Value::Array(vec![]));
    assert_eq!(json["affected_library_members"], Value::Array(vec![]));
    assert_eq!(json["affected_binary_members"], Value::Array(vec![]));
    assert_eq!(json["force_all"], false);
}

#[test]
fn env_changed_files_unset() {
    let (stdout, ok) = run_binary(&[]);
    assert!(ok);
    let json = parse_json(&stdout);
    assert_eq!(json["changed_crates"], Value::Array(vec![]));
    assert_eq!(json["force_all"], false);
}

#[test]
fn env_changed_files_extra_whitespace_ignored() {
    let (stdout, ok) = run_binary(&[(
        "CHANGED_FILES",
        "  lib-core/src/lib.rs   app-alpha/src/main.rs  ",
    )]);
    assert!(ok);
    let json = parse_json(&stdout);
    let changed: Vec<String> = serde_json::from_value(json["changed_crates"].clone()).unwrap();
    assert_eq!(changed, vec!["app-alpha", "lib-core"]);
}

// ── FORCE_TRIGGERS parsing ──────────────────────────────────────────

#[test]
fn env_force_triggers_space_separated() {
    let (stdout, ok) = run_binary(&[
        ("CHANGED_FILES", "infra/deploy.yml"),
        ("FORCE_TRIGGERS", "Cargo.lock infra/"),
    ]);
    assert!(ok);
    let json = parse_json(&stdout);
    assert_eq!(json["force_all"], true);
}

#[test]
fn env_force_triggers_newline_separated() {
    let (stdout, ok) = run_binary(&[
        ("CHANGED_FILES", "infra/deploy.yml"),
        ("FORCE_TRIGGERS", "Cargo.lock\ninfra/"),
    ]);
    assert!(ok);
    let json = parse_json(&stdout);
    assert_eq!(json["force_all"], true);
}

#[test]
fn env_force_triggers_unset_means_no_force() {
    let (stdout, ok) = run_binary(&[("CHANGED_FILES", "lib-core/src/lib.rs")]);
    assert!(ok);
    let json = parse_json(&stdout);
    assert_eq!(json["force_all"], false);
}

#[test]
fn env_force_triggers_trailing_slash_matches_directory() {
    // action.yml: "A trailing slash (e.g. "infra/") matches the directory
    // and everything inside it."
    let (stdout, ok) = run_binary(&[
        ("CHANGED_FILES", "infra/nested/deep/file.yml"),
        ("FORCE_TRIGGERS", "infra/"),
    ]);
    assert!(ok);
    let json = parse_json(&stdout);
    assert_eq!(json["force_all"], true);
}

#[test]
fn env_force_triggers_bare_name_matches_exact_path() {
    // action.yml: 'A bare name (e.g. "Cargo.lock") matches that exact path only.'
    let (stdout, ok) = run_binary(&[
        ("CHANGED_FILES", "Cargo.lock"),
        ("FORCE_TRIGGERS", "Cargo.lock"),
    ]);
    assert!(ok);
    let json = parse_json(&stdout);
    assert_eq!(json["force_all"], true);
}

#[test]
fn env_force_triggers_bare_name_no_match_on_different_file() {
    let (stdout, ok) = run_binary(&[
        ("CHANGED_FILES", "lib-core/src/lib.rs"),
        ("FORCE_TRIGGERS", "Cargo.lock"),
    ]);
    assert!(ok);
    let json = parse_json(&stdout);
    assert_eq!(json["force_all"], false);
}

#[test]
fn env_force_triggers_glob_pattern() {
    // action.yml: 'Full glob patterns are also supported (e.g. "**/*.sql", ".github/**").'
    let (stdout, ok) = run_binary(&[
        ("CHANGED_FILES", ".github/workflows/ci.yml"),
        ("FORCE_TRIGGERS", ".github/**"),
    ]);
    assert!(ok);
    let json = parse_json(&stdout);
    assert_eq!(json["force_all"], true);
}

// ── EXCLUDED_MEMBERS parsing ────────────────────────────────────────

#[test]
fn env_excluded_members_space_separated() {
    let (stdout, ok) = run_binary(&[
        ("CHANGED_FILES", "lib-utils/src/lib.rs"),
        ("EXCLUDED_MEMBERS", "lib-core app-alpha"),
    ]);
    assert!(ok);
    let json = parse_json(&stdout);
    let affected: Vec<String> =
        serde_json::from_value(json["affected_library_members"].clone()).unwrap();
    assert!(!affected.contains(&"lib-core".to_string()));
    assert!(!affected.contains(&"app-alpha".to_string()));
    assert!(affected.contains(&"lib-utils".to_string()));
    // app-beta is a binary crate — it must not appear in affected_library_members
    assert!(!affected.contains(&"app-beta".to_string()));
}

#[test]
fn env_excluded_members_newline_separated() {
    let (stdout, ok) = run_binary(&[
        ("CHANGED_FILES", "lib-utils/src/lib.rs"),
        ("EXCLUDED_MEMBERS", "lib-core\napp-alpha"),
    ]);
    assert!(ok);
    let json = parse_json(&stdout);
    let affected: Vec<String> =
        serde_json::from_value(json["affected_library_members"].clone()).unwrap();
    assert!(!affected.contains(&"lib-core".to_string()));
    assert!(!affected.contains(&"app-alpha".to_string()));
}

#[test]
fn env_excluded_members_unset_means_no_exclusions() {
    let (stdout, ok) = run_binary(&[("CHANGED_FILES", "lib-utils/src/lib.rs")]);
    assert!(ok);
    let json = parse_json(&stdout);
    let affected: Vec<String> =
        serde_json::from_value(json["affected_library_members"].clone()).unwrap();
    assert!(affected.contains(&"lib-core".to_string()));
    // app-alpha is a binary crate — it must not appear in affected_library_members
    assert!(!affected.contains(&"app-alpha".to_string()));
}

// ── GITHUB_OUTPUT file-based output ─────────────────────────────────

#[test]
fn env_github_output_writes_to_file() {
    let tmp = std::env::temp_dir().join(format!("test-github-output-{}", std::process::id()));
    // Create the file
    std::fs::write(&tmp, "").unwrap();

    let (stdout, ok) = run_binary(&[
        ("CHANGED_FILES", "app-alpha/src/main.rs"),
        ("GITHUB_OUTPUT", tmp.to_str().unwrap()),
    ]);
    assert!(ok);
    // stdout should be empty when GITHUB_OUTPUT is set
    assert!(
        stdout.trim().is_empty(),
        "stdout should be empty when GITHUB_OUTPUT is set, got: {stdout}"
    );

    let contents = std::fs::read_to_string(&tmp).unwrap();
    assert!(contents.contains("changed_crates="));
    assert!(contents.contains("affected_library_members="));
    assert!(contents.contains("affected_binary_members="));
    assert!(contents.contains("force_all="));
    // Verify the values are valid JSON arrays
    for line in contents.lines() {
        if let Some(value) = line.strip_prefix("changed_crates=") {
            let parsed: Vec<String> = serde_json::from_str(value).unwrap();
            assert_eq!(parsed, vec!["app-alpha"]);
        }
    }

    std::fs::remove_file(&tmp).ok();
}

#[test]
fn env_github_output_appends_to_existing_content() {
    let tmp =
        std::env::temp_dir().join(format!("test-github-output-append-{}", std::process::id()));
    std::fs::write(&tmp, "existing_key=existing_value\n").unwrap();

    let (_stdout, ok) = run_binary(&[
        ("CHANGED_FILES", "app-alpha/src/main.rs"),
        ("GITHUB_OUTPUT", tmp.to_str().unwrap()),
    ]);
    assert!(ok);

    let contents = std::fs::read_to_string(&tmp).unwrap();
    // Existing content is preserved
    assert!(contents.starts_with("existing_key=existing_value\n"));
    // New content appended
    assert!(contents.contains("changed_crates="));

    std::fs::remove_file(&tmp).ok();
}

// ── JSON stdout output (no GITHUB_OUTPUT) ───────────────────────────

#[test]
fn stdout_json_contains_all_fields() {
    let (stdout, ok) = run_binary(&[("CHANGED_FILES", "lib-core/src/lib.rs")]);
    assert!(ok);
    let json = parse_json(&stdout);
    // All four keys present
    assert!(json.get("changed_crates").is_some());
    assert!(json.get("affected_library_members").is_some());
    assert!(json.get("affected_binary_members").is_some());
    assert!(json.get("force_all").is_some());
    // Types are correct
    assert!(json["changed_crates"].is_array());
    assert!(json["affected_library_members"].is_array());
    assert!(json["affected_binary_members"].is_array());
    assert!(json["force_all"].is_boolean());
}

// ── Combined env var interaction ────────────────────────────────────

#[test]
fn all_env_vars_together() {
    let (stdout, ok) = run_binary(&[
        ("CHANGED_FILES", "lib-utils/src/lib.rs infra/deploy.yml"),
        ("FORCE_TRIGGERS", "infra/"),
        ("EXCLUDED_MEMBERS", "app-alpha lib-core-ext"),
    ]);
    assert!(ok);
    let json = parse_json(&stdout);

    assert_eq!(json["force_all"], true);

    let changed: Vec<String> = serde_json::from_value(json["changed_crates"].clone()).unwrap();
    assert_eq!(changed, vec!["lib-utils"]);

    let affected: Vec<String> =
        serde_json::from_value(json["affected_library_members"].clone()).unwrap();
    assert!(!affected.contains(&"app-alpha".to_string()));
    assert!(!affected.contains(&"lib-core-ext".to_string()));
    assert!(affected.contains(&"lib-core".to_string()));
    // app-beta is a binary crate — it must not appear in affected_library_members
    assert!(!affected.contains(&"app-beta".to_string()));

    let binaries: Vec<String> =
        serde_json::from_value(json["affected_binary_members"].clone()).unwrap();
    assert_eq!(binaries, vec!["app-beta", "tool-alpha"]);
}

// ── BASE_SHA-driven Cargo.lock diff ─────────────────────────────────
//
// These tests build a throwaway git repo containing a copy of the fixture
// workspace, commit two versions of `Cargo.lock`, then run the binary with
// `BASE_SHA` pointing at the first commit and `CHANGED_FILES="Cargo.lock"`.
// They cover: precise per-member diff, opt-out via FORCE_TRIGGERS, and
// the safety fallback to `force_all=true` when BASE_SHA is missing/bogus.

struct TempRepo {
    dir: PathBuf,
}

impl TempRepo {
    fn new(test_name: &str) -> Self {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "rust-affected-test-{}-{}",
            test_name,
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        // Copy the fixture workspace into the temp dir.
        let status = Command::new("cp")
            .arg("-R")
            .arg(format!("{}/.", fixture_dir().display()))
            .arg(&dir)
            .status()
            .expect("cp -R");
        assert!(status.success(), "failed to copy fixture into {dir:?}");
        Self { dir }
    }

    fn git(&self, args: &[&str]) -> std::process::Output {
        let out = Command::new("git")
            .current_dir(&self.dir)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@example.test")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@example.test")
            .args(args)
            .output()
            .expect("git invocation failed");
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    fn init_and_commit(&self, msg: &str) -> String {
        self.git(&["init", "--quiet", "-b", "main"]);
        self.git(&["add", "."]);
        self.git(&["commit", "--quiet", "-m", msg]);
        let out = self.git(&["rev-parse", "HEAD"]);
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    fn commit_all(&self, msg: &str) -> String {
        self.git(&["add", "."]);
        self.git(&["commit", "--quiet", "-m", msg]);
        let out = self.git(&["rev-parse", "HEAD"]);
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    fn write_lockfile(&self, contents: &str) {
        std::fs::write(self.dir.join("Cargo.lock"), contents).expect("write Cargo.lock");
    }

    fn run(&self, envs: &[(&str, &str)]) -> (String, bool) {
        let mut cmd = Command::new(binary_path());
        cmd.current_dir(&self.dir);
        cmd.env_remove("GITHUB_OUTPUT");
        cmd.env_remove("CHANGED_FILES");
        cmd.env_remove("FORCE_TRIGGERS");
        cmd.env_remove("EXCLUDED_MEMBERS");
        cmd.env_remove("BASE_SHA");
        for (k, v) in envs {
            cmd.env(k, v);
        }
        let out = cmd.output().expect("run binary");
        (String::from_utf8(out.stdout).unwrap(), out.status.success())
    }
}

impl Drop for TempRepo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

// Adds an isolated `anyhow` registry dep to a target workspace member in the
// fixture lockfile. Returns the modified lockfile content. Only the chain
// reaching `target` (via the fixture's static dep graph) should be flagged.
fn lockfile_with_dep_added_to(target: &str) -> String {
    let original =
        std::fs::read_to_string(fixture_dir().join("Cargo.lock")).expect("read fixture lockfile");
    let needle = format!(
        "[[package]]\nname = \"{target}\"\nversion = \"0.1.0\"\n"
    );
    let replacement = format!(
        "[[package]]\nname = \"{target}\"\nversion = \"0.1.0\"\ndependencies = [\n \"anyhow\",\n]\n\n\
         [[package]]\nname = \"anyhow\"\nversion = \"1.0.80\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"0000000000000000000000000000000000000000000000000000000000000003\"\n"
    );
    assert!(
        original.contains(&needle),
        "fixture lockfile shape changed; needle not found:\n{needle}"
    );
    original.replace(&needle, &replacement)
}

#[test]
fn lockfile_diff_only_affects_dependent_chain() {
    let repo = TempRepo::new("lockfile-diff-chain");
    let base = repo.init_and_commit("initial");
    // Modify Cargo.lock: lib-utils gains a new external dep. lib-standalone
    // and lib-with-tests should remain unaffected.
    repo.write_lockfile(&lockfile_with_dep_added_to("lib-utils"));
    repo.commit_all("bump lib-utils transitive");

    let (stdout, ok) = repo.run(&[("CHANGED_FILES", "Cargo.lock"), ("BASE_SHA", &base)]);
    assert!(ok, "binary failed; stdout={stdout}");
    let json = parse_json(&stdout);

    assert_eq!(json["force_all"], false);
    // Cargo.lock isn't inside any crate dir, so changed_crates stays empty.
    let changed: Vec<String> = serde_json::from_value(json["changed_crates"].clone()).unwrap();
    assert!(changed.is_empty());

    let libs: Vec<String> =
        serde_json::from_value(json["affected_library_members"].clone()).unwrap();
    // Pure libraries reaching lib-utils: lib-utils, lib-core, lib-core-ext.
    assert_eq!(libs, vec!["lib-core", "lib-core-ext", "lib-utils"]);

    let bins: Vec<String> =
        serde_json::from_value(json["affected_binary_members"].clone()).unwrap();
    assert_eq!(bins, vec!["app-alpha", "app-beta", "tool-alpha"]);
}

#[test]
fn lockfile_diff_isolated_change_does_not_pull_unrelated_members() {
    let repo = TempRepo::new("lockfile-diff-isolated");
    let base = repo.init_and_commit("initial");
    // Only lib-standalone gains a new external dep. Its sole dependent is
    // app-beta. Nothing else should be touched.
    repo.write_lockfile(&lockfile_with_dep_added_to("lib-standalone"));
    repo.commit_all("bump lib-standalone transitive");

    let (stdout, ok) = repo.run(&[("CHANGED_FILES", "Cargo.lock"), ("BASE_SHA", &base)]);
    assert!(ok);
    let json = parse_json(&stdout);

    assert_eq!(json["force_all"], false);
    let libs: Vec<String> =
        serde_json::from_value(json["affected_library_members"].clone()).unwrap();
    assert_eq!(libs, vec!["lib-standalone"]);
    let bins: Vec<String> =
        serde_json::from_value(json["affected_binary_members"].clone()).unwrap();
    assert_eq!(bins, vec!["app-beta"]);
}

#[test]
fn lockfile_in_force_triggers_keeps_force_all_behavior() {
    let repo = TempRepo::new("lockfile-force-trigger");
    let _base = repo.init_and_commit("initial");
    repo.write_lockfile(&lockfile_with_dep_added_to("lib-utils"));
    repo.commit_all("bump");

    // User opts out by listing Cargo.lock in force_triggers.
    let (stdout, ok) = repo.run(&[
        ("CHANGED_FILES", "Cargo.lock"),
        ("FORCE_TRIGGERS", "Cargo.lock"),
        // BASE_SHA deliberately omitted to prove force_triggers short-circuits
        // before we attempt the git fetch.
    ]);
    assert!(ok);
    let json = parse_json(&stdout);
    assert_eq!(json["force_all"], true);
}

#[test]
fn lockfile_change_with_no_base_sha_falls_back_to_force_all() {
    let repo = TempRepo::new("lockfile-no-base");
    let _base = repo.init_and_commit("initial");
    repo.write_lockfile(&lockfile_with_dep_added_to("lib-utils"));
    repo.commit_all("bump");

    // No BASE_SHA → cannot fetch old lockfile → safe fallback to force_all.
    let (stdout, ok) = repo.run(&[("CHANGED_FILES", "Cargo.lock")]);
    assert!(ok);
    let json = parse_json(&stdout);
    assert_eq!(json["force_all"], true);
}

#[test]
fn lockfile_change_with_bogus_base_sha_falls_back_to_force_all() {
    let repo = TempRepo::new("lockfile-bogus-base");
    let _base = repo.init_and_commit("initial");
    repo.write_lockfile(&lockfile_with_dep_added_to("lib-utils"));
    repo.commit_all("bump");

    // Bogus SHA → git show fails → safe fallback.
    let (stdout, ok) = repo.run(&[
        ("CHANGED_FILES", "Cargo.lock"),
        ("BASE_SHA", "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef"),
    ]);
    assert!(ok);
    let json = parse_json(&stdout);
    assert_eq!(json["force_all"], true);
}

#[test]
fn lockfile_unchanged_between_commits_yields_no_affected_members() {
    let repo = TempRepo::new("lockfile-unchanged");
    let base = repo.init_and_commit("initial");
    // Second commit touches a non-lockfile file. Cargo.lock is unchanged
    // between base and HEAD, so the new path shouldn't even be invoked
    // (CHANGED_FILES doesn't include Cargo.lock).
    std::fs::write(repo.dir.join("README.md"), "hello\n").unwrap();
    repo.commit_all("touch readme");

    let (stdout, ok) = repo.run(&[("CHANGED_FILES", "README.md"), ("BASE_SHA", &base)]);
    assert!(ok);
    let json = parse_json(&stdout);
    assert_eq!(json["force_all"], false);
    let libs: Vec<String> =
        serde_json::from_value(json["affected_library_members"].clone()).unwrap();
    assert!(libs.is_empty());
}

#[test]
fn lockfile_diff_unions_with_file_based_changes() {
    let repo = TempRepo::new("lockfile-union");
    let base = repo.init_and_commit("initial");
    // Lockfile change touches the lib-standalone chain (→ app-beta only),
    // while a file change touches app-alpha directly.
    repo.write_lockfile(&lockfile_with_dep_added_to("lib-standalone"));
    std::fs::write(
        repo.dir.join("app-alpha").join("src").join("main.rs"),
        "fn main() { println!(\"hi\"); }\n",
    )
    .unwrap();
    repo.commit_all("two-change commit");

    let (stdout, ok) = repo.run(&[
        ("CHANGED_FILES", "Cargo.lock app-alpha/src/main.rs"),
        ("BASE_SHA", &base),
    ]);
    assert!(ok);
    let json = parse_json(&stdout);
    assert_eq!(json["force_all"], false);

    // changed_crates only reflects FILE-level changes, not lockfile-derived ones.
    let changed: Vec<String> = serde_json::from_value(json["changed_crates"].clone()).unwrap();
    assert_eq!(changed, vec!["app-alpha"]);

    let libs: Vec<String> =
        serde_json::from_value(json["affected_library_members"].clone()).unwrap();
    assert_eq!(libs, vec!["lib-standalone"]);

    let bins: Vec<String> =
        serde_json::from_value(json["affected_binary_members"].clone()).unwrap();
    // app-alpha (file change) + app-beta (lockfile change via lib-standalone).
    assert_eq!(bins, vec!["app-alpha", "app-beta"]);
}

// ── BASE_SHA-driven Cargo.toml diff ─────────────────────────────────
//
// Mirror of the Cargo.lock-aware integration tests for the root-manifest
// diff. Covers: precise member-add, build-affecting profile change, opt-out
// via FORCE_TRIGGERS, and the safety fallback when BASE_SHA is bogus.

fn add_workspace_member_to_manifest(toml: &str, name: &str) -> String {
    // The fixture lists members in a multi-line array; splice the new name
    // into the last entry's slot.
    toml.replace(
        "\"tools/tool-alpha\",\n]",
        &format!("\"tools/tool-alpha\",\n    \"{name}\",\n]"),
    )
}

#[test]
fn manifest_member_add_is_no_force_all() {
    let repo = TempRepo::new("manifest-member-add");
    let base = repo.init_and_commit("initial");

    // Add a new member crate on disk.
    std::fs::create_dir_all(repo.dir.join("lib-new").join("src")).unwrap();
    std::fs::write(
        repo.dir.join("lib-new").join("Cargo.toml"),
        "[package]\nname = \"lib-new\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        repo.dir.join("lib-new").join("src").join("lib.rs"),
        "// nothing yet\n",
    )
    .unwrap();

    // Add lib-new to the workspace members list.
    let manifest = std::fs::read_to_string(repo.dir.join("Cargo.toml")).unwrap();
    std::fs::write(
        repo.dir.join("Cargo.toml"),
        add_workspace_member_to_manifest(&manifest, "lib-new"),
    )
    .unwrap();

    repo.commit_all("add lib-new workspace member");

    let (stdout, ok) = repo.run(&[
        (
            "CHANGED_FILES",
            "Cargo.toml Cargo.lock lib-new/Cargo.toml lib-new/src/lib.rs",
        ),
        ("BASE_SHA", &base),
    ]);
    assert!(ok, "binary failed; stdout={stdout}");
    let json = parse_json(&stdout);

    assert_eq!(
        json["force_all"], false,
        "adding a workspace member must not force-all"
    );
    let changed: Vec<String> = serde_json::from_value(json["changed_crates"].clone()).unwrap();
    assert_eq!(changed, vec!["lib-new"]);
    let libs: Vec<String> =
        serde_json::from_value(json["affected_library_members"].clone()).unwrap();
    assert_eq!(libs, vec!["lib-new"]);
    let bins: Vec<String> =
        serde_json::from_value(json["affected_binary_members"].clone()).unwrap();
    assert!(bins.is_empty());
}

#[test]
fn manifest_profile_change_forces_all() {
    let repo = TempRepo::new("manifest-profile-change");
    let base = repo.init_and_commit("initial");

    let original = std::fs::read_to_string(repo.dir.join("Cargo.toml")).unwrap();
    let modified = format!("{original}\n[profile.release]\nopt-level = 3\n");
    std::fs::write(repo.dir.join("Cargo.toml"), modified).unwrap();
    repo.commit_all("tweak release profile");

    let (stdout, ok) = repo.run(&[("CHANGED_FILES", "Cargo.toml"), ("BASE_SHA", &base)]);
    assert!(ok);
    let json = parse_json(&stdout);
    assert_eq!(json["force_all"], true);
}

#[test]
fn manifest_in_force_triggers_keeps_force_all_behavior() {
    let repo = TempRepo::new("manifest-force-trigger");
    let _base = repo.init_and_commit("initial");

    let original = std::fs::read_to_string(repo.dir.join("Cargo.toml")).unwrap();
    std::fs::write(
        repo.dir.join("Cargo.toml"),
        add_workspace_member_to_manifest(&original, "lib-new"),
    )
    .unwrap();
    // No need to actually create lib-new; force_triggers short-circuits
    // before cargo metadata is invoked... actually cargo metadata IS
    // invoked. Create stub files so metadata succeeds.
    std::fs::create_dir_all(repo.dir.join("lib-new").join("src")).unwrap();
    std::fs::write(
        repo.dir.join("lib-new").join("Cargo.toml"),
        "[package]\nname = \"lib-new\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        repo.dir.join("lib-new").join("src").join("lib.rs"),
        "",
    )
    .unwrap();
    repo.commit_all("add lib-new");

    let (stdout, ok) = repo.run(&[
        ("CHANGED_FILES", "Cargo.toml"),
        ("FORCE_TRIGGERS", "Cargo.toml"),
        // BASE_SHA deliberately omitted to prove force_triggers short-circuits
        // before we attempt the git fetch.
    ]);
    assert!(ok, "stdout={stdout}");
    let json = parse_json(&stdout);
    assert_eq!(json["force_all"], true);
}

#[test]
fn manifest_change_with_no_base_sha_falls_back_to_force_all() {
    let repo = TempRepo::new("manifest-no-base");
    let _base = repo.init_and_commit("initial");

    let original = std::fs::read_to_string(repo.dir.join("Cargo.toml")).unwrap();
    let modified = format!("{original}\n[profile.release]\nopt-level = 3\n");
    std::fs::write(repo.dir.join("Cargo.toml"), modified).unwrap();
    repo.commit_all("profile change");

    let (stdout, ok) = repo.run(&[("CHANGED_FILES", "Cargo.toml")]);
    assert!(ok);
    let json = parse_json(&stdout);
    assert_eq!(json["force_all"], true);
}

#[test]
fn manifest_change_with_bogus_base_sha_falls_back_to_force_all() {
    let repo = TempRepo::new("manifest-bogus-base");
    let _base = repo.init_and_commit("initial");

    let original = std::fs::read_to_string(repo.dir.join("Cargo.toml")).unwrap();
    let modified = format!("{original}\n[profile.release]\nopt-level = 3\n");
    std::fs::write(repo.dir.join("Cargo.toml"), modified).unwrap();
    repo.commit_all("profile change");

    let (stdout, ok) = repo.run(&[
        ("CHANGED_FILES", "Cargo.toml"),
        ("BASE_SHA", "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef"),
    ]);
    assert!(ok);
    let json = parse_json(&stdout);
    assert_eq!(json["force_all"], true);
}
