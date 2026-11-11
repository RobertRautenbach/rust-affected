use guppy::MetadataCommand;
use rust_affected::{AffectedResult, Warning, WorkspaceGraph, compute_affected};
use std::{
    collections::HashSet,
    env,
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

fn text(name: &str) -> Result<String, String> {
    env::var_os(name)
        .unwrap_or_default()
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
    match env::var_os(name).unwrap_or_default().to_str() {
        Some("" | "false") => Ok(false),
        Some("true") => Ok(true),
        _ => Err(format!("{name} must be 'true' or 'false'")),
    }
}

fn run() -> Result<(), String> {
    let ignore_cargo_config = flag("IGNORE_CARGO_CONFIG")?;
    let force_triggers = input("FORCE_TRIGGERS")?;
    let excluded: HashSet<String> = input("EXCLUDED_MEMBERS")?.into_iter().collect();
    // Last, so that its empty-list warning never precedes another input's error.
    let changed_files = changed_files()?;
    let graph = load_graph(ignore_cargo_config)?;
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

fn load_graph(ignore_cargo_config: bool) -> Result<WorkspaceGraph, String> {
    let mut metadata = MetadataCommand::new();
    // Discover the lock next to the workspace manifest, even when run in a member.
    let mut locate = Command::new(metadata.cargo_command().get_program());
    locate.args(["locate-project", "--workspace", "--message-format", "plain"]);
    let neutral = if ignore_cargo_config {
        let (manifest, root) = outside_checkout()?;
        locate
            .arg("--manifest-path")
            .arg(manifest)
            .current_dir(&root);
        Some(root)
    } else {
        None
    };
    let manifest = String::from_utf8(run_cargo(&mut locate, "locate-project")?)
        .map_err(|_| "cargo locate-project output is not valid UTF-8".to_string())?;
    let manifest = Path::new(manifest.trim_end_matches(['\r', '\n']));
    if !manifest.is_absolute() {
        return Err("cargo locate-project returned no absolute workspace manifest path".into());
    }
    let root = manifest
        .parent()
        .ok_or("cargo locate-project returned an invalid workspace manifest path")?;
    if let Some(neutral) = neutral {
        metadata.manifest_path(manifest).current_dir(neutral);
    }
    if root
        .join("Cargo.lock")
        .try_exists()
        .map_err(|error| format!("failed to inspect workspace Cargo.lock: {error}"))?
    {
        metadata.other_options(["--locked"]);
    } else {
        annotation(
            "warning",
            "No Cargo.lock in the workspace root; cargo metadata will resolve dependencies and may create one (owned by root inside the action). Commit Cargo.lock to use --locked.",
        )?;
    }
    let stdout = String::from_utf8(run_cargo(&mut metadata.cargo_command(), "metadata")?)
        .map_err(|_| "cargo metadata output is not valid UTF-8".to_string())?;
    let json = stdout
        .lines()
        .find(|line| line.starts_with('{'))
        .ok_or("cargo metadata returned no JSON")?;
    WorkspaceGraph::from_json(json).map_err(|error| error.to_string())
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
            "excluded_members entries matched no workspace member: {exclusions:?}. Names match crate names exactly; use a trailing slash such as 'tools/' to match a directory."
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

fn emit_output(result: &AffectedResult) -> Result<(), String> {
    let value = serde_json::to_value(result).map_err(|error| error.to_string())?;
    let fields = value
        .as_object()
        .ok_or("affected result must serialize as an object")?;
    let mut fields: Vec<_> = fields.iter().collect();
    // Preserve the action's historical line order while emitting every serialized field.
    fields.sort_by_key(|(key, _)| {
        let order = match key.as_str() {
            "changed_crates" => 0,
            "affected_library_members" => 1,
            "affected_binary_members" => 2,
            "force_all" => 3,
            _ => 4,
        };
        (order, key.as_str())
    });
    let mut lines = String::new();
    for (key, value) in fields {
        lines.push_str(key);
        lines.push('=');
        lines.push_str(&value.to_string());
        lines.push('\n');
    }
    // A JSON Value retains the existing alphabetic object-key order.
    let json = format!("{value}\n");
    let summary = render_summary(result);
    // Opening both first avoids publishing results when a destination is invalid.
    let mut output_file = open_destination("GITHUB_OUTPUT")?;
    let mut summary_file = open_destination("GITHUB_STEP_SUMMARY")?;
    if let Some(file) = &mut output_file {
        write_destination(file, &lines, "GITHUB_OUTPUT")?;
    }
    if let Some(file) = &mut summary_file {
        write_destination(file, &summary, "GITHUB_STEP_SUMMARY")?;
    }
    let log = if output_file.is_some() { &lines } else { &json };
    write_destination(&mut io::stdout().lock(), log, "stdout")?;
    Ok(())
}

fn render_summary(result: &AffectedResult) -> String {
    let mut summary = String::new();
    if result.force_all {
        summary.push_str("> [!WARNING]\n> **Force all** — a global trigger file changed, entire workspace affected.\n\n");
    }
    summary.push_str("## rust-affected\n\n| | Crates |\n|---|---|\n");
    let rows = [
        ("Changed", "Changed crates", &result.changed_crates),
        (
            "Affected libraries",
            "Affected library members",
            &result.affected_library_members,
        ),
        (
            "Affected binaries",
            "Affected binary members",
            &result.affected_binary_members,
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
