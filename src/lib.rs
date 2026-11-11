use globset::{Glob, GlobSetBuilder};
mod graph;
pub use graph::{DependencyKind, WorkspaceGraph};
use serde::Serialize;
use std::collections::HashSet;
use std::fmt;
use std::hash::BuildHasher;
use std::path::{Component, Path, PathBuf};

/// A failure while validating triggers or querying the dependency graph.
#[derive(Debug)]
pub enum Error {
    InvalidForceTrigger(globset::Error),
    /// A pattern that no workspace-relative path can match.
    UnmatchableForceTrigger(String),
    Graph(guppy::Error),
    InvalidMetadata(serde_json::Error),
    IncompleteMetadata(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidForceTrigger(error) => {
                write!(f, "invalid force_triggers pattern: {error}")
            }
            Self::UnmatchableForceTrigger(trigger) => write!(
                f,
                "force_triggers pattern {trigger:?} can never match: patterns are relative to the workspace root, so empty patterns, absolute paths and '..' are not supported (use '**' to match every file)"
            ),
            Self::Graph(error) => write!(f, "failed to build or query package graph: {error}"),
            Self::InvalidMetadata(error) => write!(f, "invalid cargo metadata: {error}"),
            Self::IncompleteMetadata(error) => write!(f, "failed to build package graph: {error}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(match self {
            Self::InvalidForceTrigger(error) => error,
            Self::Graph(error) => error,
            Self::InvalidMetadata(error) => error,
            Self::UnmatchableForceTrigger(_) | Self::IncompleteMetadata(_) => return None,
        })
    }
}

/// Non-fatal input problems, sorted and deduplicated in the result.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Warning {
    UnusablePath(String),
    UnmatchedExclusion(String),
    /// A workspace member declares a target outside its package directory.
    ExternalTarget(String),
    /// A workspace-wide Cargo file changed, but no package owns it and no force
    /// trigger matched, so the change selected nothing.
    UncoveredRootFile(String),
}

/// Root files that can change how every package builds.
const WORKSPACE_WIDE_FILES: [&str; 6] = [
    "Cargo.toml",
    "Cargo.lock",
    ".cargo/config.toml",
    ".cargo/config",
    "rust-toolchain.toml",
    "rust-toolchain",
];

/// Sorted workspace-member names and diagnostics from affected-file analysis.
/// Serialization includes the four action outputs; warnings remain diagnostics.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct AffectedResult {
    #[serde(skip)]
    pub warnings: Vec<Warning>,
    pub force_all: bool,
    pub changed_crates: Vec<String>,
    pub affected_library_members: Vec<String>,
    pub affected_binary_members: Vec<String>,
}

// Lexical normalization preserves deleted paths and does not resolve symlinks.
fn normalize_relative_path(path: &Path) -> Option<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    (!normalized.as_os_str().is_empty()).then_some(normalized)
}

// Git C-quotes names with unusual bytes, such as "caf\303\251.rs", unless
// core.quotePath is off or -z is used. Left quoted, they would own nothing.
fn unquote_git_path(quoted: &str) -> Option<String> {
    let inner = quoted.strip_prefix('"')?.strip_suffix('"')?;
    let mut bytes = Vec::with_capacity(inner.len());
    let mut rest = inner.bytes();
    while let Some(byte) = rest.next() {
        if byte == b'"' {
            return None;
        }
        if byte != b'\\' {
            bytes.push(byte);
            continue;
        }
        bytes.push(match rest.next()? {
            b'a' => 0x07,
            b'b' => 0x08,
            b't' => b'\t',
            b'n' => b'\n',
            b'v' => 0x0b,
            b'f' => 0x0c,
            b'r' => b'\r',
            escaped @ (b'"' | b'\\') => escaped,
            // Git always writes three octal digits, and a byte needs no more.
            high @ b'0'..=b'3' => {
                let mut value = high - b'0';
                for _ in 0..2 {
                    let digit = rest.next().filter(|digit| (b'0'..=b'7').contains(digit))?;
                    value = value * 8 + (digit - b'0');
                }
                value
            }
            _ => return None,
        });
    }
    String::from_utf8(bytes).ok()
}

