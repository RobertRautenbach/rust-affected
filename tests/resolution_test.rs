use rust_affected::{WorkspaceGraph, compute_affected};
use serde_json::{Value, json};
use std::{collections::HashSet, path::PathBuf, process::Command};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn metadata(name: &str) -> Value {
    let output = Command::new(env!("CARGO"))
        .args([
            "metadata",
            "--format-version",
            "1",
            "--all-features",
            "--locked",
            "--offline",
        ])
        .current_dir(fixture(name))
        .env("CARGO_CACHE_RUSTC_INFO", "0")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn cargo_selected_identities_control_affected_members() {
    for (layout, binaries) in [
        ("patch-selected", vec!["app"]),
        ("patch-config", vec!["app"]),
        ("patch-unused", vec![]),
        ("bridge", vec!["app", "second"]),
    ] {
        let data = metadata(layout);
        let packages = data["packages"].as_array().unwrap();
        let core = packages
            .iter()
            .find(|p| {
                p["manifest_path"]
                    .as_str()
                    .unwrap()
                    .ends_with("/core/Cargo.toml")
            })
            .unwrap();
        let app = packages.iter().find(|p| p["name"] == "app").unwrap();
        let app_node = data["resolve"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["id"] == app["id"])
            .unwrap();
        if layout.starts_with("patch") {
            let selected = &app_node["deps"][0]["pkg"];
            if layout == "patch-unused" {
                assert_ne!(selected, &core["id"]);
            } else {
                assert_eq!(selected, &core["id"]);
            }
        }
        let graph = WorkspaceGraph::from_json(&data.to_string()).unwrap();
        let result = compute_affected(
            &graph,
            &["core/src/lib.rs".into()],
            &[],
            &HashSet::<String>::new(),
        )
        .unwrap();
        assert_eq!(result.changed_crates, ["resolution-core"], "{layout}");
        assert_eq!(
            result.affected_library_members,
            ["resolution-core"],
            "{layout}"
        );
        assert_eq!(result.affected_binary_members, binaries, "{layout}");
        let excluded = compute_affected(
            &graph,
            &["core/src/lib.rs".into()],
            &[],
            &HashSet::from(["resolution-core".into()]),
        )
        .unwrap();
        assert_eq!(excluded.changed_crates, Vec::<String>::new());
        assert_eq!(excluded.affected_library_members, Vec::<String>::new());
        assert_eq!(excluded.affected_binary_members, binaries, "{layout}");
        // Each independent binary is also a change origin: traversal must not run forwards.
        for binary in if layout == "bridge" {
            vec!["app", "second"]
        } else {
            vec!["app"]
        } {
            let result = compute_affected(
                &graph,
                &[format!("{binary}/src/main.rs")],
                &[],
                &HashSet::<String>::new(),
            )
            .unwrap();
            assert_eq!(result.changed_crates, [binary]);
            assert_eq!(result.affected_binary_members, [binary]);
            assert_eq!(result.affected_library_members, Vec::<String>::new());
        }
        let forced = compute_affected(
            &graph,
            &["Cargo.toml".into()],
            &["Cargo.toml".into()],
            &HashSet::from(["app".into()]),
        )
        .unwrap();
        assert_eq!(forced.affected_library_members, ["resolution-core"]);
        assert_eq!(
            forced.affected_binary_members,
            if layout == "bridge" {
                vec!["second"]
            } else {
                vec![]
            }
        );
        assert!(forced.force_all);
    }
}

#[test]
fn unresolved_or_inconsistent_metadata_is_rejected() {
    let valid = metadata("patch-selected");
    for mutation in [
        "missing resolve",
        "null resolve",
        "empty nodes",
        "missing nodes",
        "duplicate node",
        "duplicate package",
        "duplicate member",
        "unknown node",
        "invalid root",
        "inconsistent dependencies",
        "missing deps",
        "null deps",
        "unsupported version",
        "missing version",
        "invalid version",
        "missing dependencies",
    ] {
        let mut data = valid.clone();
        match mutation {
            "missing resolve" => {
                data.as_object_mut().unwrap().remove("resolve");
            }
            "null resolve" => data["resolve"] = Value::Null,
            "empty nodes" => data["resolve"]["nodes"] = json!([]),
            "missing nodes" => {
                data["resolve"].as_object_mut().unwrap().remove("nodes");
            }
            "duplicate node" => {
                let node = data["resolve"]["nodes"][0].clone();
                data["resolve"]["nodes"].as_array_mut().unwrap().push(node);
            }
            "duplicate package" => {
                let package = data["packages"][0].clone();
                data["packages"].as_array_mut().unwrap().push(package);
            }
            "duplicate member" => {
                let member = data["workspace_members"][0].clone();
                data["workspace_members"]
                    .as_array_mut()
                    .unwrap()
                    .push(member);
            }
            "unknown node" => data["resolve"]["nodes"][0]["id"] = json!("missing-id"),
            "invalid root" => data["resolve"]["root"] = json!("missing-id"),
            "inconsistent dependencies" => {
                data["resolve"]["nodes"][0]["deps"][0]["pkg"] = json!("missing-id");
            }
            "missing deps" => {
                // Use a leaf: missing deps must not silently default to an empty list.
                let leaf = data["resolve"]["nodes"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|node| node["dependencies"].as_array().unwrap().is_empty())
                    .unwrap();
                leaf.as_object_mut().unwrap().remove("deps");
            }
            "null deps" => data["resolve"]["nodes"][0]["deps"] = Value::Null,
            "unsupported version" => data["version"] = json!(2),
            "missing version" => {
                data.as_object_mut().unwrap().remove("version");
            }
            "invalid version" => data["version"] = json!("1"),
            "missing dependencies" => {
                data["resolve"]["nodes"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("dependencies");
            }
            _ => unreachable!(),
        }
        assert!(
            WorkspaceGraph::from_json(&data.to_string()).is_err(),
            "accepted {mutation}"
        );
    }
}

#[test]
fn guppy_construction_errors_retain_their_source() {
    use std::error::Error;
    let mut data = metadata("patch-selected");
    for package in data["packages"].as_array_mut().unwrap() {
        package["name"] = json!("duplicate-workspace-name");
    }
    let error = WorkspaceGraph::from_json(&data.to_string()).unwrap_err();
    assert!(error.source().is_some(), "{error}");
    assert!(error.to_string().contains("package graph"));
}

#[test]
fn guppy_rejects_unknown_members_and_dependency_packages() {
    use std::error::Error as _;
    for unknown_member in [true, false] {
        let mut data = metadata("patch-selected");
        if unknown_member {
            data["workspace_members"][0] = json!("missing-id");
        } else {
            let node = &mut data["resolve"]["nodes"][0];
            node["dependencies"][0] = json!("missing-id");
            node["deps"][0]["pkg"] = json!("missing-id");
        }
        let error = WorkspaceGraph::from_json(&data.to_string()).unwrap_err();
        assert!(matches!(error, rust_affected::Error::Graph(_)), "{error}");
        assert!(error.source().is_some());
        let error = guppy::graph::PackageGraph::from_json(data.to_string()).unwrap_err();
        assert!(
            matches!(error, guppy::Error::PackageGraphConstructError(_)),
            "{error}"
        );
    }
}

#[test]
fn empty_dependency_arrays_are_valid() {
    let data = metadata("patch-selected");
    assert!(
        data["resolve"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| { node["dependencies"] == json!([]) && node["deps"] == json!([]) })
    );
    WorkspaceGraph::from_json(&data.to_string()).unwrap();
}
