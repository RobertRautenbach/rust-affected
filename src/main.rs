use guppy::MetadataCommand;
use rust_affected::{AffectedResult, Warning, WorkspaceGraph, compute_affected};
use std::{
    collections::HashSet,
    env,
    ffi::OsString,
    fs::File,
    io::{self, Write},
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    if let Err(message) = run() {
        // A closed stderr must not turn a handled failure into a second panic.
        let _ = annotation("error", &message);
        std::process::exit(1);
    }
}

fn annotation(level: &str, message: &str) -> Result<(), String> {
    writeln!(io::stderr().lock(), "{}", format_annotation(level, message))
        .map_err(|error| format!("failed to write diagnostic: {error}"))
}

fn format_annotation(level: &str, message: &str) -> String {
    let escaped = message
        .replace('%', "%25")
        .replace('\r', "%0D")
        .replace('\n', "%0A");
    format!("::{level}::{escaped}")
}

/// An input's raw value.
///
/// The runner sets `INPUT_<NAME>` for every input `action.yml` declares, over
/// whatever the job's environment holds, so that is what the workflow passed.
/// The bare `<NAME>` is read only when `INPUT_<NAME>` is absent, which never
/// happens on a runner: there a job-level variable that happens to share the
/// name must not replace the input, least of all `TRUST_CARGO_CONFIG`.
fn raw(name: &str) -> OsString {
    env::var_os(format!("INPUT_{name}"))
        .or_else(|| env::var_os(name))
        .unwrap_or_default()
}

fn text(name: &str) -> Result<String, String> {
    raw(name)
        .into_string()
        .map_err(|_| format!("{name} is not valid UTF-8"))
}

fn input(name: &str) -> Result<Vec<String>, String> {
    Ok(text(name)?.split_whitespace().map(String::from).collect())
}

/// Changed paths arrive as a JSON array, so each element is one exact path:
/// nothing is split on whitespace, unquoted or otherwise interpreted here.
/// An empty array is valid but worth a warning.
fn changed_files() -> Result<Vec<String>, String> {
    let value = text("CHANGED_FILES_JSON")?;
    // An absent value is a broken or unmigrated workflow, not "nothing changed".
    if value.trim().is_empty() {
        return Err(
            "changed_files_json is required: pass a JSON array of paths, or [] when nothing changed"
                .into(),
        );
    }
    let files: Vec<String> = serde_json::from_str(&value)
        .map_err(|error| format!("changed_files_json must be a JSON array of strings: {error}"))?;
    if files.is_empty() {
        annotation(
            "warning",
            "changed_files_json is empty; no files will be attributed. If files did change, check the step producing changed_files_json. Workspace and configuration validation still run.",
        )?;
    }
    Ok(files)
}

fn flag(name: &str) -> Result<bool, String> {
    // Anything but an exact boolean fails, so a typo cannot disable a safeguard.
    match raw(name).to_str() {
        Some("" | "false") => Ok(false),
        Some("true") => Ok(true),
        _ => Err(format!("{name} must be 'true' or 'false'")),
    }
}

/// Inputs removed in v5, and what to do instead.
const REMOVED_INPUTS: [(&str, &str); 3] = [
    (
        "changed_files",
        "use changed_files_json, a JSON array of paths",
    ),
    ("excluded_members", "renamed to excluded_packages"),
    (
        "ignore_cargo_config",
        "Cargo configuration is now unread unless trust_cargo_config is true",
    ),
];

/// Fail when a workflow still passes an input that no longer exists.
///
/// The runner only warns about an undeclared input, and still hands it to the
/// container as `INPUT_<NAME>`. Left alone, a v4 workflow's `excluded_members`
/// would be ignored and its packages would quietly return to the outputs.
fn reject_removed_inputs() -> Result<(), String> {
    let passed: Vec<String> = REMOVED_INPUTS
        .iter()
        .filter(|(name, _)| {
            env::var_os(format!("INPUT_{}", name.to_uppercase()))
                .is_some_and(|value| !value.to_string_lossy().trim().is_empty())
        })
        .map(|(name, advice)| format!("{name} ({advice})"))
        .collect();
    if passed.is_empty() {
        return Ok(());
    }
    Err(format!(
        "inputs removed in v5 were passed: {}. See \"Migrating from v4\" in the README.",
        passed.join("; ")
    ))
}