fn normalize_changed_file(file: &str, root: &Path) -> Option<PathBuf> {
    // A quote that cannot be decoded (a name split on whitespace) is unusable.
    let unquoted;
    let file = if file.starts_with('"') {
        unquoted = unquote_git_path(file)?;
        unquoted.as_str()
    } else {
        file
    };
    let path = Path::new(file);
    let relative = if path.is_absolute() {
        path.strip_prefix(root).ok()?
    } else {
        path
    };
    normalize_relative_path(relative)
}

fn exclusion_matches(entry: &str, name: &str, directory: &Path) -> bool {
    // Classify before normalization: "./foo" is a directory, not a crate name.
    if entry.contains('/') {
        normalize_relative_path(Path::new(entry))
            .is_some_and(|prefix| directory.starts_with(prefix))
    } else {
        entry == name
    }
}

/// Match normalized workspace-relative paths against force-trigger globs.
///
/// Patterns are normalized like paths: `./` and repeated separators are dropped.
///
/// # Errors
/// Returns an error for invalid patterns and for patterns no relative path can
/// match (empty, absolute or containing `..`), even when there are no files.
pub fn check_force_triggers(
    changed_files: &[String],
    force_triggers: &[String],
) -> Result<bool, Error> {
    if force_triggers.is_empty() {
        return Ok(false);
    }

    let mut builder = GlobSetBuilder::new();
    for trigger in force_triggers {
        // Changed files are compared in normalized form, so patterns must be too:
        // "infra//" would otherwise never fire, and nothing would say so.
        let unmatchable = || Error::UnmatchableForceTrigger(trigger.clone());
        let mut parts = Vec::new();
        for part in trigger.split('/') {
            match part {
                "" | "." => {}
                ".." => return Err(unmatchable()),
                _ => parts.push(part),
            }
        }
        if trigger.starts_with('/') || parts.is_empty() {
            return Err(unmatchable());
        }
        let pattern = parts.join("/");
        if trigger.ends_with('/') {
            builder.add(Glob::new(&format!("{pattern}/**")).map_err(Error::InvalidForceTrigger)?);
        }
        builder.add(Glob::new(&pattern).map_err(Error::InvalidForceTrigger)?);
    }
    let globset = builder.build().map_err(Error::InvalidForceTrigger)?;
    Ok(changed_files.iter().any(|f| globset.is_match(f)))
}

/// The packages owning `files`, warning about workspace-wide files nobody covers.
///
/// Every package Cargo loaded from beneath the root can own files, not only
/// members: an excluded path dependency, a `[patch]` target or a vendored crate
/// still reaches members through the resolved graph. Only members are reported.
fn direct_owners(
    graph: &guppy::graph::PackageGraph,
    files: &[PathBuf],
    force_all: bool,
    warnings: &mut Vec<Warning>,
) -> HashSet<guppy::PackageId> {
    let root = graph.workspace().root();
    let owners: Vec<_> = graph
        .packages()
        .filter_map(|pkg| {
            let dir = pkg.manifest_path().parent()?.strip_prefix(root);
            Some((dir.ok()?, pkg))
        })
        .collect();
    let mut direct_ids = HashSet::new();
    for file in files {
        if let Some((_, pkg)) = owners
            .iter()
            .filter(|(dir, _)| file.starts_with(dir.as_std_path()))
            .max_by_key(|(dir, _)| dir.components().count())
        {
            direct_ids.insert(pkg.id().clone());
        } else if !force_all
            && WORKSPACE_WIDE_FILES
                .iter()
                .any(|name| file == Path::new(name))
        {
            warnings.push(Warning::UncoveredRootFile(
                file.to_string_lossy().into_owned(),
            ));
        }
    }
    direct_ids
}

