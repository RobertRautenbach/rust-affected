use guppy::{MetadataCommand, graph::PackageGraph};
use rust_affected::{AffectedResult, Warning, compute_affected};
use std::{
    env,
    fs::File,
    io::{self, Write},
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
    let excluded = input("EXCLUDED_MEMBERS")?.into_iter().collect();
    if changed_files.is_empty() {
        annotation(
            "warning",
            "changed_files is empty; no files will be attributed. If files did change, check the step producing changed_files. Workspace and configuration validation still run.",
        )?;
    }
    let mut command = MetadataCommand::new();
    // Metadata must not rewrite a consumer's compiler-version cache in target/.
    command.env("CARGO_CACHE_RUSTC_INFO", "0");
    let graph = PackageGraph::from_command(&mut command)
        .map_err(|error| format!("cargo metadata failed: {}", error_chain(&error)))?;
    let result = compute_affected(&graph, &changed_files, &force_triggers, &excluded)
        .map_err(|error| error.to_string())?;
    emit_warnings(&result.warnings)?;
    emit_output(&result)
}

fn error_chain(error: &dyn std::error::Error) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(error) = source {
        message.push_str(&format!(": {error}"));
        source = error.source();
    }
    message
}

fn emit_warnings(warnings: &[Warning]) -> Result<(), String> {
    let mut paths = Vec::new();
    let mut exclusions = Vec::new();
    for warning in warnings {
        match warning {
            Warning::UnusablePath(path) => paths.push(path.as_str()),
            Warning::UnmatchedExclusion(entry) => exclusions.push(entry.as_str()),
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
    let changed = serde_json::to_string(&result.changed_crates).map_err(|e| e.to_string())?;
    let libraries =
        serde_json::to_string(&result.affected_library_members).map_err(|e| e.to_string())?;
    let binaries =
        serde_json::to_string(&result.affected_binary_members).map_err(|e| e.to_string())?;
    let lines = format!(
        "changed_crates={changed}\naffected_library_members={libraries}\naffected_binary_members={binaries}\nforce_all={}\n",
        result.force_all
    );
    let json = format!(
        "{}\n",
        serde_json::json!({
            "changed_crates": result.changed_crates,
            "affected_library_members": result.affected_library_members,
            "affected_binary_members": result.affected_binary_members,
            "force_all": result.force_all,
        })
    );
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
    if output_file.is_none() {
        write_destination(&mut io::stdout().lock(), &json, "stdout")?;
    }
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
        summary.push_str(&format!("| **{label}** | {names} |\n"));
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
        summary.push_str(&format!("\n### {heading}\n{names}\n"));
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
