use globset::{Glob, GlobSetBuilder};
use guppy::graph::PackageGraph;
use serde::Serialize;
use std::collections::HashSet;
use std::fmt;
use std::hash::BuildHasher;
use std::path::{Component, Path, PathBuf};

/// A failure while validating triggers or querying the dependency graph.
#[derive(Debug)]
pub enum Error {
    InvalidForceTrigger(globset::Error),
    Graph(guppy::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidForceTrigger(error) => {
                write!(f, "invalid force_triggers pattern: {error}")
            }
            Self::Graph(error) => write!(f, "failed to query package graph: {error}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(match self {
            Self::InvalidForceTrigger(error) => error,
            Self::Graph(error) => error,
        })
    }
}

/// Non-fatal input problems, sorted and deduplicated in the result.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Warning {
    UnusablePath(String),
    UnmatchedExclusion(String),
}

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

fn normalize_changed_file(file: &str, root: &Path) -> Option<PathBuf> {
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
/// # Errors
/// Returns an error for invalid patterns, even when there are no files.
pub fn check_force_triggers(
    changed_files: &[String],
    force_triggers: &[String],
) -> Result<bool, Error> {
    if force_triggers.is_empty() {
        return Ok(false);
    }

    let mut builder = GlobSetBuilder::new();
    for trigger in force_triggers {
        let trigger = trigger.trim_start_matches("./");
        let pattern = if trigger.ends_with('/') {
            format!("{trigger}**")
        } else {
            trigger.to_string()
        };
        builder.add(Glob::new(&pattern).map_err(Error::InvalidForceTrigger)?);
    }
    let globset = builder.build().map_err(Error::InvalidForceTrigger)?;
    Ok(changed_files.iter().any(|f| globset.is_match(f)))
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
/// by its deepest matching member, before exclusions are applied.
///
/// Paths are normalized lexically, so deleted files work. Unusable paths are
/// warned about and ignored, including for force triggers.
///
/// # Errors
/// Returns an error for invalid force-trigger patterns or failed graph queries.
pub fn compute_affected<S: BuildHasher>(
    graph: &PackageGraph,
    changed_files: &[String],
    force_triggers: &[String],
    excluded: &HashSet<String, S>,
) -> Result<AffectedResult, Error> {
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
    warnings.sort();
    warnings.dedup();

    let mut direct_ids = HashSet::new();
    for file in &files {
        if let Some((_, pkg)) = members
            .iter()
            .filter(|(dir, _)| file.starts_with(dir.as_std_path()))
            .max_by_key(|(dir, _)| dir.components().count())
        {
            direct_ids.insert(pkg.id().clone());
        }
    }

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
