use guppy::MetadataCommand;
use rust_affected::{AffectedResult, Warning, WorkspaceGraph, compute_affected};
use std::{
    collections::HashSet,
    env,
    fs::File,
    io::{self, Write},
    path::Path,
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

fn input(name: &str) -> Result<Vec<String>, String> {
    let value = env::var_os(name)
        .unwrap_or_default()
        .into_string()
        .map_err(|_| format!("{name} is not valid UTF-8"))?;
    Ok(value.split_whitespace().map(String::from).collect())
}

fn run() -> Result<(), String> {
    let changed_files = input("CHANGED_FILES")?;
    let force_triggers = input("FORCE_TRIGGERS")?;
    let excluded: HashSet<String> = input("EXCLUDED_MEMBERS")?.into_iter().collect();
    if changed_files.is_empty() {
        annotation(
            "warning",
            "changed_files is empty; no files will be attributed. If files did change, check the step producing changed_files. Workspace and configuration validation still run.",
        )?;
    }
    let graph = load_graph()?;
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

fn load_graph() -> Result<WorkspaceGraph, String> {
    let mut metadata = MetadataCommand::new();
    // Discover the lock next to the workspace manifest, even when run in a member.
    let mut locate = Command::new(metadata.cargo_command().get_program());
    locate.args(["locate-project", "--workspace", "--message-format", "plain"]);
    let manifest = String::from_utf8(run_cargo(&mut locate, "locate-project")?)
        .map_err(|_| "cargo locate-project output is not valid UTF-8".to_string())?;
    let manifest = Path::new(manifest.trim_end_matches(['\r', '\n']));
    if !manifest.is_absolute() {
        return Err("cargo locate-project returned no absolute workspace manifest path".into());
    }
    let root = manifest
        .parent()
        .ok_or("cargo locate-project returned an invalid workspace manifest path")?;
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

fn emit_warnings(warnings: &[Warning]) -> Result<(), String> {
    let mut paths = Vec::new();
    let mut exclusions = Vec::new();
    let mut external_targets = Vec::new();
    for warning in warnings {
        match warning {
            Warning::UnusablePath(path) => paths.push(path.as_str()),
            Warning::UnmatchedExclusion(entry) => exclusions.push(entry.as_str()),
            Warning::ExternalTarget(member) => external_targets.push(member.as_str()),
        }
    }
    if !paths.is_empty() {
        annotation(
            "warning",
            &format!(
                "{} changed file(s) were ignored for attribution and force triggers: {paths:?}. Use paths relative to the workspace root or absolute paths beneath it; empty paths and '..' are not supported.",
                paths.len()
            ),
        )?;
    }
    if !exclusions.is_empty() {
        annotation(
            "warning",
            &format!(
                "excluded_members entries matched no workspace member: {exclusions:?}. Names match crate names exactly; use a trailing slash such as 'tools/' to match a directory."
            ),
        )?;
    }
    if !external_targets.is_empty() {
        annotation(
            "warning",
            &format!(
                "workspace members declare targets outside their package directories: {external_targets:?}. Changes to those sources may not affect the declaring members. Configure force_triggers for shared source directories within the workspace root; sources outside that root require workflow-level invalidation."
            ),
        )?;
    }
    Ok(())
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