fn run() -> Result<(), String> {
    reject_removed_inputs()?;
    let trust_cargo_config = flag("TRUST_CARGO_CONFIG")?;
    let force_triggers = input("FORCE_TRIGGERS")?;
    let excluded: HashSet<String> = input("EXCLUDED_PACKAGES")?.into_iter().collect();
    // Last, so that its empty-list warning never precedes another input's error.
    let changed_files = changed_files()?;
    let graph = load_graph(trust_cargo_config)?;
    let result = compute_affected(&graph, &changed_files, &force_triggers, &excluded)
        .map_err(|error| error.to_string())?;
    emit_warnings(&result.warnings)?;
    emit_output(&result)
}

fn run_cargo(command: &mut Command, operation: &str) -> Result<Vec<u8>, String> {
    // Metadata can otherwise rewrite target/.rustc_info.json in the checkout.
    command
        .env("CARGO_CACHE_RUSTC_INFO", "0")
        .env("CARGO_TERM_COLOR", "never")
        .env("RUSTUP_TERM_COLOR", "never");
    let output = command
        .output()
        .map_err(|error| format!("failed to start cargo {operation}: {error}"))?;
    // Cargo diagnostics can contain workflow commands; emit only an escaped failure summary.
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr);
        let detail = message
            .lines()
            .find(|line| line.trim_start().starts_with("error"))
            .or_else(|| message.lines().find(|line| !line.trim().is_empty()))
            .map_or_else(|| output.status.to_string(), str::to_owned);
        return Err(format!("cargo {operation} failed: {detail}"));
    }
    Ok(output.stdout)
}

/// The nearest manifest, as Cargo would find it, and the filesystem root.
///
/// Cargo and rustup read configuration from the working directory upwards and
/// never from `--manifest-path`. Started at the filesystem root, they cannot see
/// a checkout's `.cargo/config.toml` or `rust-toolchain.toml`, which could
/// otherwise make them run arbitrary programs.
fn outside_checkout() -> Result<(PathBuf, PathBuf), String> {
    let cwd = env::current_dir()
        .map_err(|error| format!("failed to read the working directory: {error}"))?;
    let manifest = cwd
        .ancestors()
        .map(|directory| directory.join("Cargo.toml"))
        .find(|manifest| manifest.is_file())
        .ok_or("could not find Cargo.toml in the working directory or any parent directory")?;
    let root = cwd.ancestors().last().unwrap_or(&cwd).to_path_buf();
    Ok((manifest, root))
}

/// Homes the job's environment can relocate, and what one inside the checkout
/// would let a pull request do.
const HOMES: [(&str, &str); 2] = [
    (
        "CARGO_HOME",
        "Cargo would read configuration from the repository",
    ),
    (
        "RUSTUP_HOME",
        "rustup would run a toolchain from the repository",
    ),
];

/// Refuse a `CARGO_HOME` or `RUSTUP_HOME` inside the checkout.
///
/// Cargo reads `$CARGO_HOME/config.toml` wherever it starts, and rustup runs
/// `cargo` and `rustc` from `$RUSTUP_HOME/toolchains`. Starting them at the
/// filesystem root does not help when the job keeps either home in the
/// checkout: its contents are then the pull request's. The checkout is
/// `GITHUB_WORKSPACE`, or failing that the outermost directory above the
/// working directory that holds a manifest.
fn reject_homes_in_checkout(neutral: &Path) -> Result<(), String> {
    let cwd = env::current_dir()
        .map_err(|error| format!("failed to read the working directory: {error}"))?;
    let outermost = cwd
        .ancestors()
        .filter(|directory| directory.join("Cargo.toml").is_file())
        .last()
        .map(Path::to_path_buf);
    let workspace = env::var_os("GITHUB_WORKSPACE")
        .filter(|workspace| !workspace.is_empty())
        .map(PathBuf::from);
    let checkouts: Vec<PathBuf> = [workspace, outermost]
        .into_iter()
        .flatten()
        .map(|checkout| resolved(&checkout))
        .collect();
    for (name, consequence) in HOMES {
        let Some(home) = env::var_os(name).filter(|home| !home.is_empty()) else {
            continue;
        };
        // Cargo and rustup resolve a relative home against the directory they start in.
        let home = resolved(&neutral.join(home));
        if checkouts.iter().any(|checkout| home.starts_with(checkout)) {
            return Err(format!(
                "{name} ({}) is inside the checkout, so {consequence} although trust_cargo_config is false. Point {name} outside the checkout, or set trust_cargo_config: true if this checkout is trusted.",
                home.display()
            ));
        }
    }
    Ok(())
}

