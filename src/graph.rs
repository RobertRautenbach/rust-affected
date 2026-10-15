use crate::Error;
use serde::Deserialize;
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    path::{Component, Path, PathBuf},
};

/// The kind of a declared workspace dependency. All kinds are followed today.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum DependencyKind {
    #[default]
    Normal,
    Build,
    Dev,
    // Cargo may add dependency kinds without changing the metadata version.
    #[serde(other)]
    Other,
}

#[derive(Debug)]
pub(crate) struct Member {
    pub name: String,
    pub binary: bool,
}

/// Workspace members keyed by manifest directory, with reverse path-dependency edges.
#[derive(Debug)]
pub struct WorkspaceGraph {
    root: PathBuf,
    pub(crate) members: BTreeMap<PathBuf, Member>,
    reverse: HashMap<PathBuf, Vec<(PathBuf, DependencyKind)>>,
}

#[derive(Deserialize)]
struct Metadata {
    version: u32,
    workspace_root: PathBuf,
    workspace_members: Vec<String>,
    packages: Vec<Package>,
}

#[derive(Deserialize)]
struct Package {
    id: String,
    name: String,
    manifest_path: PathBuf,
    dependencies: Vec<Dependency>,
    targets: Vec<Target>,
}

#[derive(Deserialize)]
struct Dependency {
    path: Option<PathBuf>,
    kind: Option<DependencyKind>,
}

#[derive(Deserialize)]
struct Target {
    kind: Vec<String>,
}

impl WorkspaceGraph {
    /// Read Cargo's version 1 metadata; a resolved dependency graph is not needed.
    /// Only declared path dependencies between workspace members form edges.
    ///
    /// # Errors
    /// Returns an error for malformed JSON, unsupported metadata versions, or
    /// inconsistent workspace identities and manifest paths.
    pub fn from_json(json: &str) -> Result<Self, Error> {
        let metadata: Metadata = serde_json::from_str(json).map_err(Error::InvalidMetadata)?;
        if metadata.version != 1 {
            return Err(Error::Graph(format!(
                "unsupported metadata version {}",
                metadata.version
            )));
        }
        let root = absolute_path(&metadata.workspace_root)?;
        let mut packages = HashMap::new();
        for package in metadata.packages {
            if packages.insert(package.id.clone(), package).is_some() {
                return Err(Error::Graph("duplicate package ID".into()));
            }
        }
        let mut members = BTreeMap::new();
        let mut dependencies = Vec::new();
        let mut names = HashSet::new();
        for id in metadata.workspace_members {
            let package = packages.remove(&id).ok_or_else(|| {
                Error::Graph(format!("unknown or duplicate workspace member {id}"))
            })?;
            let manifest = absolute_path(&package.manifest_path)?;
            let directory = manifest
                .parent()
                .and_then(|path| path.strip_prefix(&root).ok())
                .ok_or_else(|| {
                    Error::Graph(format!(
                        "workspace manifest is outside workspace root: {}",
                        manifest.display()
                    ))
                })?
                .to_path_buf();
            if !names.insert(package.name.clone()) {
                return Err(Error::Graph(format!(
                    "duplicate workspace name {}",
                    package.name
                )));
            }
            let member = Member {
                name: package.name,
                binary: package
                    .targets
                    .iter()
                    .any(|target| target.kind.iter().any(|kind| kind == "bin")),
            };
            if members.insert(directory.clone(), member).is_some() {
                return Err(Error::Graph(format!(
                    "duplicate workspace directory {}",
                    directory.display()
                )));
            }
            dependencies.push((directory, package.dependencies));
        }
        let mut reverse: HashMap<PathBuf, Vec<(PathBuf, DependencyKind)>> = HashMap::new();
        for (dependent, declarations) in dependencies {
            for dependency in declarations {
                let Some(path) = dependency.path else {
                    continue;
                };
                let path = absolute_path(&path)?;
                let Ok(directory) = path.strip_prefix(&root) else {
                    continue;
                };
                if members.contains_key(directory) {
                    let edge = (dependent.clone(), dependency.kind.unwrap_or_default());
                    let edges = reverse.entry(directory.to_path_buf()).or_default();
                    if !edges.contains(&edge) {
                        edges.push(edge);
                    }
                }
            }
        }
        Ok(Self {
            root,
            members,
            reverse,
        })
    }

