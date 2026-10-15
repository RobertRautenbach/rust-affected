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
        assert!(excluded.changed_crates.is_empty());
        assert!(excluded.affected_library_members.is_empty());
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
            assert!(result.affected_library_members.is_empty());
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
    for mutation in 0..7 {
        let mut data = valid.clone();
        match mutation {
            0 => {
                data.as_object_mut().unwrap().remove("resolve");
            }
            1 => data["resolve"] = Value::Null,
            2 => data["resolve"]["nodes"] = json!([]),
            3 => data["resolve"]["nodes"][0]["deps"][0]["pkg"] = json!("missing-id"),
            4 => data["version"] = json!(2),
            5 => {
                let node = data["resolve"]["nodes"][0].clone();
                data["resolve"]["nodes"].as_array_mut().unwrap().push(node);
            }
            6 => data["resolve"]["nodes"][0]["id"] = json!("missing-id"),
            _ => unreachable!(),
        }
        assert!(
            WorkspaceGraph::from_json(&data.to_string()).is_err(),
            "accepted mutation {mutation}"
        );
    }
}
