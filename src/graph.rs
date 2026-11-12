use crate::Error;
use guppy::graph::PackageGraph;
use serde::Deserialize;
use std::{collections::HashSet, path::Path};

/// A declared dependency kind, retained for source compatibility.
/// Analysis follows all edges in Cargo's resolved graph.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum DependencyKind {
    #[default]
    Normal,
    Build,
    Dev,
    #[serde(other)]
    Other,
}

/// A validated, Cargo-resolved graph, including non-workspace packages.
#[derive(Debug)]
pub struct WorkspaceGraph {
    pub(crate) inner: PackageGraph,
}

impl WorkspaceGraph {
    /// Read Cargo's version 1 metadata with a complete resolved graph.
    /// Metadata must be produced with `--all-features` and without `--no-deps`
    /// or platform filtering. Declared paths are never used to infer edges.
    ///
    /// # Errors
    /// Returns an error for invalid JSON, unsupported metadata versions, missing
    /// resolution, inconsistent identities, or Guppy graph-construction errors.
    pub fn from_json(json: &str) -> Result<Self, Error> {
        let value: serde_json::Value =
            serde_json::from_str(json).map_err(Error::InvalidMetadata)?;
        check_contract(&value)?;
        let metadata: cargo_metadata::Metadata =
            serde_json::from_value(value).map_err(Error::InvalidMetadata)?;
        let resolve = metadata.resolve.as_ref().ok_or_else(|| {
            Error::IncompleteMetadata(
                "cargo metadata has no resolved graph; --no-deps is not supported".into(),
            )
        })?;
        check_identities(&metadata, resolve)?;
        check_dependencies(resolve)?;
        let inner = PackageGraph::from_json(json).map_err(Error::Graph)?;
        Ok(Self { inner })
    }

    /// The absolute workspace root reported by Cargo.
    #[must_use]
    pub fn root(&self) -> &Path {
        self.inner.workspace().root().as_std_path()
    }
}

/// `cargo_metadata` keeps the version private and defaults a missing `node.deps`
/// to empty. Hold the stricter input contract before delegating schema parsing.
fn check_contract(value: &serde_json::Value) -> Result<(), Error> {
    if value.get("version").and_then(serde_json::Value::as_u64) != Some(1) {
        return Err(Error::IncompleteMetadata(
            "unsupported or missing metadata version; expected 1".into(),
        ));
    }
    let node_without_deps =
        |node: &serde_json::Value| node.is_object() && node.get("deps").is_none();
    if value
        .pointer("/resolve/nodes")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|nodes| nodes.iter().any(node_without_deps))
    {
        return Err(Error::IncompleteMetadata(
            "resolved node is missing deps".into(),
        ));
    }
    Ok(())
}

/// Guppy permits unresolved metadata and collapses some duplicate identities.
/// Retain completeness checks; Guppy validates member and dependency references.
fn check_identities(
    metadata: &cargo_metadata::Metadata,
    resolve: &cargo_metadata::Resolve,
) -> Result<(), Error> {
    let packages: HashSet<_> = metadata.packages.iter().map(|p| &p.id).collect();
    let nodes: HashSet<_> = resolve.nodes.iter().map(|n| &n.id).collect();
    let members: HashSet<_> = metadata.workspace_members.iter().collect();
    let unique = packages.len() == metadata.packages.len()
        && nodes.len() == resolve.nodes.len()
        && members.len() == metadata.workspace_members.len();
    let root_resolved = resolve.root.as_ref().is_none_or(|id| nodes.contains(id));
    if unique && packages == nodes && root_resolved {
        return Ok(());
    }
    Err(Error::IncompleteMetadata(
        "inconsistent package, workspace or resolved node identities".into(),
    ))
}

/// Each node's dependency list must name exactly the packages its edges do.
fn check_dependencies(resolve: &cargo_metadata::Resolve) -> Result<(), Error> {
    for node in &resolve.nodes {
        if node.dependencies.iter().collect::<HashSet<_>>()
            != node.deps.iter().map(|dep| &dep.pkg).collect()
        {
            return Err(Error::IncompleteMetadata(format!(
                "inconsistent resolved dependencies for {}",
                node.id
            )));
        }
    }
    Ok(())
}