/// The path with symbolic links resolved as far as it exists; the rest is kept as written.
fn resolved(path: &Path) -> PathBuf {
    let path = path_clean::clean(path);
    for existing in path.ancestors() {
        if let Ok(real) = existing.canonicalize() {
            return match path.strip_prefix(existing) {
                Ok(rest) if !rest.as_os_str().is_empty() => real.join(rest),
                _ => real,
            };
        }
    }
    path
}

/// The absolute workspace manifest, as `cargo locate-project` reports it.
fn workspace_manifest(locate: &mut Command) -> Result<PathBuf, String> {
    let manifest = String::from_utf8(run_cargo(locate, "locate-project")?)
        .map_err(|_| "cargo locate-project output is not valid UTF-8".to_string())?;
    let manifest = Path::new(manifest.trim_end_matches(['\r', '\n']));
    if !manifest.is_absolute() {
        return Err("cargo locate-project returned no absolute workspace manifest path".into());
    }
    Ok(manifest.to_path_buf())
}

/// Resolve with `--locked` when the workspace has a lock, and warn when it has none.
/// Returns whether a lock constrains the resolution.
fn respect_lock(metadata: &mut MetadataCommand, manifest: &Path) -> Result<bool, String> {
    let root = manifest
        .parent()
        .ok_or("cargo locate-project returned an invalid workspace manifest path")?;
    if root
        .join("Cargo.lock")
        .try_exists()
        .map_err(|error| format!("failed to inspect workspace Cargo.lock: {error}"))?
    {
        metadata.other_options(["--locked"]);
        return Ok(true);
    }
    annotation(
        "warning",
        "No Cargo.lock in the workspace root; cargo metadata will resolve dependencies and may create one (owned by root inside the action). Commit Cargo.lock to use --locked.",
    )?;
    Ok(false)
}

/// The configuration file Cargo would have read from the checkout, were it trusted:
/// the nearest one between the working directory and the workspace root.
fn unread_config(workspace_root: &Path) -> Option<PathBuf> {
    let cwd = env::current_dir().ok()?;
    cwd.ancestors()
        .take_while(|directory| directory.starts_with(workspace_root))
        .flat_map(|directory| {
            ["config.toml", "config"].map(|name| directory.join(".cargo").join(name))
        })
        .find(|config| config.is_file())
}

/// With a lock, leaving a `[patch]` or source replacement unread makes Cargo
/// fail. Without one it resolves a different graph and says nothing, so say it.
fn warn_about_unread_config(config: Option<&Path>, locked: bool) -> Result<(), String> {
    match config {
        Some(config) if !locked => annotation(
            "warning",
            &format!(
                "{} is not read because trust_cargo_config is false, and without a Cargo.lock a [patch] or source replacement defined there is silently not applied. Set trust_cargo_config: true if this checkout is trusted.",
                config.display()
            ),
        ),
        _ => Ok(()),
    }
}

/// Point a Cargo failure at the configuration it was not allowed to read.
fn blame_unread_config(error: String, config: Option<&Path>) -> String {
    match config {
        Some(config) => format!(
            "{error}. {} was not read because trust_cargo_config is false; set trust_cargo_config: true if this checkout is trusted and Cargo needs that configuration.",
            config.display()
        ),
        None => error,
    }
}