    /// The absolute workspace root reported by Cargo.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn reverse_closure(&self, direct: &HashSet<PathBuf>) -> HashSet<PathBuf> {
        let mut affected = direct.clone();
        let mut pending: VecDeque<_> = direct.iter().cloned().collect();
        while let Some(directory) = pending.pop_front() {
            if let Some(edges) = self.reverse.get(&directory) {
                // Include normal, build, dev, optional and target-specific declarations.
                for (dependent, _kind) in edges {
                    if affected.insert(dependent.clone()) {
                        pending.push_back(dependent.clone());
                    }
                }
            }
        }
        affected
    }
}

// Cargo emits absolute paths. Normalize their components without requiring
// dependency directories to exist (or following symlinks in changed-file input).
fn absolute_path(path: &Path) -> Result<PathBuf, Error> {
    if !path.is_absolute() {
        return Err(Error::Graph(format!(
            "metadata path is not absolute: {}",
            path.display()
        )));
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(Error::Graph(format!(
                        "metadata path escapes its root: {}",
                        path.display()
                    )));
                }
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute_affected;
    use serde_json::{Value, json};

    fn metadata() -> Value {
        let root = std::env::temp_dir().join("rust-affected-graph");
        json!({
            "version": 1,
            "workspace_root": root,
            "workspace_members": ["opaque-a", "opaque-b", "opaque-c"],
            "packages": [
                {"id":"opaque-a", "name":"a", "manifest_path":root.join("a/Cargo.toml"), "dependencies":[], "targets":[{"kind":["lib"]}]},
                {"id":"opaque-b", "name":"b", "manifest_path":root.join("b/Cargo.toml"), "dependencies":[], "targets":[{"kind":["lib"]}]},
                {"id":"opaque-c", "name":"c", "manifest_path":root.join("c/Cargo.toml"), "dependencies":[], "targets":[{"kind":["bin"]}]}
            ],
            "resolve": null,
            "future_field": true
        })
    }

    fn dependency(metadata: &Value, member: &str, kind: Option<&str>) -> Value {
        let root = Path::new(metadata["workspace_root"].as_str().unwrap());
        json!({"path":root.join(member), "kind":kind})
    }

    #[test]
    fn cycles_diamonds_and_duplicate_edges_terminate_without_duplicate_results() {
        let mut data = metadata();
        data["packages"][0]["dependencies"] = json!([dependency(&data, "b", Some("dev"))]);
        data["packages"][1]["dependencies"] = json!([
            dependency(&data, "a", None),
            dependency(&data, "a", None),
            dependency(&data, "a", Some("build")),
            dependency(&data, "a", Some("dev"))
        ]);
        data["packages"][2]["dependencies"] =
            json!([dependency(&data, "a", None), dependency(&data, "b", None)]);
        let graph = WorkspaceGraph::from_json(&data.to_string()).unwrap();
        assert_eq!(graph.reverse[Path::new("a")].len(), 4);
        for kind in [
            DependencyKind::Normal,
            DependencyKind::Build,
            DependencyKind::Dev,
        ] {
            assert!(graph.reverse[Path::new("a")].contains(&(PathBuf::from("b"), kind)));
        }
        let result = compute_affected(
            &graph,
            &["a/deleted.rs".into()],
            &[],
            &HashSet::<String>::new(),
        )
        .unwrap();
        assert_eq!(result.changed_crates, ["a"]);
        assert_eq!(result.affected_library_members, ["a", "b"]);
        assert_eq!(result.affected_binary_members, ["c"]);
    }

    #[test]
    fn edges_use_paths_instead_of_names_and_ignore_unknown_fields() {
        let mut data = metadata();
        let mut edge = dependency(&data, "b/../a/.", Some("future-kind"));
        edge["name"] = json!("misleading-name");
        edge["rename"] = json!("alias");
        edge["optional"] = json!(true);
        edge["target"] = json!("cfg(target_os = \"none\")");
        data["packages"][1]["dependencies"] = json!([edge]);
        // A registry dependency sharing a member name is not a path edge.
        data["packages"][2]["dependencies"] = json!([{"name":"a", "kind":null}]);
        let graph = WorkspaceGraph::from_json(&data.to_string()).unwrap();
        assert_eq!(
            graph.reverse[Path::new("a")],
            [(PathBuf::from("b"), DependencyKind::Other)]
        );
        let result =
            compute_affected(&graph, &["a/file".into()], &[], &HashSet::<String>::new()).unwrap();
        assert_eq!(result.affected_library_members, ["a", "b"]);
        assert_eq!(result.affected_binary_members, Vec::<String>::new());
    }

    #[test]
    fn non_member_packages_do_not_bridge_workspace_edges() {
        let mut data = metadata();
        data["workspace_members"] = json!(["opaque-a", "opaque-c"]);
        data["packages"][1]["dependencies"] = json!([dependency(&data, "a", None)]);
        data["packages"][2]["dependencies"] = json!([dependency(&data, "b", None)]);
        let graph = WorkspaceGraph::from_json(&data.to_string()).unwrap();
        let result =
            compute_affected(&graph, &["a/file".into()], &[], &HashSet::<String>::new()).unwrap();
        assert_eq!(result.affected_library_members, ["a"]);
        assert_eq!(result.affected_binary_members, Vec::<String>::new());
    }

    #[test]
    fn only_binary_target_kinds_classify_members_as_binaries() {
        let mut data = metadata();
        data["packages"][0]["targets"] =
            json!([{"kind":["proc-macro"]}, {"kind":["test"], "crate_types":["bin"]}]);
        data["packages"][1]["targets"] = json!([{"kind":["lib"]}, {"kind":["bin"]}]);
        let graph = WorkspaceGraph::from_json(&data.to_string()).unwrap();
        let result = compute_affected(
            &graph,
            &["Cargo.toml".into()],
            &["Cargo.toml".into()],
            &HashSet::<String>::new(),
        )
        .unwrap();
        assert_eq!(result.affected_library_members, ["a"]);
        assert_eq!(result.affected_binary_members, ["b", "c"]);
    }

    #[test]
    fn inconsistent_metadata_is_rejected_with_context() {
        for (pointer, value, message) in [
            ("/version", json!(2), "unsupported metadata version"),
            ("/workspace_root", json!("relative"), "not absolute"),
            (
                "/workspace_members/0",
                json!("missing"),
                "unknown or duplicate workspace member",
            ),
            (
                "/workspace_members/1",
                json!("opaque-a"),
                "unknown or duplicate workspace member",
            ),
            ("/packages/1/id", json!("opaque-a"), "duplicate package ID"),
            ("/packages/1/name", json!("a"), "duplicate workspace name"),
            (
                "/packages/1/manifest_path",
                metadata()["packages"][0]["manifest_path"].clone(),
                "duplicate workspace directory",
            ),
            (
                "/packages/0/manifest_path",
                json!(std::env::temp_dir().join("outside/Cargo.toml")),
                "outside workspace root",
            ),
        ] {
            let mut data = metadata();
            *data.pointer_mut(pointer).unwrap() = value;
            let error = WorkspaceGraph::from_json(&data.to_string()).unwrap_err();
            assert!(error.to_string().contains(message), "{error}");
        }
        assert!(matches!(
            WorkspaceGraph::from_json("{}"),
            Err(Error::InvalidMetadata(_))
        ));
    }
}
