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
    assert!(output.stdout.is_empty());
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