/// The JSON document printed by `cargo metadata`.
fn resolved_metadata(metadata: &MetadataCommand) -> Result<String, String> {
    let stdout = String::from_utf8(run_cargo(&mut metadata.cargo_command(), "metadata")?)
        .map_err(|_| "cargo metadata output is not valid UTF-8".to_string())?;
    let json = stdout
        .lines()
        .find(|line| line.starts_with('{'))
        .ok_or("cargo metadata returned no JSON")?;
    Ok(json.to_owned())
}

fn load_graph(trust_cargo_config: bool) -> Result<WorkspaceGraph, String> {
    let mut metadata = MetadataCommand::new();
    // Discover the lock next to the workspace manifest, even when run in a member.
    let mut locate = Command::new(metadata.cargo_command().get_program());
    locate.args(["locate-project", "--workspace", "--message-format", "plain"]);
    // A checkout's configuration can make Cargo run any program, so Cargo only
    // starts where it can read that configuration when told the checkout is trusted.
    let neutral = if trust_cargo_config {
        None
    } else {
        let (manifest, root) = outside_checkout()?;
        reject_homes_in_checkout(&root)?;
        locate
            .arg("--manifest-path")
            .arg(manifest)
            .current_dir(&root);
        Some(root)
    };
    let manifest = workspace_manifest(&mut locate)?;
    let unread = if let Some(neutral) = neutral {
        metadata.manifest_path(&manifest).current_dir(neutral);
        manifest.parent().and_then(unread_config)
    } else {
        None
    };
    let locked = respect_lock(&mut metadata, &manifest)?;
    warn_about_unread_config(unread.as_deref(), locked)?;
    let json = resolved_metadata(&metadata)
        .map_err(|error| blame_unread_config(error, unread.as_deref()))?;
    WorkspaceGraph::from_json(&json).map_err(|error| error.to_string())
}

/// Entries of each kind of warning, in the order the kinds are reported.
fn group_warnings(warnings: &[Warning]) -> [Vec<&str>; 4] {
    let mut groups: [Vec<&str>; 4] = Default::default();
    for warning in warnings {
        let (group, entry) = match warning {
            Warning::UnusablePath(path) => (0, path),
            Warning::UnmatchedExclusion(entry) => (1, entry),
            Warning::ExternalTarget(member) => (2, member),
            Warning::UncoveredRootFile(file) => (3, file),
        };
        groups[group].push(entry.as_str());
    }
    groups
}

/// One annotation for a kind of warning, or none when it has no entries.
fn warn_about(entries: &[&str], message: impl FnOnce() -> String) -> Result<(), String> {
    if entries.is_empty() {
        return Ok(());
    }
    annotation("warning", &message())
}

fn emit_warnings(warnings: &[Warning]) -> Result<(), String> {
    let [paths, exclusions, external_targets, uncovered] = group_warnings(warnings);
    warn_about(&paths, || {
        format!(
            "{} changed file(s) were ignored for attribution and force triggers: {paths:?}. Use paths relative to the workspace root or absolute paths beneath it; empty paths and '..' are not supported.",
            paths.len()
        )
    })?;
    warn_about(&exclusions, || {
        format!(
            "excluded_packages entries matched no workspace package: {exclusions:?}. Names match package names exactly; use a trailing slash such as 'tools/' to match a directory."
        )
    })?;
    warn_about(&external_targets, || {
        format!(
            "workspace members declare targets outside their package directories: {external_targets:?}. Changes to those sources may not affect the declaring members. Configure force_triggers for shared source directories within the workspace root; sources outside that root require workflow-level invalidation."
        )
    })?;
    warn_about(&uncovered, || {
        format!(
            "workspace-wide files changed but belong to no package and matched no force trigger, so nothing was selected for them: {uncovered:?}. They can affect every package; add them to force_triggers."
        )
    })
}

fn open_destination(name: &str) -> Result<Option<File>, String> {
    env::var_os(name)
        .map(|path| {
            std::fs::OpenOptions::new()
                .append(true)
                .open(path)
                .map_err(|error| format!("failed to open {name}: {error}"))
        })
        .transpose()
}

