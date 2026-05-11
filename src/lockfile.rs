use cargo_lock::{Dependency, Lockfile, Package};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockfileDiff {
    /// Workspace member names whose transitive package set differs between
    /// the old and new lockfile.
    pub affected_members: HashSet<String>,
    /// `Some(reason)` when the diff could not be computed and the caller
    /// should fall back to a full rebuild. Examples: parse errors, malformed
    /// lockfile. `None` on success.
    pub fallback_reason: Option<String>,
}

/// Identity tuple used for comparing a package across the old and new
/// lockfiles. `source` is `None` for workspace members and path/replace
/// patches; `checksum` is `None` for git/path deps and workspace members.
/// Treat both `None`s as distinct values — do not flatten to empty string.
type PackageKey = (String, String, Option<String>, Option<String>);

/// Compute which workspace members are affected by a `Cargo.lock` change.
///
/// For each workspace member, the function walks its transitive dependency
/// set in both lockfiles. If any package in that set differs by name,
/// version, source, or checksum, the member is affected.
///
/// This naturally propagates through workspace-internal deps: if `lib-utils`
/// gains an external dep, `lib-core` (which depends on `lib-utils`) sees the
/// new dep in its own transitive set, so both end up in `affected_members`.
///
/// `workspace_member_names` should be the names of the current workspace
/// members (per the new graph). Members not present in either lockfile are
/// silently skipped — they'll be picked up by the file-based path via their
/// added/removed `Cargo.toml`.
pub fn compute_lockfile_diff(
    old_lockfile_content: &str,
    new_lockfile_content: &str,
    workspace_member_names: &[&str],
) -> LockfileDiff {
    let old_lock = match old_lockfile_content.parse::<Lockfile>() {
        Ok(lock) => lock,
        Err(e) => {
            return LockfileDiff {
                affected_members: HashSet::new(),
                fallback_reason: Some(format!("failed to parse old Cargo.lock: {e}")),
            };
        }
    };
    let new_lock = match new_lockfile_content.parse::<Lockfile>() {
        Ok(lock) => lock,
        Err(e) => {
            return LockfileDiff {
                affected_members: HashSet::new(),
                fallback_reason: Some(format!("failed to parse new Cargo.lock: {e}")),
            };
        }
    };

    let old_index = build_name_index(&old_lock);
    let new_index = build_name_index(&new_lock);

    let mut affected = HashSet::new();
    for name in workspace_member_names {
        let old_member = find_workspace_member(&old_lock, name);
        let new_member = find_workspace_member(&new_lock, name);

        if let (Some(old_pkg), Some(new_pkg)) = (old_member, new_member) {
            let old_set = transitive_set(old_pkg, &old_index);
            let new_set = transitive_set(new_pkg, &new_index);
            if old_set != new_set {
                affected.insert((*name).to_string());
            }
        }
        // Members present in one lockfile but not the other (new/removed
        // workspace members) are handled by the file-based diff and are
        // intentionally skipped here.
    }

    LockfileDiff {
        affected_members: affected,
        fallback_reason: None,
    }
}

/// Locate the lockfile entry for a workspace member by name.
///
/// Workspace members in `Cargo.lock` have no `source` field. We require a
/// unique source-less entry: zero or multiple matches return `None` to guard
/// against ambiguity (e.g. a `[patch]` that introduces another source-less
/// package with the same name as a workspace member).
fn find_workspace_member<'a>(lock: &'a Lockfile, name: &str) -> Option<&'a Package> {
    let mut iter = lock
        .packages
        .iter()
        .filter(|p| p.name.as_str() == name && p.source.is_none());
    let first = iter.next()?;
    if iter.next().is_some() {
        return None;
    }
    Some(first)
}

fn build_name_index(lock: &Lockfile) -> HashMap<String, Vec<&Package>> {
    let mut map: HashMap<String, Vec<&Package>> = HashMap::new();
    for pkg in &lock.packages {
        map.entry(pkg.name.as_str().to_string())
            .or_default()
            .push(pkg);
    }
    map
}

fn transitive_set<'a>(
    root: &'a Package,
    index: &HashMap<String, Vec<&'a Package>>,
) -> HashSet<PackageKey> {
    let mut visited: HashSet<PackageKey> = HashSet::new();
    let mut stack: Vec<&Package> = vec![root];
    while let Some(pkg) = stack.pop() {
        let key = package_key(pkg);
        if !visited.insert(key) {
            continue;
        }
        for dep in &pkg.dependencies {
            if let Some(matched) = resolve_dep(dep, index) {
                stack.push(matched);
            }
        }
    }
    visited
}

fn resolve_dep<'a>(
    dep: &Dependency,
    index: &HashMap<String, Vec<&'a Package>>,
) -> Option<&'a Package> {
    let candidates = index.get(dep.name.as_str())?;
    // `Dependency` after deserialization always carries a concrete (name,
    // version); `Dependency::matches` checks both. Two `[[package]]` entries
    // with identical (name, version) but different sources are not permitted
    // by Cargo, so name+version is a unique key in practice.
    candidates.iter().copied().find(|p| dep.matches(p))
}

fn package_key(pkg: &Package) -> PackageKey {
    (
        pkg.name.as_str().to_string(),
        pkg.version.to_string(),
        pkg.source.as_ref().map(|s| s.to_string()),
        pkg.checksum.as_ref().map(|c| c.to_string()),
    )
}
