use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "rust-affected-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn workspace() -> Self {
        let scratch = Self::new();
        std::fs::create_dir_all(scratch.0.join("member/src")).unwrap();
        std::fs::write(
            scratch.0.join("Cargo.toml"),
            "[workspace]\nmembers = [\"member\"]\nresolver = \"2\"\n",
        )
        .unwrap();
        std::fs::write(
            scratch.0.join("member/Cargo.toml"),
            "[package]\nname = \"member\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::write(scratch.0.join("member/src/lib.rs"), "").unwrap();
        scratch
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/workspace")
}

fn command(cwd: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rust-affected"));
    cmd.current_dir(cwd);
    for key in [
        "GITHUB_OUTPUT",
        "GITHUB_STEP_SUMMARY",
        "CHANGED_FILES",
        "FORCE_TRIGGERS",
        "EXCLUDED_MEMBERS",
    ] {
        cmd.env_remove(key);
    }
    cmd.env("CARGO", env!("CARGO"));
    cmd.env("CARGO_NET_OFFLINE", "true");
    cmd
}

fn run(cwd: &Path, envs: &[(&str, &str)]) -> Output {
    command(cwd).envs(envs.iter().copied()).output().unwrap()
}

fn assert_error(output: &Output, message: &str) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert_eq!(output.stdout, Vec::<u8>::new());
    assert_eq!(
        stderr
            .lines()
            .filter(|line| line.starts_with("::error::"))
            .count(),
        1,
        "{stderr}"
    );
    assert!(stderr.contains(message), "{stderr}");
    assert!(!stderr.contains("panicked"), "{stderr}");
}

#[test]
fn missing_workspace_fails_even_without_changes() {
    let scratch = Scratch::new();
    for changed in ["x", ""] {
        assert_error(
            &run(&scratch.0, &[("CHANGED_FILES", changed)]),
            "could not find",
        );
    }
}

#[test]
fn invalid_glob_fails_even_without_changes() {
    for changed in ["lib-core/src/lib.rs", ""] {
        assert_error(
            &run(
                &fixture(),
                &[("CHANGED_FILES", changed), ("FORCE_TRIGGERS", "[abc")],
            ),
            "invalid force_triggers pattern",
        );
    }
}

#[test]
fn warnings_are_grouped_and_do_not_pollute_json() {
    for (envs, messages) in [
        (vec![], vec!["changed_files is empty"]),
        (
            vec![
                ("CHANGED_FILES", "../x ../x /outside/file"),
                ("FORCE_TRIGGERS", "**"),
                ("EXCLUDED_MEMBERS", "tools unknown"),
            ],
            vec!["changed file", "excluded_members"],
        ),
        (
            vec![("EXCLUDED_MEMBERS", "tools")],
            vec!["changed_files is empty", "excluded_members"],
        ),
    ] {
        let output = run(&fixture(), &envs);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{stderr}");
        assert_eq!(
            stderr
                .lines()
                .filter(|line| line.starts_with("::warning::"))
                .count(),
            messages.len(),
            "{stderr}"
        );
        for message in messages {
            assert!(stderr.contains(message), "{stderr}");
        }
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(json["force_all"], false);
        assert_eq!(json["changed_crates"], serde_json::json!([]));
    }
}

#[cfg(unix)]
#[test]
fn every_input_rejects_invalid_utf8_even_without_changes() {
    use std::os::unix::ffi::OsStrExt;
    for key in ["CHANGED_FILES", "FORCE_TRIGGERS", "EXCLUDED_MEMBERS"] {
        for changed in ["", "lib-core/src/lib.rs"] {
            let output = command(&fixture())
                .env("CHANGED_FILES", changed)
                .env(key, OsStr::from_bytes(b"\xff"))
                .output()
                .unwrap();
            assert_error(&output, &format!("{key} is not valid UTF-8"));
        }
    }
}