/// Compute which workspace crates are affected by a set of changed files.
///
/// `affected_library_members` and `affected_binary_members` are mutually
/// exclusive: a crate with a binary target appears only in `affected_binary_members`,
/// while pure library crates appear only in `affected_library_members`.
///
/// `excluded` filters crate names from all three output lists (`changed_crates`,
/// `affected_library_members`, `affected_binary_members`) but does **not** prune the
/// dependency graph traversal. An excluded crate is still traversed when resolving
/// transitive dependents — it simply won't appear in the results.
///
/// Exclusion entries that contain `/` are matched as path prefixes against each
/// package's directory relative to the workspace root (e.g. `tools/` excludes
/// every crate under `tools/`). Entries without `/` are matched against the crate
/// name directly. A prefix includes nested members. Each accepted path is owned
/// by the deepest package containing it, before exclusions are applied. Owners
/// include non-member packages beneath the workspace root (excluded path
/// dependencies, `[patch]` targets, vendored crates); these are never reported
/// themselves, but the members that depend on them are.
///
/// Paths are normalized lexically, so deleted files work. Git-quoted paths are
/// decoded first. Unusable paths are warned about and ignored, including for
/// force triggers.
///
/// # Errors
/// Returns an error for invalid force-trigger patterns or failed graph queries.
pub fn compute_affected<S: BuildHasher>(
    graph: &WorkspaceGraph,
    changed_files: &[String],
    force_triggers: &[String],
    excluded: &HashSet<String, S>,
) -> Result<AffectedResult, Error> {
    let graph = &graph.inner;
    let workspace = graph.workspace();
    let mut warnings = Vec::new();
    let files: Vec<PathBuf> = changed_files
        .iter()
        .filter_map(|file| {
            let normalized = normalize_changed_file(file, workspace.root().as_std_path());
            if normalized.is_none() {
                warnings.push(Warning::UnusablePath(file.clone()));
            }
            normalized
        })
        .collect();
    // Input strings and workspace paths are UTF-8; normalization only removes components.
    let normalized_files: Vec<String> = files
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    let force_all = check_force_triggers(&normalized_files, force_triggers)?;
    let members: Vec<_> = workspace.iter_by_path().collect();
    for (dir, pkg) in &members {
        let package_dir = path_clean::clean(workspace.root().join(dir));
        if pkg
            .build_targets()
            .any(|target| !path_clean::clean(target.path()).starts_with(&package_dir))
        {
            warnings.push(Warning::ExternalTarget(pkg.name().to_string()));
        }
    }
    let mut excluded_ids = HashSet::new();
    for entry in excluded {
        let mut matched = false;
        for (dir, pkg) in &members {
            if exclusion_matches(entry, pkg.name(), dir.as_std_path()) {
                matched = true;
                excluded_ids.insert(pkg.id().clone());
            }
        }
        if !matched {
            warnings.push(Warning::UnmatchedExclusion(entry.clone()));
        }
    }

    let direct_ids = direct_owners(graph, &files, force_all, &mut warnings);
    warnings.sort();
    warnings.dedup();

    let affected_set = if force_all {
        graph.query_workspace().resolve()
    } else {
        graph
            .query_reverse(direct_ids.iter())
            .map_err(Error::Graph)?
            .resolve()
    };

    let mut result = AffectedResult {
        warnings,
        force_all,
        ..AffectedResult::default()
    };
    for pkg in affected_set.packages(guppy::graph::DependencyDirection::Forward) {
        if !pkg.in_workspace() || excluded_ids.contains(pkg.id()) {
            continue;
        }
        if direct_ids.contains(pkg.id()) {
            result.changed_crates.push(pkg.name().to_string());
        }
        let members = if pkg
            .build_targets()
            .any(|target| matches!(target.id(), guppy::graph::BuildTargetId::Binary(_)))
        {
            &mut result.affected_binary_members
        } else {
            &mut result.affected_library_members
        };
        members.push(pkg.name().to_string());
    }
    result.changed_crates.sort();
    result.affected_library_members.sort();
    result.affected_binary_members.sort();
    Ok(result)
}