fn write_destination(writer: &mut impl Write, contents: &str, name: &str) -> Result<(), String> {
    writer
        .write_all(contents.as_bytes())
        .map_err(|error| format!("failed to write {name}: {error}"))
}

/// The action's historical output order; any other field follows alphabetically.
fn field_rank(key: &str) -> u8 {
    match key {
        "changed_packages" => 0,
        "affected_libraries" => 1,
        "affected_binaries" => 2,
        "force_all" => 3,
        _ => 4,
    }
}

/// The `key=value` lines for `GITHUB_OUTPUT` and the single-line JSON document.
fn render_outputs(result: &AffectedResult) -> Result<(String, String), String> {
    let value = serde_json::to_value(result).map_err(|error| error.to_string())?;
    let fields = value
        .as_object()
        .ok_or("affected result must serialize as an object")?;
    let mut fields: Vec<_> = fields.iter().collect();
    fields.sort_by_key(|(key, _)| (field_rank(key), key.as_str()));
    let mut lines = String::new();
    for (key, value) in fields {
        lines.push_str(key);
        lines.push('=');
        lines.push_str(&value.to_string());
        lines.push('\n');
    }
    // A JSON Value retains the existing alphabetic object-key order.
    Ok((lines, format!("{value}\n")))
}

fn append(file: &mut Option<File>, contents: &str, name: &str) -> Result<(), String> {
    match file {
        Some(file) => write_destination(file, contents, name),
        None => Ok(()),
    }
}

fn emit_output(result: &AffectedResult) -> Result<(), String> {
    let (lines, json) = render_outputs(result)?;
    // Opening both first avoids publishing results when a destination is invalid.
    let mut output_file = open_destination("GITHUB_OUTPUT")?;
    let mut summary_file = open_destination("GITHUB_STEP_SUMMARY")?;
    append(&mut output_file, &lines, "GITHUB_OUTPUT")?;
    append(
        &mut summary_file,
        &render_summary(result),
        "GITHUB_STEP_SUMMARY",
    )?;
    let log = if output_file.is_some() { &lines } else { &json };
    write_destination(&mut io::stdout().lock(), log, "stdout")
}

fn render_summary(result: &AffectedResult) -> String {
    let mut summary = String::new();
    if result.force_all {
        summary.push_str("> [!WARNING]\n> **Force all** — a global trigger file changed, entire workspace affected.\n\n");
    }
    summary.push_str("## rust-affected\n\n| | Crates |\n|---|---|\n");
    let rows = [
        ("Changed", "Changed packages", &result.changed_packages),
        (
            "Affected libraries",
            "Affected libraries",
            &result.affected_libraries,
        ),
        (
            "Affected binaries",
            "Affected binaries",
            &result.affected_binaries,
        ),
    ];
    for (label, _, names) in rows {
        let names = names
            .iter()
            .map(|name| format!("`{name}`"))
            .collect::<Vec<_>>()
            .join(" ");
        summary.push_str("| **");
        summary.push_str(label);
        summary.push_str("** | ");
        summary.push_str(&names);
        summary.push_str(" |\n");
    }
    for (_, heading, names) in rows {
        let names = if names.is_empty() {
            "_none_".to_string()
        } else {
            names
                .iter()
                .map(|name| format!("- `{name}`"))
                .collect::<Vec<_>>()
                .join("\n")
        };
        summary.push_str("\n### ");
        summary.push_str(heading);
        summary.push('\n');
        summary.push_str(&names);
        summary.push('\n');
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annotations_escape_command_data() {
        assert_eq!(
            format_annotation("error", "100%\r\n::warning::fake"),
            "::error::100%25%0D%0A::warning::fake"
        );
    }

    #[test]
    fn output_write_errors_keep_the_destination_context() {
        struct FailedWriter;
        impl Write for FailedWriter {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("disk full"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        for destination in ["GITHUB_OUTPUT", "GITHUB_STEP_SUMMARY", "stdout"] {
            let error = write_destination(&mut FailedWriter, "results", destination).unwrap_err();
            assert!(error.contains(destination));
            assert!(error.contains("disk full"));
        }
    }
}