#[test]
fn output_destination_failures_are_errors() {
    let scratch = Scratch::new();
    for key in ["GITHUB_OUTPUT", "GITHUB_STEP_SUMMARY"] {
        let output = command(&fixture())
            .env("CHANGED_FILES", "lib-core/src/lib.rs")
            .env(key, &scratch.0)
            .output()
            .unwrap();
        assert_error(&output, key);
    }
}

#[test]
fn metadata_preserves_existing_compiler_cache() {
    let scratch = Scratch::workspace();
    std::fs::create_dir(scratch.0.join("target")).unwrap();
    let cache = scratch.0.join("target/.rustc_info.json");
    let original = b"{\"rustc_fingerprint\":0,\"outputs\":{},\"successes\":{}}";
    std::fs::write(&cache, original).unwrap();
    let output = command(&scratch.0)
        .env("CHANGED_FILES", "member/src/lib.rs")
        .env("CARGO_TARGET_DIR", scratch.0.join("target"))
        .env("CARGO_CACHE_RUSTC_INFO", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(std::fs::read(cache).unwrap(), original);
}

#[test]
fn stale_locks_fail_without_rewriting_from_root_or_member() {
    for directory in ["", "member"] {
        for changed in ["", "member/src/lib.rs"] {
            let scratch = Scratch::workspace();
            let lock = scratch.0.join("Cargo.lock");
            std::fs::write(&lock, "version = 4\n").unwrap();
            let output = run(&scratch.0.join(directory), &[("CHANGED_FILES", changed)]);
            assert_error(&output, "lock file");
            assert_eq!(std::fs::read_to_string(lock).unwrap(), "version = 4\n");
        }
    }
}

#[test]
fn missing_locks_warn_and_are_created_even_without_changes() {
    for directory in ["", "member"] {
        for changed in ["", "member/src/lib.rs"] {
            let scratch = Scratch::workspace();
            let output = run(&scratch.0.join(directory), &[("CHANGED_FILES", changed)]);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(output.status.success(), "{stderr}");
            assert!(
                stderr.contains("::warning::No Cargo.lock in the workspace root"),
                "{stderr}"
            );
            assert!(scratch.0.join("Cargo.lock").is_file());
            assert!(!scratch.0.join("member/Cargo.lock").exists());
        }
    }
}

#[test]
fn valid_locks_are_preserved_without_a_missing_lock_warning() {
    let scratch = Scratch::workspace();
    let lock = scratch.0.join("Cargo.lock");
    std::fs::write(
        &lock,
        "version = 4\n\n[[package]]\nname = \"member\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let original = std::fs::read(&lock).unwrap();
    for directory in ["", "member"] {
        let output = run(
            &scratch.0.join(directory),
            &[("CHANGED_FILES", "member/src/lib.rs")],
        );
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!String::from_utf8_lossy(&output.stderr).contains("::warning::"));
        assert_eq!(std::fs::read(&lock).unwrap(), original);
    }
}

#[test]
fn cargo_errors_are_forwarded_without_color_and_outputs_are_untouched() {
    let scratch = Scratch::new();
    let output_path = scratch.0.join("output");
    let summary_path = scratch.0.join("summary");
    std::fs::write(&output_path, "sentinel\n").unwrap();
    std::fs::write(&summary_path, "sentinel\n").unwrap();
    let output = command(&scratch.0)
        .env("CHANGED_FILES", "x")
        .env("CARGO_TERM_COLOR", "always")
        .env("RUSTUP_TERM_COLOR", "always")
        .env("GITHUB_OUTPUT", &output_path)
        .env("GITHUB_STEP_SUMMARY", &summary_path)
        .output()
        .unwrap();
    assert_error(&output, "could not find");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr
            .lines()
            .any(|line| line.starts_with("error: could not find")),
        "{stderr}"
    );
    assert!(!output.stderr.contains(&0x1b));
    assert_eq!(std::fs::read_to_string(output_path).unwrap(), "sentinel\n");
    assert_eq!(std::fs::read_to_string(summary_path).unwrap(), "sentinel\n");
}

