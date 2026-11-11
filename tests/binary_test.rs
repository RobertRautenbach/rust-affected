use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;

fn binary_path() -> PathBuf {
    // cargo test builds binaries into the same target directory
    let path = PathBuf::from(env!("CARGO_BIN_EXE_rust-affected"));
    // Ensure the binary exists (built by cargo test)
    assert!(path.exists(), "Binary not found at {}", path.display());
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
    cmd.env_remove("GITHUB_STEP_SUMMARY");
    cmd.env_remove("CHANGED_FILES_JSON");
    cmd.env_remove("FORCE_TRIGGERS");
    cmd.env_remove("EXCLUDED_PACKAGES");
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

// ── CHANGED_FILES_JSON ──────────────────────────────────────────────

#[test]
fn env_changed_files_json_array() {
    let (stdout, ok) = run_binary(&[(
        "CHANGED_FILES_JSON",
        r#"["lib-utils/src/lib.rs","lib-standalone/src/lib.rs"]"#,
    )]);
    assert!(ok);
    let json = parse_json(&stdout);
    let changed: Vec<String> = serde_json::from_value(json["changed_packages"].clone()).unwrap();
    assert_eq!(changed, vec!["lib-standalone", "lib-utils"]);
}

#[test]
fn env_changed_files_empty_array() {
    let (stdout, ok) = run_binary(&[("CHANGED_FILES_JSON", "[]")]);
    assert!(ok);
    let json = parse_json(&stdout);
    assert_eq!(json["changed_packages"], Value::Array(vec![]));
    assert_eq!(json["affected_libraries"], Value::Array(vec![]));
    assert_eq!(json["affected_binaries"], Value::Array(vec![]));
    assert_eq!(json["force_all"], false);
}

// ── FORCE_TRIGGERS parsing ──────────────────────────────────────────

#[test]
fn env_force_triggers_space_separated() {
    let (stdout, ok) = run_binary(&[
        ("CHANGED_FILES_JSON", r#"["infra/deploy.yml"]"#),
        ("FORCE_TRIGGERS", "Cargo.lock infra/"),
    ]);
    assert!(ok);
    let json = parse_json(&stdout);
    assert_eq!(json["force_all"], true);
}

#[test]
fn env_force_triggers_newline_separated() {
    let (stdout, ok) = run_binary(&[
        ("CHANGED_FILES_JSON", r#"["infra/deploy.yml"]"#),
        ("FORCE_TRIGGERS", "Cargo.lock\ninfra/"),
    ]);
    assert!(ok);
    let json = parse_json(&stdout);
    assert_eq!(json["force_all"], true);
}

#[test]
fn env_force_triggers_unset_means_no_force() {
    let (stdout, ok) = run_binary(&[("CHANGED_FILES_JSON", r#"["lib-core/src/lib.rs"]"#)]);
    assert!(ok);
    let json = parse_json(&stdout);
    assert_eq!(json["force_all"], false);
}

#[test]
fn env_force_triggers_trailing_slash_matches_directory() {
    // action.yml: "A trailing slash (e.g. "infra/") matches the directory
    // and everything inside it."
    let (stdout, ok) = run_binary(&[
        ("CHANGED_FILES_JSON", r#"["infra/nested/deep/file.yml"]"#),
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
        ("CHANGED_FILES_JSON", r#"["Cargo.lock"]"#),
        ("FORCE_TRIGGERS", "Cargo.lock"),
    ]);
    assert!(ok);
    let json = parse_json(&stdout);
    assert_eq!(json["force_all"], true);
}

#[test]
fn env_force_triggers_bare_name_no_match_on_different_file() {
    let (stdout, ok) = run_binary(&[
        ("CHANGED_FILES_JSON", r#"["lib-core/src/lib.rs"]"#),
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
        ("CHANGED_FILES_JSON", r#"[".github/workflows/ci.yml"]"#),
        ("FORCE_TRIGGERS", ".github/**"),
    ]);
    assert!(ok);
    let json = parse_json(&stdout);
    assert_eq!(json["force_all"], true);
}

// ── EXCLUDED_PACKAGES parsing ────────────────────────────────────────

#[test]
fn env_excluded_packages_space_separated() {
    let (stdout, ok) = run_binary(&[
        ("CHANGED_FILES_JSON", r#"["lib-utils/src/lib.rs"]"#),
        ("EXCLUDED_PACKAGES", "lib-core app-alpha"),
    ]);
    assert!(ok);
    let json = parse_json(&stdout);
    let affected: Vec<String> = serde_json::from_value(json["affected_libraries"].clone()).unwrap();
    assert!(!affected.contains(&"lib-core".to_string()));
    assert!(!affected.contains(&"app-alpha".to_string()));
    assert!(affected.contains(&"lib-utils".to_string()));
    // app-beta is a binary crate — it must not appear in affected_libraries
    assert!(!affected.contains(&"app-beta".to_string()));
}

#[test]
fn env_excluded_packages_newline_separated() {
    let (stdout, ok) = run_binary(&[
        ("CHANGED_FILES_JSON", r#"["lib-utils/src/lib.rs"]"#),
        ("EXCLUDED_PACKAGES", "lib-core\napp-alpha"),
    ]);
    assert!(ok);
    let json = parse_json(&stdout);
    let affected: Vec<String> = serde_json::from_value(json["affected_libraries"].clone()).unwrap();
    assert!(!affected.contains(&"lib-core".to_string()));
    assert!(!affected.contains(&"app-alpha".to_string()));
}

#[test]
fn env_excluded_packages_unset_means_no_exclusions() {
    let (stdout, ok) = run_binary(&[("CHANGED_FILES_JSON", r#"["lib-utils/src/lib.rs"]"#)]);
    assert!(ok);
    let json = parse_json(&stdout);
    let affected: Vec<String> = serde_json::from_value(json["affected_libraries"].clone()).unwrap();
    assert!(affected.contains(&"lib-core".to_string()));
    // app-alpha is a binary crate — it must not appear in affected_libraries
    assert!(!affected.contains(&"app-alpha".to_string()));
}

// ── GITHUB_OUTPUT file-based output ─────────────────────────────────

#[test]
fn env_github_output_writes_to_file() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path().join("output");
    // Create the file
    std::fs::write(&tmp, "").unwrap();

    let (stdout, ok) = run_binary(&[
        ("CHANGED_FILES_JSON", r#"["app-alpha/src/main.rs"]"#),
        ("GITHUB_OUTPUT", tmp.to_str().unwrap()),
    ]);
    assert!(ok);
    let contents = std::fs::read_to_string(&tmp).unwrap();
    assert_eq!(stdout, contents);
    assert!(contents.contains("changed_packages="));
    assert!(contents.contains("affected_libraries="));
    assert!(contents.contains("affected_binaries="));
    assert!(contents.contains("force_all="));
    // Verify the values are valid JSON arrays
    for line in contents.lines() {
        if let Some(value) = line.strip_prefix("changed_packages=") {
            let parsed: Vec<String> = serde_json::from_str(value).unwrap();
            assert_eq!(parsed, vec!["app-alpha"]);
        }
    }
}

#[test]
fn env_github_output_appends_to_existing_content() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path().join("output");
    std::fs::write(&tmp, "existing_key=existing_value\n").unwrap();

    let (_stdout, ok) = run_binary(&[
        ("CHANGED_FILES_JSON", r#"["app-alpha/src/main.rs"]"#),
        ("GITHUB_OUTPUT", tmp.to_str().unwrap()),
    ]);
    assert!(ok);

    let contents = std::fs::read_to_string(&tmp).unwrap();
    // Existing content is preserved
    assert!(contents.starts_with("existing_key=existing_value\n"));
    // New content appended
    assert!(contents.contains("changed_packages="));
}

// ── JSON stdout output (no GITHUB_OUTPUT) ───────────────────────────

#[test]
fn stdout_json_contains_all_fields() {
    let (stdout, ok) = run_binary(&[("CHANGED_FILES_JSON", r#"["lib-core/src/lib.rs"]"#)]);
    assert!(ok);
    let json = parse_json(&stdout);
    // All four keys present
    assert!(json.get("changed_packages").is_some());
    assert!(json.get("affected_libraries").is_some());
    assert!(json.get("affected_binaries").is_some());
    assert!(json.get("force_all").is_some());
    // Types are correct
    assert!(json["changed_packages"].is_array());
    assert!(json["affected_libraries"].is_array());
    assert!(json["affected_binaries"].is_array());
    assert!(json["force_all"].is_boolean());
}

// ── Combined env var interaction ────────────────────────────────────

#[test]
fn all_env_vars_together() {
    let (stdout, ok) = run_binary(&[
        (
            "CHANGED_FILES_JSON",
            r#"["lib-utils/src/lib.rs","infra/deploy.yml"]"#,
        ),
        ("FORCE_TRIGGERS", "infra/"),
        ("EXCLUDED_PACKAGES", "app-alpha lib-core-ext"),
    ]);
    assert!(ok);
    let json = parse_json(&stdout);

    assert_eq!(json["force_all"], true);

    let changed: Vec<String> = serde_json::from_value(json["changed_packages"].clone()).unwrap();
    assert_eq!(changed, vec!["lib-utils"]);

    let affected: Vec<String> = serde_json::from_value(json["affected_libraries"].clone()).unwrap();
    assert!(!affected.contains(&"app-alpha".to_string()));
    assert!(!affected.contains(&"lib-core-ext".to_string()));
    assert!(affected.contains(&"lib-core".to_string()));
    // app-beta is a binary crate — it must not appear in affected_libraries
    assert!(!affected.contains(&"app-beta".to_string()));

    let binaries: Vec<String> = serde_json::from_value(json["affected_binaries"].clone()).unwrap();
    assert_eq!(binaries, vec!["app-beta", "tool-alpha"]);
}
