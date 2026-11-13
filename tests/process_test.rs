use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use tempfile::TempDir;

fn workspace() -> TempDir {
    let scratch = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(scratch.path().join("member/src")).unwrap();
    std::fs::write(
        scratch.path().join("Cargo.toml"),
        "[workspace]\nmembers = [\"member\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    std::fs::write(
        scratch.path().join("member/Cargo.toml"),
        "[package]\nname = \"member\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(scratch.path().join("member/src/lib.rs"), "").unwrap();
    scratch
}

/// The JSON array for a whitespace-separated list of paths.
fn json(files: &str) -> String {
    serde_json::to_string(&files.split_whitespace().collect::<Vec<_>>()).unwrap()
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
        "GITHUB_WORKSPACE",
        "CHANGED_FILES_JSON",
        "FORCE_TRIGGERS",
        "EXCLUDED_PACKAGES",
        "IGNORE_CARGO_CONFIG",
        "TRUST_CARGO_CONFIG",
        "INPUT_CHANGED_FILES_JSON",
        "INPUT_FORCE_TRIGGERS",
        "INPUT_EXCLUDED_PACKAGES",
        "INPUT_TRUST_CARGO_CONFIG",
        "INPUT_CHANGED_FILES",
        "INPUT_EXCLUDED_MEMBERS",
        "INPUT_IGNORE_CARGO_CONFIG",
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
    let scratch = tempfile::tempdir().unwrap();
    for changed in ["x", ""] {
        assert_error(
            &run(scratch.path(), &[("CHANGED_FILES_JSON", &json(changed))]),
            "could not find",
        );
    }
}

#[test]
fn external_targets_warn_without_changing_attribution() {
    let scratch = workspace();
    std::fs::create_dir(scratch.path().join("member-extra")).unwrap();
    std::fs::write(scratch.path().join("member-extra/lib.rs"), "").unwrap();
    std::fs::write(
        scratch.path().join("member-extra/main.rs"),
        "fn main() {}\n",
    )
    .unwrap();
    let manifest = scratch.path().join("member/Cargo.toml");
    let original = std::fs::read_to_string(&manifest).unwrap();
    for (targets, warns) in [
        ("[lib]\npath = \"../member/src/lib.rs\"\n", false),
        ("[lib]\npath = \"../member-extra/lib.rs\"\n", true),
        (
            "[[bin]]\nname = \"external\"\npath = \"../member-extra/main.rs\"\n",
            true,
        ),
        ("build = \"../member-extra/main.rs\"\n", true),
        (
            "[lib]\npath = \"../member-extra/lib.rs\"\n[[bin]]\nname = \"external\"\npath = \"../member-extra/main.rs\"\n",
            true,
        ),
    ] {
        std::fs::write(&manifest, format!("{original}{targets}")).unwrap();
        for (changed, trigger, excluded) in [
            ("member-extra/lib.rs", "", ""),
            ("", "", ""),
            ("member-extra/lib.rs", "member-extra/", "member"),
        ] {
            let output = run(
                scratch.path(),
                &[
                    ("CHANGED_FILES_JSON", &json(changed)),
                    ("FORCE_TRIGGERS", trigger),
                    ("EXCLUDED_PACKAGES", excluded),
                ],
            );
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(output.status.success(), "{stderr}");
            assert_eq!(
                stderr
                    .matches("workspace members declare targets outside")
                    .count(),
                usize::from(warns),
                "{stderr}"
            );
            if warns {
                assert!(stderr.contains("[\"member\"]"), "{stderr}");
                assert!(stderr.contains("Configure force_triggers"), "{stderr}");
            }
            let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(
                result,
                serde_json::json!({
                    "changed_packages": [],
                    "affected_libraries": [],
                    "affected_binaries": [],
                    "force_all": !trigger.is_empty(),
                })
            );
        }
    }
}

#[test]
fn invalid_glob_fails_even_without_changes() {
    for changed in ["lib-core/src/lib.rs", ""] {
        assert_error(
            &run(
                &fixture(),
                &[
                    ("CHANGED_FILES_JSON", &json(changed)),
                    ("FORCE_TRIGGERS", "[abc"),
                ],
            ),
            "invalid force_triggers pattern",
        );
    }
}

#[test]
fn unmatchable_trigger_fails_even_without_changes() {
    for changed in ["lib-core/src/lib.rs", ""] {
        for trigger in ["/Cargo.lock", "./", "infra/../infra/"] {
            assert_error(
                &run(
                    &fixture(),
                    &[
                        ("CHANGED_FILES_JSON", &json(changed)),
                        ("FORCE_TRIGGERS", trigger),
                    ],
                ),
                "can never match",
            );
        }
    }
}

const MEMBER_LOCK: &str = "version = 4\n\n[[package]]\nname = \"member\"\nversion = \"0.1.0\"\n";
const MEMBER_OUTPUT: &str = "{\"affected_binaries\":[],\"affected_libraries\":[\"member\"],\"changed_packages\":[\"member\"],\"force_all\":false}\n";

const CHANGED_MEMBER: (&str, &str) = ("CHANGED_FILES_JSON", r#"["member/src/lib.rs"]"#);

/// A locked workspace whose `.cargo/config.toml` makes Cargo run a program
/// that leaves a marker. Returns the workspace and the marker's path.
#[cfg(unix)]
fn workspace_with_planted_program() -> (TempDir, PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let scratch = workspace();
    std::fs::write(scratch.path().join("Cargo.lock"), MEMBER_LOCK).unwrap();
    let marker = scratch.path().join("executed");
    let program = scratch.path().join("rustc.sh");
    std::fs::write(
        &program,
        format!(
            "#!/bin/sh\necho ran >> '{}'\nexec rustc \"$@\"\n",
            marker.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::create_dir(scratch.path().join(".cargo")).unwrap();
    std::fs::write(
        scratch.path().join(".cargo/config.toml"),
        format!("[build]\nrustc = '{}'\n", program.display()),
    )
    .unwrap();
    (scratch, marker)
}

#[cfg(unix)]
#[test]
fn a_job_variable_cannot_replace_an_input() {
    // The runner sets INPUT_<NAME> for every declared input, so a variable the
    // job happens to define under the bare name must not be read instead.
    let (scratch, marker) = workspace_with_planted_program();
    for input in ["false", ""] {
        let output = command(scratch.path())
            .envs([
                CHANGED_MEMBER,
                ("INPUT_TRUST_CARGO_CONFIG", input),
                ("TRUST_CARGO_CONFIG", "true"),
            ])
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{stderr}");
        assert_eq!(String::from_utf8_lossy(&output.stdout), MEMBER_OUTPUT);
        assert!(!marker.exists(), "program ran with input {input:?}");
    }
    // An input left empty stays empty.
    let changed = ("INPUT_CHANGED_FILES_JSON", r#"["lib-utils/src/lib.rs"]"#);
    let output = run(
        &fixture(),
        &[
            changed,
            ("INPUT_EXCLUDED_PACKAGES", ""),
            ("EXCLUDED_PACKAGES", "tools/"),
            ("INPUT_FORCE_TRIGGERS", ""),
            ("FORCE_TRIGGERS", "**"),
        ],
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["force_all"], false);
    assert_eq!(
        json["affected_binaries"],
        serde_json::json!(["app-alpha", "app-beta", "tool-alpha"])
    );
    assert_error(
        &run(
            &fixture(),
            &[
                ("INPUT_CHANGED_FILES_JSON", ""),
                ("CHANGED_FILES_JSON", r#"["lib-utils/src/lib.rs"]"#),
            ],
        ),
        "changed_files_json is required",
    );
}

#[cfg(unix)]
#[test]
fn cargo_home_inside_an_untrusted_checkout_is_refused() {
    // Cargo reads $CARGO_HOME/config.toml wherever it starts.
    let (scratch, marker) = workspace_with_planted_program();
    let home = scratch.path().join(".cargo");
    let home = home.to_str().unwrap();
    let message = "is inside the checkout, so Cargo would read configuration from the repository although trust_cargo_config is false";
    for directory in ["", "member"] {
        let output = command(&scratch.path().join(directory))
            .envs([CHANGED_MEMBER, ("CARGO_HOME", home)])
            .output()
            .unwrap();
        assert_error(&output, message);
        assert!(!marker.exists(), "program ran from {directory:?}");
    }
    // A home that does not exist yet is refused too: the pull request could add it.
    let absent = scratch.path().join("member/absent-home");
    assert_error(
        &run(
            scratch.path(),
            &[CHANGED_MEMBER, ("CARGO_HOME", absent.to_str().unwrap())],
        ),
        message,
    );
    // GITHUB_WORKSPACE names the checkout even when no manifest sits at its top.
    let outer = tempfile::tempdir().unwrap();
    let inner = outer.path().join("crates");
    std::fs::create_dir_all(inner.join("src")).unwrap();
    std::fs::write(
        inner.join("Cargo.toml"),
        "[package]\nname = \"member\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(inner.join("src/lib.rs"), "").unwrap();
    let sibling = outer.path().join("cargo-home");
    let envs = [
        ("CHANGED_FILES_JSON", r#"["src/lib.rs"]"#),
        ("CARGO_HOME", sibling.to_str().unwrap()),
    ];
    assert!(run(&inner, &envs).status.success());
    let output = command(&inner)
        .envs(envs)
        .env("GITHUB_WORKSPACE", outer.path())
        .output()
        .unwrap();
    assert_error(&output, message);
    // Trusted, the checkout's configuration is meant to be read.
    let output = command(scratch.path())
        .envs([
            CHANGED_MEMBER,
            ("CARGO_HOME", home),
            ("TRUST_CARGO_CONFIG", "true"),
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(marker.exists());
}

/// The toolchain's own `rustc`, next to the `cargo` running these tests.
///
/// A run that gets past the guard with a relocated `RUSTUP_HOME` would
/// otherwise reach `rustc` through the rustup proxy, which looks for the
/// toolchain in that home and may try to download it.
fn toolchain_rustc() -> PathBuf {
    Path::new(env!("CARGO")).with_file_name(format!("rustc{}", std::env::consts::EXE_SUFFIX))
}

#[test]
fn rustup_home_inside_an_untrusted_checkout_is_refused() {
    // rustup runs cargo and rustc from $RUSTUP_HOME/toolchains wherever it starts.
    let scratch = workspace();
    std::fs::write(scratch.path().join("Cargo.lock"), MEMBER_LOCK).unwrap();
    let message = "is inside the checkout, so rustup would run a toolchain from the repository although trust_cargo_config is false";
    // The home need not exist yet: the pull request could add it.
    let home = scratch.path().join(".rustup");
    let home = home.to_str().unwrap();
    for directory in ["", "member"] {
        let output = run(
            &scratch.path().join(directory),
            &[CHANGED_MEMBER, ("RUSTUP_HOME", home)],
        );
        assert_error(&output, message);
        assert!(String::from_utf8_lossy(&output.stderr).contains("::error::RUSTUP_HOME ("));
    }
    // An acceptable CARGO_HOME does not end the check before RUSTUP_HOME is looked at.
    let outside = tempfile::tempdir().unwrap();
    let outside = outside.path().to_str().unwrap();
    assert_error(
        &run(
            scratch.path(),
            &[
                CHANGED_MEMBER,
                ("CARGO_HOME", outside),
                ("RUSTUP_HOME", home),
            ],
        ),
        message,
    );
    // Past the guard, Cargo is given rustc directly so that rustup is never consulted.
    let rustc = toolchain_rustc();
    let passing = |envs: &[(&str, &str)]| {
        let output = command(scratch.path())
            .envs([CHANGED_MEMBER, ("RUSTUP_AUTO_INSTALL", "0")])
            .env("RUSTC", &rustc)
            .envs(envs.iter().copied())
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{envs:?}: {stderr}");
        assert_eq!(String::from_utf8_lossy(&output.stdout), MEMBER_OUTPUT);
    };
    // A home outside the checkout is the job's own business.
    passing(&[("RUSTUP_HOME", outside)]);
    // Trusted, the checkout may supply whatever it likes.
    passing(&[("RUSTUP_HOME", home), ("TRUST_CARGO_CONFIG", "true")]);
    // GITHUB_WORKSPACE names the checkout even when no manifest sits at its top.
    let outer = tempfile::tempdir().unwrap();
    let inner = outer.path().join("crates");
    std::fs::create_dir_all(inner.join("src")).unwrap();
    std::fs::write(
        inner.join("Cargo.toml"),
        "[package]\nname = \"member\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(inner.join("src/lib.rs"), "").unwrap();
    let sibling = outer.path().join("rustup-home");
    let output = command(&inner)
        .envs([
            ("CHANGED_FILES_JSON", r#"["src/lib.rs"]"#),
            ("RUSTUP_HOME", sibling.to_str().unwrap()),
        ])
        .env("GITHUB_WORKSPACE", outer.path())
        .output()
        .unwrap();
    assert_error(&output, message);
}

#[cfg(unix)]
#[test]
fn repository_cargo_config_is_read_only_when_trusted() {
    let (scratch, marker) = workspace_with_planted_program();
    // Untrusted unless stated: the configured program never runs, from the root
    // or from a member. The switch removed in v5 cannot turn the isolation off.
    for (directory, extra) in [
        ("", None),
        ("member", None),
        ("", Some(("TRUST_CARGO_CONFIG", "false"))),
        ("", Some(("IGNORE_CARGO_CONFIG", "false"))),
    ] {
        let output = command(&scratch.path().join(directory))
            .envs([CHANGED_MEMBER])
            .envs(extra)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{stderr}");
        assert_eq!(stderr, "", "{directory:?} {extra:?}");
        assert_eq!(String::from_utf8_lossy(&output.stdout), MEMBER_OUTPUT);
        assert!(!marker.exists(), "program ran: {directory:?} {extra:?}");
    }
    // Trusted: Cargo honours the checkout's configuration and runs the program.
    command(scratch.path())
        .envs([CHANGED_MEMBER, ("TRUST_CARGO_CONFIG", "true")])
        .output()
        .unwrap();
    assert!(marker.exists());
    assert_eq!(
        std::fs::read_to_string(scratch.path().join("Cargo.lock")).unwrap(),
        MEMBER_LOCK
    );
}

#[test]
fn a_cargo_failure_names_the_unread_repository_config() {
    let patched = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/patch-config");
    let changed = ("CHANGED_FILES_JSON", r#"["core/src/lib.rs"]"#);
    let trusted = run(&patched, &[changed, ("TRUST_CARGO_CONFIG", "true")]);
    assert!(trusted.status.success());
    assert_eq!(String::from_utf8_lossy(&trusted.stderr), "");
    // The [patch] lives only in .cargo/config.toml; without it the lock is stale.
    let output = run(&patched, &[changed]);
    assert_error(&output, "cargo metadata failed");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(
            ".cargo/config.toml was not read because trust_cargo_config is false; set trust_cargo_config: true if this checkout is trusted and Cargo needs that configuration."
        ),
        "{stderr}"
    );
    // With no configuration to blame, the failure is reported as it is.
    let scratch = workspace();
    std::fs::write(scratch.path().join("Cargo.lock"), "version = 4\n").unwrap();
    let output = run(scratch.path(), &[CHANGED_MEMBER]);
    assert_error(&output, "cargo metadata failed");
    assert!(!String::from_utf8_lossy(&output.stderr).contains("trust_cargo_config"));
}

#[test]
fn an_unread_repository_config_without_a_lock_is_warned_about() {
    // Without a lock, ignoring a [patch] or source replacement changes the
    // resolved graph instead of failing, so the unread file must be named.
    let lock_warning = "::warning::No Cargo.lock in the workspace root;";
    let config_warning = "is not read because trust_cargo_config is false, and without a Cargo.lock a [patch] or source replacement defined there is silently not applied. Set trust_cargo_config: true if this checkout is trusted.";
    for (config, directory) in [
        (".cargo/config.toml", ""),
        (".cargo/config", ""),
        ("member/.cargo/config.toml", "member"),
        (".cargo/config.toml", "member"),
    ] {
        let scratch = workspace();
        let config_path = scratch.path().join(config);
        std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        std::fs::write(&config_path, "[net]\nretry = 1\n").unwrap();
        let cwd = scratch.path().join(directory);
        let lock = scratch.path().join("Cargo.lock");

        let output = run(&cwd, &[CHANGED_MEMBER]);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{stderr}");
        let lines: Vec<_> = stderr.lines().collect();
        assert_eq!(lines.len(), 2, "{stderr}");
        assert!(lines[0].starts_with(lock_warning), "{stderr}");
        assert!(
            lines[1].starts_with("::warning::")
                && lines[1].contains(config)
                && lines[1].ends_with(config_warning),
            "{stderr}"
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout), MEMBER_OUTPUT);

        // Trusted, the file is read, so only the missing lock is reported.
        std::fs::remove_file(&lock).unwrap();
        let output = run(&cwd, &[CHANGED_MEMBER, ("TRUST_CARGO_CONFIG", "true")]);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(stderr.lines().count(), 1, "{stderr}");
        assert!(stderr.starts_with(lock_warning), "{stderr}");

        // With a lock, a difference in resolution fails instead: nothing to say.
        let output = run(&cwd, &[CHANGED_MEMBER]);
        assert_eq!(String::from_utf8_lossy(&output.stderr), "");
        assert_eq!(String::from_utf8_lossy(&output.stdout), MEMBER_OUTPUT);
    }
    // A configuration file outside the workspace is not the repository's.
    let outer = tempfile::tempdir().unwrap();
    std::fs::create_dir(outer.path().join(".cargo")).unwrap();
    std::fs::write(outer.path().join(".cargo/config.toml"), "").unwrap();
    let inner = outer.path().join("checkout");
    std::fs::create_dir_all(inner.join("src")).unwrap();
    std::fs::write(
        inner.join("Cargo.toml"),
        "[package]\nname = \"member\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(inner.join("src/lib.rs"), "").unwrap();
    let output = run(&inner, &[("CHANGED_FILES_JSON", r#"["src/lib.rs"]"#)]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert_eq!(stderr.lines().count(), 1, "{stderr}");
    assert!(stderr.starts_with(lock_warning), "{stderr}");
}

#[test]
fn trust_cargo_config_requires_a_boolean_and_isolation_a_manifest() {
    for value in ["1", "yes", "TRUE", "True", " true"] {
        assert_error(
            &run(
                &fixture(),
                &[
                    ("CHANGED_FILES_JSON", r#"["lib-core/src/lib.rs"]"#),
                    ("TRUST_CARGO_CONFIG", value),
                ],
            ),
            "TRUST_CARGO_CONFIG must be 'true' or 'false'",
        );
    }
    let scratch = tempfile::tempdir().unwrap();
    assert_error(
        &run(scratch.path(), &[("CHANGED_FILES_JSON", r#"["x"]"#)]),
        "could not find Cargo.toml in the working directory or any parent directory",
    );
}

#[test]
fn uncovered_workspace_files_warn_without_changing_results() {
    let output = run(
        &fixture(),
        &[(
            "CHANGED_FILES_JSON",
            r#"["Cargo.lock","./Cargo.toml","README.md"]"#,
        )],
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert_eq!(stderr.lines().count(), 1, "{stderr}");
    assert!(
        stderr.starts_with("::warning::workspace-wide files changed but belong to no package and matched no force trigger, so nothing was selected for them: [\"Cargo.lock\", \"Cargo.toml\"]."),
        "{stderr}"
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "{\"affected_binaries\":[],\"affected_libraries\":[],\"changed_packages\":[],\"force_all\":false}\n"
    );
}

#[test]
fn json_paths_are_exact_strings() {
    let scratch = workspace();
    std::fs::write(scratch.path().join("Cargo.lock"), MEMBER_LOCK).unwrap();
    // Nothing is split, trimmed or decoded: each element is one path.
    for files in [
        r#"["member/src/lib.rs"]"#,
        r#"["member/src/my file.rs"]"#,
        r#"["member/src/café.rs"]"#,
        r#"["member/src/café.rs"]"#,
        r#"["member/src/a\"b.rs"]"#,
        r#"["member/src/a\\b.rs"]"#,
        r#"["member/src/tab\there.rs", "member/src/line\nbreak.rs"]"#,
        "  [\n  \"member/src/lib.rs\" ,\n  \"member/Cargo.toml\"\n]\n",
        r#"["README.md", "member/src/lib.rs", "member/src/lib.rs"]"#,
    ] {
        let output = run(scratch.path(), &[("CHANGED_FILES_JSON", files)]);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{files}: {stderr}");
        assert_eq!(stderr, "", "{files}");
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            MEMBER_OUTPUT,
            "{files}"
        );
    }
    // Neither is it split or unquoted: a space keeps "member/src/lib.rs" inside
    // an unowned directory, and git's quoted form is a different, unowned path.
    let output = run(
        scratch.path(),
        &[(
            "CHANGED_FILES_JSON",
            r#"["docs/notes member/src/lib.rs", "\"member/src/caf\\303\\251.rs\"", "\"member/src/lib.rs\""]"#,
        )],
    );
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "{\"affected_binaries\":[],\"affected_libraries\":[],\"changed_packages\":[],\"force_all\":false}\n"
    );
}

#[test]
fn changed_files_json_is_required_and_must_be_an_array_of_strings() {
    let scratch = workspace();
    std::fs::write(scratch.path().join("Cargo.lock"), MEMBER_LOCK).unwrap();
    for envs in [
        vec![],
        vec![("CHANGED_FILES_JSON", "")],
        vec![("CHANGED_FILES_JSON", " \n\t")],
        // The input removed in v5 is not read.
        vec![("CHANGED_FILES", "member/src/lib.rs")],
    ] {
        assert_error(
            &run(scratch.path(), &envs),
            "changed_files_json is required: pass a JSON array of paths, or [] when nothing changed",
        );
    }
    for value in [
        "member/src/lib.rs",
        "member/src/lib.rs member/Cargo.toml",
        r#""member/src/lib.rs""#,
        r#"{"files":["member/src/lib.rs"]}"#,
        r#"["member/src/lib.rs", 1]"#,
        r#"[["member/src/lib.rs"]]"#,
        "[null]",
        "null",
        r#"["member/src/lib.rs"] ["member/Cargo.toml"]"#,
        r#"["member/src/lib.rs""#,
        r#"[\"member/src/lib.rs\"]"#,
    ] {
        assert_error(
            &run(scratch.path(), &[("CHANGED_FILES_JSON", value)]),
            "changed_files_json must be a JSON array of strings",
        );
    }
}

#[test]
fn an_empty_array_is_valid_and_still_warns() {
    for value in ["[]", " [ ] "] {
        let output = run(&fixture(), &[("CHANGED_FILES_JSON", value)]);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{stderr}");
        assert_eq!(stderr.lines().count(), 1, "{stderr}");
        assert!(
            stderr.starts_with("::warning::changed_files_json is empty;"),
            "{stderr}"
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "{\"affected_binaries\":[],\"affected_libraries\":[],\"changed_packages\":[],\"force_all\":false}\n"
        );
    }
}

#[test]
fn inputs_removed_in_v5_fail_instead_of_being_ignored() {
    // The runner hands every `with:` key to the container as INPUT_<NAME>,
    // including ones action.yml no longer declares.
    let changed = ("CHANGED_FILES_JSON", r#"["lib-core/src/lib.rs"]"#);
    for (variable, advice) in [
        (
            "INPUT_CHANGED_FILES",
            "changed_files (use changed_files_json, a JSON array of paths)",
        ),
        (
            "INPUT_EXCLUDED_MEMBERS",
            "excluded_members (renamed to excluded_packages)",
        ),
        (
            "INPUT_IGNORE_CARGO_CONFIG",
            "ignore_cargo_config (Cargo configuration is now unread unless trust_cargo_config is true)",
        ),
    ] {
        for value in ["lib-utils", "false"] {
            let output = run(&fixture(), &[changed, (variable, value)]);
            assert_error(&output, "inputs removed in v5 were passed: ");
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.contains(advice), "{stderr}");
            assert!(
                stderr.contains("See \"Migrating from v4\" in the README."),
                "{stderr}"
            );
        }
    }
    // Every removed input that was passed is named, in one error, before anything else is read.
    let output = run(
        &fixture(),
        &[
            ("INPUT_EXCLUDED_MEMBERS", "tools/"),
            ("INPUT_CHANGED_FILES", "lib-core/src/lib.rs"),
            ("FORCE_TRIGGERS", "[abc"),
        ],
    );
    assert_error(
        &output,
        "inputs removed in v5 were passed: changed_files (use changed_files_json, a JSON array of paths); excluded_members (renamed to excluded_packages). See",
    );
    // A blank value sets nothing, so it is not worth failing over, and the
    // current inputs' own INPUT_ variables are not mistaken for removed ones.
    let output = run(
        &fixture(),
        &[
            changed,
            ("INPUT_CHANGED_FILES", ""),
            ("INPUT_EXCLUDED_MEMBERS", " \n"),
            ("INPUT_IGNORE_CARGO_CONFIG", ""),
            ("INPUT_CHANGED_FILES_JSON", r#"["lib-core/src/lib.rs"]"#),
            ("INPUT_EXCLUDED_PACKAGES", "tools/"),
            ("INPUT_TRUST_CARGO_CONFIG", "false"),
        ],
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert_eq!(stderr, "");
    // What the runner really sets for a migrated workflow: names that merely
    // begin like a removed one, and nothing else.
    let output = run(
        &fixture(),
        &[
            changed,
            ("INPUT_CHANGED_FILES_JSON", r#"["lib-core/src/lib.rs"]"#),
            ("INPUT_EXCLUDED_PACKAGES", "tools/"),
            ("INPUT_TRUST_CARGO_CONFIG", "false"),
        ],
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert_eq!(stderr, "");
}

#[test]
fn warnings_are_grouped_and_do_not_pollute_json() {
    for (envs, messages) in [
        (
            vec![("CHANGED_FILES_JSON", "[]")],
            vec!["changed_files_json is empty"],
        ),
        (
            vec![
                ("CHANGED_FILES_JSON", r#"["../x","../x","/outside/file"]"#),
                ("FORCE_TRIGGERS", "**"),
                ("EXCLUDED_PACKAGES", "tools unknown"),
            ],
            vec!["changed file", "excluded_packages"],
        ),
        (
            vec![("CHANGED_FILES_JSON", "[]"), ("EXCLUDED_PACKAGES", "tools")],
            vec!["changed_files_json is empty", "excluded_packages"],
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
        assert_eq!(json["changed_packages"], serde_json::json!([]));
    }
}

#[cfg(unix)]
#[test]
fn every_input_rejects_invalid_utf8_even_without_changes() {
    use std::os::unix::ffi::OsStrExt;
    for key in ["CHANGED_FILES_JSON", "FORCE_TRIGGERS", "EXCLUDED_PACKAGES"] {
        for changed in ["", "lib-core/src/lib.rs"] {
            let output = command(&fixture())
                .env("CHANGED_FILES_JSON", json(changed))
                .env(key, OsStr::from_bytes(b"\xff"))
                .output()
                .unwrap();
            assert_error(&output, &format!("{key} is not valid UTF-8"));
        }
    }
}

#[test]
fn output_destination_failures_are_errors() {
    let scratch = tempfile::tempdir().unwrap();
    for key in ["GITHUB_OUTPUT", "GITHUB_STEP_SUMMARY"] {
        let output = command(&fixture())
            .env("CHANGED_FILES_JSON", r#"["lib-core/src/lib.rs"]"#)
            .env(key, scratch.path())
            .output()
            .unwrap();
        assert_error(&output, key);
    }
}

#[test]
fn metadata_preserves_existing_compiler_cache() {
    let scratch = workspace();
    std::fs::create_dir(scratch.path().join("target")).unwrap();
    let cache = scratch.path().join("target/.rustc_info.json");
    let original = b"{\"rustc_fingerprint\":0,\"outputs\":{},\"successes\":{}}";
    std::fs::write(&cache, original).unwrap();
    let output = command(scratch.path())
        .env("CHANGED_FILES_JSON", r#"["member/src/lib.rs"]"#)
        .env("CARGO_TARGET_DIR", scratch.path().join("target"))
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
            let scratch = workspace();
            let lock = scratch.path().join("Cargo.lock");
            std::fs::write(&lock, "version = 4\n").unwrap();
            let output = run(
                &scratch.path().join(directory),
                &[("CHANGED_FILES_JSON", &json(changed))],
            );
            assert_error(&output, "lock file");
            assert_eq!(std::fs::read_to_string(lock).unwrap(), "version = 4\n");
        }
    }
}

#[test]
fn missing_locks_warn_and_are_created_even_without_changes() {
    for directory in ["", "member"] {
        for changed in ["", "member/src/lib.rs"] {
            let scratch = workspace();
            let output = run(
                &scratch.path().join(directory),
                &[("CHANGED_FILES_JSON", &json(changed))],
            );
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(output.status.success(), "{stderr}");
            assert!(
                stderr.contains("::warning::No Cargo.lock in the workspace root"),
                "{stderr}"
            );
            assert!(scratch.path().join("Cargo.lock").is_file());
            assert!(!scratch.path().join("member/Cargo.lock").exists());
        }
    }
}

#[test]
fn valid_locks_are_preserved_without_a_missing_lock_warning() {
    let scratch = workspace();
    let lock = scratch.path().join("Cargo.lock");
    std::fs::write(
        &lock,
        "version = 4\n\n[[package]]\nname = \"member\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let original = std::fs::read(&lock).unwrap();
    for directory in ["", "member"] {
        let output = run(
            &scratch.path().join(directory),
            &[("CHANGED_FILES_JSON", r#"["member/src/lib.rs"]"#)],
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
fn cargo_errors_are_summarized_without_color_and_outputs_are_untouched() {
    let scratch = tempfile::tempdir().unwrap();
    let output_path = scratch.path().join("output");
    let summary_path = scratch.path().join("summary");
    std::fs::write(&output_path, "sentinel\n").unwrap();
    std::fs::write(&summary_path, "sentinel\n").unwrap();
    let output = command(scratch.path())
        .env("CHANGED_FILES_JSON", r#"["x"]"#)
        .env("CARGO_TERM_COLOR", "always")
        .env("RUSTUP_TERM_COLOR", "always")
        .env("GITHUB_OUTPUT", &output_path)
        .env("GITHUB_STEP_SUMMARY", &summary_path)
        .output()
        .unwrap();
    assert_error(&output, "could not find");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(stderr.lines().count(), 1, "{stderr}");
    assert!(!output.stderr.contains(&0x1b));
    assert_eq!(std::fs::read_to_string(output_path).unwrap(), "sentinel\n");
    assert_eq!(std::fs::read_to_string(summary_path).unwrap(), "sentinel\n");
}

#[test]
fn cargo_config_warning_commands_are_not_forwarded() {
    let scratch = workspace();
    std::fs::write(
        scratch.path().join("Cargo.lock"),
        "version = 4\n\n[[package]]\nname = \"member\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    std::fs::create_dir(scratch.path().join(".cargo")).unwrap();
    std::fs::write(
        scratch.path().join(".cargo/config.toml"),
        "[build]\n\"unused\\n::add-mask::member\\n##[warning]legacy-injection\\n::stop-commands::attacker\\nunused\" = true\n",
    )
    .unwrap();
    let output_path = scratch.path().join("output");
    let summary_path = scratch.path().join("summary");
    std::fs::write(&output_path, "").unwrap();
    std::fs::write(&summary_path, "").unwrap();
    let output = command(scratch.path())
        // Cargo only warns about this configuration when it reads it.
        .env("TRUST_CARGO_CONFIG", "true")
        .env("CHANGED_FILES_JSON", r#"["member/src/lib.rs"]"#)
        .env("EXCLUDED_PACKAGES", "unknown")
        .env("GITHUB_OUTPUT", &output_path)
        .env("GITHUB_STEP_SUMMARY", &summary_path)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert_eq!(stderr.lines().count(), 1, "{stderr}");
    assert!(
        stderr.starts_with("::warning::excluded_packages entries matched no workspace package:"),
        "{stderr}"
    );
    assert_eq!(
        output.stdout,
        b"changed_packages=[\"member\"]\naffected_libraries=[\"member\"]\naffected_binaries=[]\nforce_all=false\n"
    );
    assert_eq!(std::fs::read(output_path).unwrap(), output.stdout);
    assert!(
        std::fs::read_to_string(summary_path)
            .unwrap()
            .contains("| **Affected libraries** | `member` |")
    );
}

#[test]
fn cargo_failure_diagnostic_commands_are_not_forwarded() {
    let scratch = workspace();
    std::fs::write(
        scratch.path().join("Cargo.lock"),
        "version = 4\n\n[[package]]\nname = \"member\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let manifest = scratch.path().join("member/Cargo.toml");
    let contents = std::fs::read_to_string(&manifest).unwrap();
    std::fs::write(
        manifest,
        format!(
            "{contents}\n[dependencies]\nmissing = {{ path = \"missing\\n::add-mask::member\\n##[warning]legacy-injection\\n::error::spoof\" }}\n"
        ),
    )
    .unwrap();
    let output_path = scratch.path().join("output");
    let summary_path = scratch.path().join("summary");
    std::fs::write(&output_path, "sentinel\n").unwrap();
    std::fs::write(&summary_path, "sentinel\n").unwrap();
    let output = command(scratch.path())
        .env("CHANGED_FILES_JSON", r#"["member/src/lib.rs"]"#)
        .env("GITHUB_OUTPUT", &output_path)
        .env("GITHUB_STEP_SUMMARY", &summary_path)
        .output()
        .unwrap();
    assert_error(&output, "cargo metadata failed");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(stderr.lines().count(), 1, "{stderr}");
    assert!(stderr.contains("failed to load manifest"), "{stderr}");
    assert_eq!(std::fs::read_to_string(output_path).unwrap(), "sentinel\n");
    assert_eq!(std::fs::read_to_string(summary_path).unwrap(), "sentinel\n");
}

#[test]
fn missing_cargo_executable_is_a_handled_error() {
    let scratch = tempfile::tempdir().unwrap();
    let output = command(&fixture())
        .env("CHANGED_FILES_JSON", r#"["x"]"#)
        .env("CARGO", scratch.path().join("missing-cargo"))
        .output()
        .unwrap();
    assert_error(&output, "failed to start cargo locate-project");
}

#[cfg(unix)]
#[test]
fn lock_inspection_errors_do_not_enable_unlocked_resolution() {
    let scratch = workspace();
    std::os::unix::fs::symlink("Cargo.lock", scratch.path().join("Cargo.lock")).unwrap();
    let output = run(
        scratch.path(),
        &[("CHANGED_FILES_JSON", r#"["member/src/lib.rs"]"#)],
    );
    assert_error(&output, "failed to inspect workspace Cargo.lock");
    assert!(!String::from_utf8_lossy(&output.stderr).contains("No Cargo.lock"));
}

#[cfg(unix)]
fn fake_cargo(scratch: &TempDir, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = scratch.path().join("cargo");
    std::fs::write(&path, format!("#!/bin/sh\nif [ \"$1\" = locate-project ]; then\n  printf '%s\\n' \"$FAKE_MANIFEST\"\n  exit 0\nfi\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[cfg(unix)]
#[test]
fn malformed_metadata_and_failed_cargo_have_context() {
    let scratch = tempfile::tempdir().unwrap();
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
            .env("CHANGED_FILES_JSON", r#"["x"]"#)
            .env("CARGO", executable)
            .env("FAKE_MANIFEST", fixture().join("Cargo.toml"))
            .output()
            .unwrap();
        assert_error(&output, message);
    }
}

#[cfg(unix)]
#[test]
fn successful_cargo_stderr_is_suppressed_and_graph_errors_are_handled() {
    let scratch = tempfile::tempdir().unwrap();
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
    let metadata_path = scratch.path().join("metadata.json");
    let executable = fake_cargo(
        &scratch,
        r#"
[ "$CARGO_TERM_COLOR" = never ] && [ "$RUSTUP_TERM_COLOR" = never ] && [ "$CARGO_CACHE_RUSTC_INFO" = 0 ] || exit 8
[ "$*" = 'metadata --format-version 1 --all-features --locked' ] || exit 9
printf 'cargo notice: suppressed\n' >&2
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
            .env("CHANGED_FILES_JSON", r#"["lib-core/src/lib.rs"]"#)
            .env("CARGO", &executable)
            // The fake cargo accepts only the trusted command line.
            .env("TRUST_CARGO_CONFIG", "true")
            .env("FAKE_MANIFEST", fixture().join("Cargo.toml"))
            .env("FAKE_METADATA", &metadata_path)
            .env("CARGO_TERM_COLOR", "always")
            .env("RUSTUP_TERM_COLOR", "always")
            .output()
            .unwrap();
        assert!(!String::from_utf8_lossy(&output.stderr).contains("cargo notice: suppressed"));
        if valid_graph {
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(output.stderr, Vec::<u8>::new());
            let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(json["changed_packages"], serde_json::json!(["lib-core"]));
        } else {
            assert_error(&output, "workspace member 'missing-package-id' not found");
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
            let scratch = tempfile::tempdir().unwrap();
            let output_path = scratch.path().join("output");
            let summary_path = scratch.path().join("summary");
            std::fs::write(&output_path, "existing=value\n").unwrap();
            std::fs::write(&summary_path, "existing summary\n").unwrap();
            let mut cmd = command(&fixture());
            cmd.env("CHANGED_FILES_JSON", json(changed))
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
                "### Changed packages",
                "### Affected libraries",
                "### Affected binaries",
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
            let scratch = tempfile::tempdir().unwrap();
            let output_path = scratch.path().join("output");
            let summary_path = scratch.path().join("summary");
            std::fs::write(&output_path, "existing=value\n").unwrap();
            std::fs::write(&summary_path, "existing summary\n").unwrap();
            let mut cmd = command(&workspace);
            // Keep unrelated Cargo processes from adding package-cache lock notices.
            // This fixture has only local dependencies, so an empty cache is sufficient.
            cmd.env("CARGO_HOME", scratch.path().join("cargo-home"));
            cmd.env("CHANGED_FILES_JSON", json(changed))
                .env("FORCE_TRIGGERS", trigger)
                .env("EXCLUDED_PACKAGES", excluded)
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
    let scratch = tempfile::tempdir().unwrap();
    let output_path = scratch.path().join("output");
    std::fs::write(&output_path, "sentinel\n").unwrap();
    let output = command(&fixture())
        .env("CHANGED_FILES_JSON", r#"["x"]"#)
        .env("GITHUB_OUTPUT", &output_path)
        .env("GITHUB_STEP_SUMMARY", scratch.path())
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
    let scratch = tempfile::tempdir().unwrap();
    let path = scratch.path().join(OsStr::from_bytes(b"output-\xff"));
    std::fs::write(&path, "").unwrap();
    let output = command(&fixture())
        .env("CHANGED_FILES_JSON", r#"["app-alpha/src/main.rs"]"#)
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
        cmd.env("CHANGED_FILES_JSON", r#"["app-alpha/src/main.rs"]"#);
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
            let scratch = workspace();
            let manifest = scratch.path().join("member/Cargo.toml");
            let contents = std::fs::read_to_string(&manifest).unwrap();
            std::fs::write(
                &manifest,
                format!("{contents}\n[dependencies]\n{dependency}\n"),
            )
            .unwrap();
            let output_path = scratch.path().join("output");
            let summary_path = scratch.path().join("summary");
            std::fs::write(&output_path, "sentinel\n").unwrap();
            std::fs::write(&summary_path, "sentinel\n").unwrap();
            // An empty Cargo home, kept outside the checkout as isolation requires.
            let home = tempfile::tempdir().unwrap();
            let output = command(scratch.path())
                .env("CARGO_HOME", home.path())
                .env("CHANGED_FILES_JSON", json(changed))
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
            let scratch = workspace();
            let lock = scratch.path().join("Cargo.lock");
            std::fs::write(&lock, b"not toml\xff").unwrap();
            let output = run(
                &scratch.path().join(directory),
                &[("CHANGED_FILES_JSON", &json(changed))],
            );
            assert_error(&output, "cargo metadata failed");
            assert_eq!(std::fs::read(lock).unwrap(), b"not toml\xff");
        }
    }
}

#[test]
fn git_source_patch_uses_cargos_selected_workspace_identity() {
    let scratch = workspace();
    let repo = tempfile::tempdir().unwrap();
    std::fs::create_dir(repo.path().join("src")).unwrap();
    std::fs::write(
        repo.path().join("Cargo.toml"),
        "[package]\nname = \"member\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(repo.path().join("src/lib.rs"), "").unwrap();
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
                .current_dir(repo.path())
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    let url = format!("file://{}", repo.path().display());
    std::fs::create_dir_all(scratch.path().join("app/src")).unwrap();
    std::fs::write(scratch.path().join("app/src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(scratch.path().join("app/Cargo.toml"), format!("[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\nrenamed = {{ package = \"member\", git = \"{url}\" }}\n")).unwrap();
    std::fs::write(scratch.path().join("Cargo.toml"), format!("[workspace]\nmembers = [\"member\", \"app\"]\nresolver = \"2\"\n[patch.\"{url}\"]\nmember = {{ path = \"member\" }}\n")).unwrap();
    // An empty Cargo home, kept outside the checkout as isolation requires.
    let home = tempfile::tempdir().unwrap();
    let cache = home.path().join("cargo-home");
    let metadata = Command::new(env!("CARGO"))
        .current_dir(scratch.path())
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
    let output = command(scratch.path())
        .env("CARGO_HOME", &cache)
        .env("CHANGED_FILES_JSON", r#"["member/src/lib.rs"]"#)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["affected_binaries"], serde_json::json!(["app"]));
}

#[cfg(unix)]
#[test]
fn locate_project_failures_are_reported_before_metadata() {
    let scratch = tempfile::tempdir().unwrap();
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
                // The fake cargo accepts only the trusted command line.
                .env("TRUST_CARGO_CONFIG", "true")
                .env("CHANGED_FILES_JSON", json(changed))
                .output()
                .unwrap();
            assert_error(&output, expected);
        }
    }
}

#[cfg(unix)]
#[test]
fn locked_failure_is_not_retried_and_commands_share_context() {
    let scratch = tempfile::tempdir().unwrap();
    let executable = fake_cargo(&scratch, "");
    let log = scratch.path().join("commands");
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
        // Trusted, both commands run in the working directory itself.
        .env("TRUST_CARGO_CONFIG", "true")
        .env("FAKE_MANIFEST", fixture().join("Cargo.toml"))
        .env("CHANGED_FILES_JSON", r#"["lib-core/src/lib.rs"]"#)
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
    let scratch = tempfile::tempdir().unwrap();
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
    let data = scratch.path().join("metadata.json");
    std::fs::write(&data, metadata.stdout).unwrap();
    let executable = fake_cargo(&scratch, "cat \"$FAKE_METADATA\"");
    for (changed, force) in [("", ""), ("Cargo.toml", "**"), ("lib-core/src/lib.rs", "")] {
        let output_file = scratch.path().join("output");
        let summary = scratch.path().join("summary");
        std::fs::write(&output_file, "sentinel\n").unwrap();
        std::fs::write(&summary, "sentinel\n").unwrap();
        let output = command(&fixture())
            .env("CARGO", &executable)
            .env("FAKE_MANIFEST", fixture().join("Cargo.toml"))
            .env("FAKE_METADATA", &data)
            .env("CHANGED_FILES_JSON", json(changed))
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

#[cfg(unix)]
#[test]
fn invalid_metadata_annotations_are_escaped_and_outputs_remain_untouched() {
    let scratch = tempfile::tempdir().unwrap();
    let metadata = Command::new(env!("CARGO"))
        .current_dir(fixture())
        .args([
            "metadata",
            "--format-version",
            "1",
            "--all-features",
            "--locked",
            "--offline",
        ])
        .env("CARGO_CACHE_RUSTC_INFO", "0")
        .output()
        .unwrap();
    assert!(metadata.status.success());
    let valid: serde_json::Value = serde_json::from_slice(&metadata.stdout).unwrap();
    let data_path = scratch.path().join("metadata.json");
    let executable = fake_cargo(&scratch, "cat \"$FAKE_METADATA\"");
    let output_file = scratch.path().join("output");
    let summary = scratch.path().join("summary");
    for mutation in ["member", "dependency", "deps", "version"] {
        let mut data = valid.clone();
        let invalid_id = serde_json::json!("missing%id\n::warning::spoof");
        match mutation {
            "member" => data["workspace_members"][0] = invalid_id,
            "dependency" => {
                let node = data["resolve"]["nodes"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|node| !node["deps"].as_array().unwrap().is_empty())
                    .unwrap();
                node["dependencies"][0] = invalid_id.clone();
                node["deps"][0]["pkg"] = invalid_id;
            }
            "deps" => {
                data["resolve"]["nodes"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("deps");
            }
            "version" => data["version"] = serde_json::json!(2),
            _ => unreachable!(),
        }
        std::fs::write(&data_path, data.to_string()).unwrap();
        std::fs::write(&output_file, "sentinel\n").unwrap();
        std::fs::write(&summary, "sentinel\n").unwrap();
        let output = command(&fixture())
            .env("CARGO", &executable)
            .env("FAKE_MANIFEST", fixture().join("Cargo.toml"))
            .env("FAKE_METADATA", &data_path)
            .env("CHANGED_FILES_JSON", r#"["lib-core/src/lib.rs"]"#)
            .env("GITHUB_OUTPUT", &output_file)
            .env("GITHUB_STEP_SUMMARY", &summary)
            .output()
            .unwrap();
        assert_error(&output, "failed to build");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(stderr.lines().count(), 1, "{stderr}");
        if matches!(mutation, "member" | "dependency") {
            assert!(
                stderr.contains("missing%25id%0A::warning::spoof"),
                "{stderr}"
            );
        }
        assert_eq!(std::fs::read_to_string(&output_file).unwrap(), "sentinel\n");
        assert_eq!(std::fs::read_to_string(&summary).unwrap(), "sentinel\n");
    }
}