#[test]
fn missing_cargo_executable_is_a_handled_error() {
    let scratch = Scratch::new();
    let output = command(&fixture())
        .env("CHANGED_FILES", "x")
        .env("CARGO", scratch.0.join("missing-cargo"))
        .output()
        .unwrap();
    assert_error(&output, "failed to start cargo locate-project");
}

#[cfg(unix)]
#[test]
fn lock_inspection_errors_do_not_enable_unlocked_resolution() {
    let scratch = Scratch::workspace();
    std::os::unix::fs::symlink("Cargo.lock", scratch.0.join("Cargo.lock")).unwrap();
    let output = run(&scratch.0, &[("CHANGED_FILES", "member/src/lib.rs")]);
    assert_error(&output, "failed to inspect workspace Cargo.lock");
    assert!(!String::from_utf8_lossy(&output.stderr).contains("No Cargo.lock"));
}

#[cfg(unix)]
fn fake_cargo(scratch: &Scratch, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = scratch.0.join("cargo");
    std::fs::write(&path, format!("#!/bin/sh\nif [ \"$1\" = locate-project ]; then\n  printf '%s\\n' \"$FAKE_MANIFEST\"\n  exit 0\nfi\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[cfg(unix)]
#[test]
fn malformed_metadata_and_failed_cargo_have_context() {
    let scratch = Scratch::new();
    for (script, message) in [
        ("printf '{invalid json}\\n'", "invalid cargo metadata"),
        ("printf 'no JSON\\n'", "cargo metadata returned no JSON"),
        ("printf '\\377'", "cargo metadata output is not valid UTF-8"),
        (
            "printf 'notice\\nerror: broken 100%%\\n' >&2; exit 7",
            "::error::cargo metadata failed: error: broken 100%25",
        ),
        ("exit 7", "cargo metadata failed: exit status: 7"),
    ] {
        let executable = fake_cargo(&scratch, script);
        let output = command(&fixture())
            .env("CHANGED_FILES", "x")
            .env("CARGO", executable)
            .env("FAKE_MANIFEST", fixture().join("Cargo.toml"))
            .output()
            .unwrap();
        assert_error(&output, message);
    }
}

#[cfg(unix)]
#[test]
fn successful_cargo_stderr_is_forwarded_and_graph_errors_are_handled() {
    let scratch = Scratch::new();
    let metadata = Command::new(env!("CARGO"))
        .args([
            "metadata",
            "--format-version",
            "1",
            "--all-features",
            "--locked",
            "--offline",
        ])
        .current_dir(fixture())
        .env("CARGO_CACHE_RUSTC_INFO", "0")
        .output()
        .unwrap();
    assert!(metadata.status.success());
    let metadata_path = scratch.0.join("metadata.json");
    let executable = fake_cargo(
        &scratch,
        r#"
[ "$CARGO_TERM_COLOR" = never ] && [ "$RUSTUP_TERM_COLOR" = never ] && [ "$CARGO_CACHE_RUSTC_INFO" = 0 ] || exit 8
[ "$*" = 'metadata --format-version 1 --all-features --locked' ] || exit 9
printf 'cargo notice: retained\n' >&2
printf 'prelude before JSON\n'
cat "$FAKE_METADATA"
"#,
    );
    for valid_graph in [true, false] {
        let mut json: serde_json::Value = serde_json::from_slice(&metadata.stdout).unwrap();
        if !valid_graph {
            json["workspace_members"] = serde_json::json!(["missing-package-id"]);
        }
        std::fs::write(&metadata_path, serde_json::to_vec(&json).unwrap()).unwrap();
        let output = command(&fixture())
            .env("CHANGED_FILES", "lib-core/src/lib.rs")
            .env("CARGO", &executable)
            .env("FAKE_MANIFEST", fixture().join("Cargo.toml"))
            .env("FAKE_METADATA", &metadata_path)
            .env("CARGO_TERM_COLOR", "always")
            .env("RUSTUP_TERM_COLOR", "always")
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .lines()
                .any(|line| line == "cargo notice: retained")
        );
        if valid_graph {
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(json["changed_crates"], serde_json::json!(["lib-core"]));
        } else {
            assert_error(&output, "failed to build package graph");
        }
    }
}

#[test]
fn results_are_echoed_and_summary_is_appended_in_both_modes() {
    for (changed, trigger) in [
        ("", ""),
        ("app-alpha/src/main.rs", ""),
        ("Cargo.lock", "Cargo.lock"),
    ] {
        for file_output in [false, true] {
            let scratch = Scratch::new();
            let output_path = scratch.0.join("output");
            let summary_path = scratch.0.join("summary");
            std::fs::write(&output_path, "existing=value\n").unwrap();
            std::fs::write(&summary_path, "existing summary\n").unwrap();
            let mut cmd = command(&fixture());
            cmd.env("CHANGED_FILES", changed)
                .env("FORCE_TRIGGERS", trigger)
                .env("GITHUB_STEP_SUMMARY", &summary_path);
            if file_output {
                cmd.env("GITHUB_OUTPUT", &output_path);
            }
            let output = cmd.output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            if file_output {
                let content = std::fs::read_to_string(output_path).unwrap();
                assert_eq!(
                    content.strip_prefix("existing=value\n").unwrap().as_bytes(),
                    output.stdout
                );
                assert_eq!(String::from_utf8_lossy(&output.stdout).lines().count(), 4);
            } else {
                serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap();
            }
            let summary = std::fs::read_to_string(summary_path).unwrap();
            assert!(summary.starts_with("existing summary\n"));
            for text in [
                "## rust-affected",
                "| **Changed** |",
                "| **Affected libraries** |",
                "| **Affected binaries** |",
                "### Changed crates",
                "### Affected library members",
                "### Affected binary members",
            ] {
                assert!(summary.contains(text), "{summary}");
            }
            assert_eq!(summary.contains("[!WARNING]"), !trigger.is_empty());
            if changed.is_empty() {
                assert_eq!(summary.matches("_none_").count(), 3);
            }
            if changed.starts_with("app-alpha") {
                assert!(summary.contains("| **Changed** | `app-alpha` |"));
            }
        }
    }
}

#[test]
fn output_bytes_match_stage_three_in_both_modes() {
    let snapshots = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots");
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/root-package");
    for (name, changed, trigger, excluded) in [
        ("ordinary", "sub-lib/src/lib.rs src/main.rs", "", ""),
        ("force", "Cargo.lock", "Cargo.lock", ""),
        ("empty", "", "", ""),
        ("excluded", "sub-lib/src/lib.rs", "", "sub-lib"),
        ("warnings", "../bad%path sub-lib/src/lib.rs", "", "unknown"),
    ] {
        for file_output in [false, true] {
            let scratch = Scratch::new();
            let output_path = scratch.0.join("output");
            let summary_path = scratch.0.join("summary");
            std::fs::write(&output_path, "existing=value\n").unwrap();
            std::fs::write(&summary_path, "existing summary\n").unwrap();
            let mut cmd = command(&workspace);
            // Keep unrelated Cargo processes from adding package-cache lock notices.
            // This fixture has only local dependencies, so an empty cache is sufficient.
            cmd.env("CARGO_HOME", scratch.0.join("cargo-home"));
            cmd.env("CHANGED_FILES", changed)
                .env("FORCE_TRIGGERS", trigger)
                .env("EXCLUDED_MEMBERS", excluded)
                .env("GITHUB_STEP_SUMMARY", &summary_path);
            if file_output {
                cmd.env("GITHUB_OUTPUT", &output_path);
            }
            let output = cmd.output().unwrap();
            assert_eq!(output.status.code(), Some(0), "{name}: {output:?}");
            let extension = if file_output { "output" } else { "json" };
            assert_eq!(
                output.stdout,
                std::fs::read(snapshots.join(format!("{name}.{extension}"))).unwrap(),
                "{name}: stdout"
            );
            assert_eq!(
                output.stderr,
                std::fs::read(snapshots.join(format!("{name}.stderr"))).unwrap(),
                "{name}: stderr"
            );
            assert_eq!(
                std::fs::read(summary_path).unwrap(),
                std::fs::read(snapshots.join(format!("{name}.summary"))).unwrap(),
                "{name}: summary"
            );
            let mut expected_file = b"existing=value\n".to_vec();
            if file_output {
                expected_file.extend_from_slice(&output.stdout);
            }
            assert_eq!(
                std::fs::read(output_path).unwrap(),
                expected_file,
                "{name}: output file"
            );
        }
    }
}

#[test]
fn invalid_summary_does_not_publish_outputs() {
    let scratch = Scratch::new();
    let output_path = scratch.0.join("output");
    std::fs::write(&output_path, "sentinel\n").unwrap();
    let output = command(&fixture())
        .env("CHANGED_FILES", "x")
        .env("GITHUB_OUTPUT", &output_path)
        .env("GITHUB_STEP_SUMMARY", &scratch.0)
        .output()
        .unwrap();
    assert_error(&output, "GITHUB_STEP_SUMMARY");
    assert_eq!(std::fs::read_to_string(output_path).unwrap(), "sentinel\n");
}

// macOS filesystems require UTF-8 filenames; Linux permits arbitrary bytes.
#[cfg(target_os = "linux")]
#[test]
fn output_locations_can_be_non_utf8_paths() {
    use std::os::unix::ffi::OsStrExt;
    let scratch = Scratch::new();
    let path = scratch.0.join(OsStr::from_bytes(b"output-\xff"));
    std::fs::write(&path, "").unwrap();
    let output = command(&fixture())
        .env("CHANGED_FILES", "app-alpha/src/main.rs")
        .env("GITHUB_OUTPUT", &path)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(std::fs::read(path).unwrap(), output.stdout);
}

#[cfg(target_os = "linux")]
#[test]
fn real_output_write_failures_exit_one() {
    for destination in ["GITHUB_OUTPUT", "GITHUB_STEP_SUMMARY", "stdout"] {
        let mut cmd = command(&fixture());
        cmd.env("CHANGED_FILES", "app-alpha/src/main.rs");
        if destination == "stdout" {
            cmd.stdout(
                std::fs::OpenOptions::new()
                    .write(true)
                    .open("/dev/full")
                    .unwrap(),
            );
        } else {
            cmd.env(destination, "/dev/full");
        }
        assert_error(&cmd.output().unwrap(), destination);
    }
}

#[test]
fn unavailable_sources_fail_without_publishing_partial_results() {
    for dependency in [
        "unavailable-registry-crate = \"99\"",
        "unavailable-git-crate = { git = \"https://example.invalid/never-fetch\" }",
    ] {
        for changed in ["", "member/src/lib.rs"] {
            let scratch = Scratch::workspace();
            let manifest = scratch.0.join("member/Cargo.toml");
            let contents = std::fs::read_to_string(&manifest).unwrap();
            std::fs::write(
                &manifest,
                format!("{contents}\n[dependencies]\n{dependency}\n"),
            )
            .unwrap();
            let output_path = scratch.0.join("output");
            let summary_path = scratch.0.join("summary");
            std::fs::write(&output_path, "sentinel\n").unwrap();
            std::fs::write(&summary_path, "sentinel\n").unwrap();
            let output = command(&scratch.0)
                .env("CARGO_HOME", scratch.0.join("cargo-home"))
                .env("CHANGED_FILES", changed)
                .env("GITHUB_OUTPUT", &output_path)
                .env("GITHUB_STEP_SUMMARY", &summary_path)
                .output()
                .unwrap();
            assert_error(&output, "cargo metadata failed");
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.find("No Cargo.lock").unwrap() < stderr.find("error:").unwrap());
            assert_eq!(std::fs::read_to_string(output_path).unwrap(), "sentinel\n");
            assert_eq!(std::fs::read_to_string(summary_path).unwrap(), "sentinel\n");
        }
    }
}

#[test]
fn malformed_lockfiles_fail_without_modification() {
    for directory in ["", "member"] {
        for changed in ["", "member/src/lib.rs"] {
            let scratch = Scratch::workspace();
            let lock = scratch.0.join("Cargo.lock");
            std::fs::write(&lock, b"not toml\xff").unwrap();
            let output = run(&scratch.0.join(directory), &[("CHANGED_FILES", changed)]);
            assert_error(&output, "cargo metadata failed");
            assert_eq!(std::fs::read(lock).unwrap(), b"not toml\xff");
        }
    }
}

#[test]
fn git_source_patch_uses_cargos_selected_workspace_identity() {
    let scratch = Scratch::workspace();
    let repo = Scratch::new();
    std::fs::create_dir(repo.0.join("src")).unwrap();
    std::fs::write(
        repo.0.join("Cargo.toml"),
        "[package]\nname = \"member\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(repo.0.join("src/lib.rs"), "").unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["add", "."],
        vec![
            "-c",
            "user.name=test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "fixture",
        ],
    ] {
        assert!(
            Command::new("git")
                .current_dir(&repo.0)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    let url = format!("file://{}", repo.0.display());
    std::fs::create_dir_all(scratch.0.join("app/src")).unwrap();
    std::fs::write(scratch.0.join("app/src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(scratch.0.join("app/Cargo.toml"), format!("[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\nrenamed = {{ package = \"member\", git = \"{url}\" }}\n")).unwrap();
    std::fs::write(scratch.0.join("Cargo.toml"), format!("[workspace]\nmembers = [\"member\", \"app\"]\nresolver = \"2\"\n[patch.\"{url}\"]\nmember = {{ path = \"member\" }}\n")).unwrap();
    let cache = scratch.0.join("cargo-home");
    let metadata = Command::new(env!("CARGO"))
        .current_dir(&scratch.0)
        .args(["metadata", "--format-version", "1", "--all-features"])
        .env("CARGO_HOME", &cache)
        .env("CARGO_NET_OFFLINE", "false")
        .env("CARGO_CACHE_RUSTC_INFO", "0")
        .output()
        .unwrap();
    assert!(metadata.status.success(), "{metadata:?}");
    let data: serde_json::Value = serde_json::from_slice(&metadata.stdout).unwrap();
    let packages = data["packages"].as_array().unwrap();
    let member = packages.iter().find(|p| p["name"] == "member").unwrap();
    assert!(member["source"].is_null());
    let app = packages.iter().find(|p| p["name"] == "app").unwrap();
    let node = data["resolve"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == app["id"])
        .unwrap();
    assert_eq!(node["deps"][0]["pkg"], member["id"]);
    let output = command(&scratch.0)
        .env("CARGO_HOME", &cache)
        .env("CHANGED_FILES", "member/src/lib.rs")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        result["affected_binary_members"],
        serde_json::json!(["app"])
    );
}

#[cfg(unix)]
#[test]
fn locate_project_failures_are_reported_before_metadata() {
    let scratch = Scratch::new();
    for (script, expected) in [
        (
            "printf '\\377'",
            "cargo locate-project output is not valid UTF-8",
        ),
        (
            "printf 'relative/Cargo.toml\\n'",
            "no absolute workspace manifest path",
        ),
        ("printf '/'", "invalid workspace manifest path"),
        ("exit 0", "no absolute workspace manifest path"),
        (
            "printf 'error: locate failed\\n' >&2; exit 7",
            "cargo locate-project failed: error: locate failed",
        ),
    ] {
        let executable = fake_cargo(&scratch, "");
        std::fs::write(&executable, format!("#!/bin/sh\n[ \"$*\" = 'locate-project --workspace --message-format plain' ] || exit 9\n{script}\n")).unwrap();
        for changed in ["", "lib-core/src/lib.rs"] {
            let output = command(&fixture())
                .env("CARGO", &executable)
                .env("CHANGED_FILES", changed)
                .output()
                .unwrap();
            assert_error(&output, expected);
        }
    }
}

#[cfg(unix)]
#[test]
fn locked_failure_is_not_retried_and_commands_share_context() {
    let scratch = Scratch::new();
    let executable = fake_cargo(&scratch, "");
    let log = scratch.0.join("commands");
    std::fs::write(&executable, r#"#!/bin/sh
[ "$CARGO_TERM_COLOR" = never ] && [ "$RUSTUP_TERM_COLOR" = never ] && [ "$CARGO_CACHE_RUSTC_INFO" = 0 ] || exit 8
[ "$PWD" = "$EXPECTED_CWD" ] || exit 9
printf '%s\n' "$*" >> "$COMMAND_LOG"
case "$*" in
  'locate-project --workspace --message-format plain') printf '%s\n' "$FAKE_MANIFEST" ;;
  'metadata --format-version 1 --all-features --locked') printf 'error: locked failure\n' >&2; exit 7 ;;
  *) exit 10 ;;
esac
"#).unwrap();
    let cwd = fixture().join("lib-core");
    let output = command(&cwd)
        .env("CARGO", executable)
        .env("EXPECTED_CWD", &cwd)
        .env("COMMAND_LOG", &log)
        .env("FAKE_MANIFEST", fixture().join("Cargo.toml"))
        .env("CHANGED_FILES", "lib-core/src/lib.rs")
        .output()
        .unwrap();
    assert_error(&output, "cargo metadata failed: error: locked failure");
    assert_eq!(
        std::fs::read_to_string(log).unwrap(),
        "locate-project --workspace --message-format plain\nmetadata --format-version 1 --all-features --locked\n"
    );
}

#[cfg(unix)]
#[test]
fn unresolved_metadata_fails_even_for_empty_input_and_force_all() {
    let scratch = Scratch::new();
    let metadata = Command::new(env!("CARGO"))
        .current_dir(fixture())
        .args([
            "metadata",
            "--format-version",
            "1",
            "--all-features",
            "--no-deps",
            "--locked",
            "--offline",
        ])
        .env("CARGO_CACHE_RUSTC_INFO", "0")
        .output()
        .unwrap();
    assert!(metadata.status.success());
    let data = scratch.0.join("metadata.json");
    std::fs::write(&data, metadata.stdout).unwrap();
    let executable = fake_cargo(&scratch, "cat \"$FAKE_METADATA\"");
    for (changed, force) in [("", ""), ("Cargo.toml", "**"), ("lib-core/src/lib.rs", "")] {
        let output_file = scratch.0.join("output");
        let summary = scratch.0.join("summary");
        std::fs::write(&output_file, "sentinel\n").unwrap();
        std::fs::write(&summary, "sentinel\n").unwrap();
        let output = command(&fixture())
            .env("CARGO", &executable)
            .env("FAKE_MANIFEST", fixture().join("Cargo.toml"))
            .env("FAKE_METADATA", &data)
            .env("CHANGED_FILES", changed)
            .env("FORCE_TRIGGERS", force)
            .env("GITHUB_OUTPUT", &output_file)
            .env("GITHUB_STEP_SUMMARY", &summary)
            .output()
            .unwrap();
        assert_error(&output, "cargo metadata has no resolved graph");
        assert_eq!(std::fs::read_to_string(output_file).unwrap(), "sentinel\n");
        assert_eq!(std::fs::read_to_string(summary).unwrap(), "sentinel\n");
    }
}
