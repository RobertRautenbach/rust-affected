use std::collections::BTreeSet;
use toml::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestDiff {
    /// `true` when a build-affecting key in the root `Cargo.toml` changed
    /// between the old and new revision, requiring a full workspace rebuild.
    /// `false` when only "safe" keys changed — workspace member adds/removes,
    /// metadata, and workspace-deps changes that are reflected in `Cargo.lock`.
    pub force_all: bool,
    /// `Some(reason)` on parse failure; the caller should fall back to a full
    /// rebuild and emit a warning.
    pub fallback_reason: Option<String>,
}

/// Diff the root `Cargo.toml` from two revisions and decide whether any
/// build-affecting key changed. See README "How root Cargo.toml changes are
/// handled" for the precise allow-list.
pub fn compute_manifest_diff(old_content: &str, new_content: &str) -> ManifestDiff {
    let old: Value = match toml::from_str(old_content) {
        Ok(v) => v,
        Err(e) => {
            return ManifestDiff {
                force_all: false,
                fallback_reason: Some(format!("failed to parse old Cargo.toml: {e}")),
            };
        }
    };
    let new: Value = match toml::from_str(new_content) {
        Ok(v) => v,
        Err(e) => {
            return ManifestDiff {
                force_all: false,
                fallback_reason: Some(format!("failed to parse new Cargo.toml: {e}")),
            };
        }
    };

    let mut changed: Vec<String> = Vec::new();
    diff_paths(&old, &new, "", &mut changed);

    for path in &changed {
        if !is_safe_path(path) {
            return ManifestDiff {
                force_all: true,
                fallback_reason: None,
            };
        }
    }
    ManifestDiff {
        force_all: false,
        fallback_reason: None,
    }
}

/// Walk `old` and `new` in parallel and append leaf-level dotted paths whose
/// values differ, or whose key exists in only one side. Tables recurse;
/// arrays and primitives are compared as leaves. When a whole subtree exists
/// on only one side, every leaf under it is emitted individually so the
/// allow-list can classify each one (e.g. `patch.crates-io.serde.git`).
fn diff_paths(old: &Value, new: &Value, prefix: &str, out: &mut Vec<String>) {
    match (old, new) {
        (Value::Table(a), Value::Table(b)) => {
            let keys: BTreeSet<&str> = a.keys().chain(b.keys()).map(String::as_str).collect();
            for k in keys {
                let path = if prefix.is_empty() {
                    k.to_string()
                } else {
                    format!("{prefix}.{k}")
                };
                match (a.get(k), b.get(k)) {
                    (Some(av), Some(bv)) => diff_paths(av, bv, &path, out),
                    (Some(only), None) | (None, Some(only)) => emit_leaves(only, &path, out),
                    (None, None) => {}
                }
            }
        }
        (a, b) if a != b => out.push(prefix.to_string()),
        _ => {}
    }
}

/// Recurse into `v` and append every leaf path with `prefix`. Empty tables
/// emit nothing (there's no meaningful change to classify).
fn emit_leaves(v: &Value, prefix: &str, out: &mut Vec<String>) {
    match v {
        Value::Table(t) if !t.is_empty() => {
            for (k, val) in t {
                let path = format!("{prefix}.{k}");
                emit_leaves(val, &path, out);
            }
        }
        _ => out.push(prefix.to_string()),
    }
}

/// True when a changed leaf path is allow-listed (does not require a full
/// rebuild). Everything else — including unrecognized sections — is treated
/// as build-affecting.
fn is_safe_path(path: &str) -> bool {
    // ── Workspace structure (member adds caught by file-based path) ──
    if matches!(
        path,
        "workspace.members" | "workspace.exclude" | "workspace.default-members"
    ) {
        return true;
    }

    // ── Workspace metadata (Cargo doesn't read for build) ──
    if path == "workspace.metadata" || path.starts_with("workspace.metadata.") {
        return true;
    }

    // ── workspace.package.<metadata-only> ──
    if let Some(rest) = path.strip_prefix("workspace.package.") {
        let field = rest.split('.').next().unwrap_or(rest);
        const SAFE_PKG_FIELDS: &[&str] = &[
            "description",
            "authors",
            "license",
            "license-file",
            "license_file",
            "repository",
            "homepage",
            "documentation",
            "readme",
            "keywords",
            "categories",
            "publish",
            "include",
            "exclude",
        ];
        if SAFE_PKG_FIELDS.contains(&field) {
            return true;
        }
        // edition, rust-version, version, links, etc. are NOT safe.
        return false;
    }

    // ── workspace.dependencies.<name>[.<field>] ──
    if let Some(rest) = path.strip_prefix("workspace.dependencies.") {
        return workspace_dep_field_is_safe(rest);
    }

    // ── patch.<source>.<name>[.<field>] / replace.<name>[.<field>] ──
    if let Some(rest) = path.strip_prefix("patch.") {
        // <source>.<name>[.<field>...]
        let mut parts = rest.splitn(3, '.');
        let _source = parts.next();
        let _name = parts.next();
        match parts.next() {
            None => return true, // patch.<source> or patch.<source>.<name> add/remove
            Some(field_path) => {
                let field = field_path.split('.').next().unwrap_or(field_path);
                return PATCH_SAFE_FIELDS.contains(&field);
            }
        }
    }
    if let Some(rest) = path.strip_prefix("replace.") {
        // <name>[.<field>]
        match rest.split_once('.') {
            None => return true, // entry add/remove
            Some((_name, field_path)) => {
                let field = field_path.split('.').next().unwrap_or(field_path);
                return PATCH_SAFE_FIELDS.contains(&field);
            }
        }
    }

    // ── Non-virtual workspace root: per-package keys (file-based handles) ──
    const ROOT_PACKAGE_PREFIXES: &[&str] = &[
        "package",
        "dependencies",
        "dev-dependencies",
        "dev_dependencies",
        "build-dependencies",
        "build_dependencies",
        "features",
        "lib",
        "bin",
        "test",
        "example",
        "bench",
        "target",
        "badges",
    ];
    for prefix in ROOT_PACKAGE_PREFIXES {
        if path == *prefix || path.starts_with(&format!("{prefix}.")) {
            return true;
        }
    }

    false
}

/// Allow-list of patch/replace sub-fields that are reflected in `Cargo.lock`
/// (and thus handled by the lockfile-aware diff). All other fields, plus
/// `features`/`default-features`/`optional`, force a full rebuild.
const PATCH_SAFE_FIELDS: &[&str] = &[
    "version", "git", "rev", "branch", "tag", "path", "package", "registry",
];

/// Classify a path beneath `workspace.dependencies.` (the prefix is already
/// stripped). Returns `true` when the change is lockfile-reflected and
/// therefore covered by the Cargo.lock-aware diff.
fn workspace_dep_field_is_safe(rest: &str) -> bool {
    // `rest` is "<name>" (entry add/remove) or "<name>.<field>[...]".
    match rest.split_once('.') {
        None => true, // workspace.dependencies.<name> as a whole — lockfile catches add/remove
        Some((_name, field_path)) => {
            let field = field_path.split('.').next().unwrap_or(field_path);
            const SAFE: &[&str] = &[
                "version", "git", "rev", "branch", "tag", "path", "registry",
            ];
            // features, default-features, default_features, optional, package
            // rename are intentionally absent — these change compilation without
            // necessarily changing Cargo.lock.
            SAFE.contains(&field)
        }
    }
}
