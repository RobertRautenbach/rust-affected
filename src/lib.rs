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
    pub changed_packages: Vec<String>,
    pub affected_libraries: Vec<String>,
    pub affected_binaries: Vec<String>,
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

/// A force-trigger pattern in the normalized form changed files are compared in.
///
/// "infra//" would otherwise never fire, and nothing would say so. A pattern no
/// relative path can match is an error for the same reason.
fn normalize_trigger(trigger: &str) -> Result<String, Error> {
    let unmatchable = || Error::UnmatchableForceTrigger(trigger.to_owned());
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
    Ok(parts.join("/"))
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
        let pattern = normalize_trigger(trigger)?;
        if trigger.ends_with('/') {
            builder.add(Glob::new(&format!("{pattern}/**")).map_err(Error::InvalidForceTrigger)?);
        }
        builder.add(Glob::new(&pattern).map_err(Error::InvalidForceTrigger)?);
    }
    let globset = builder.build().map_err(Error::InvalidForceTrigger)?;
    Ok(changed_files.iter().any(|f| globset.is_match(f)))
}

/// The changed files that can be attributed, warning about the rest.
fn usable_files(
    changed_files: &[String],
    graph: &guppy::graph::PackageGraph,
    warnings: &mut Vec<Warning>,
) -> Vec<PathBuf> {
    let root = graph.workspace().root().as_std_path();
    changed_files
        .iter()
        .filter_map(|file| {
            let normalized = normalize_changed_file(file, root);
            if normalized.is_none() {
                warnings.push(Warning::UnusablePath(file.clone()));
            }
            normalized
        })
        .collect()
}

/// Warn about members whose targets live outside their package directory.
fn warn_about_external_targets(graph: &guppy::graph::PackageGraph, warnings: &mut Vec<Warning>) {
    let workspace = graph.workspace();
    for (dir, pkg) in workspace.iter_by_path() {
        let package_dir = path_clean::clean(workspace.root().join(dir));
        if pkg
            .build_targets()
            .any(|target| !path_clean::clean(target.path()).starts_with(&package_dir))
        {
            warnings.push(Warning::ExternalTarget(pkg.name().to_string()));
        }
    }
}

/// The members matched by the exclusions, warning about entries that match none.
fn excluded_packages<S: BuildHasher>(
    graph: &guppy::graph::PackageGraph,
    excluded: &HashSet<String, S>,
    warnings: &mut Vec<Warning>,
) -> HashSet<guppy::PackageId> {
    let mut excluded_ids = HashSet::new();
    for entry in excluded {
        // Collected per entry: an entry matching only already-excluded members still matched.
        let matched: Vec<_> = graph
            .workspace()
            .iter_by_path()
            .filter(|(dir, pkg)| exclusion_matches(entry, pkg.name(), dir.as_std_path()))
            .map(|(_, pkg)| pkg.id().clone())
            .collect();
        if matched.is_empty() {
            warnings.push(Warning::UnmatchedExclusion(entry.clone()));
        }
        excluded_ids.extend(matched);
    }
    excluded_ids
}

/// Add an affected member to the lists it belongs in.
fn report(pkg: &guppy::graph::PackageMetadata<'_>, changed: bool, result: &mut AffectedResult) {
    let name = pkg.name().to_string();
    if changed {
        result.changed_packages.push(name.clone());
    }
    if pkg
        .build_targets()
        .any(|target| matches!(target.id(), guppy::graph::BuildTargetId::Binary(_)))
    {
        result.affected_binaries.push(name);
    } else {
        result.affected_libraries.push(name);
    }
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
/// `affected_libraries` and `affected_binaries` are mutually
/// exclusive: a crate with a binary target appears only in `affected_binaries`,
/// while pure library crates appear only in `affected_libraries`.
///
/// `excluded` filters crate names from all three output lists (`changed_packages`,
/// `affected_libraries`, `affected_binaries`) but does **not** prune the
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
/// Paths are normalized lexically, so deleted files work, and are otherwise
/// taken literally: nothing is unquoted or split. Unusable paths are warned
/// about and ignored, including for force triggers.
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
    let mut warnings = Vec::new();
    let files = usable_files(changed_files, graph, &mut warnings);
    // Input strings and workspace paths are UTF-8; normalization only removes components.
    let normalized_files: Vec<String> = files
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    let force_all = check_force_triggers(&normalized_files, force_triggers)?;
    warn_about_external_targets(graph, &mut warnings);
    let excluded_ids = excluded_packages(graph, excluded, &mut warnings);
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
        if pkg.in_workspace() && !excluded_ids.contains(pkg.id()) {
            report(&pkg, direct_ids.contains(pkg.id()), &mut result);
        }
    }
    result.changed_packages.sort();
    result.affected_libraries.sort();
    result.affected_binaries.sort();
    Ok(result)
}
