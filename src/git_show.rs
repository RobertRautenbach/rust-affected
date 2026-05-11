use std::path::Path;
use std::process::Command;

/// Run `git show <sha>:<path>` and return the file content at that revision.
///
/// `repo_root` is the directory in which to invoke git (the workspace root,
/// where `.git/` lives). Returns `Err` with a descriptive message on any git
/// failure (missing SHA, missing path at that SHA, no `.git` directory).
pub fn git_show(repo_root: &Path, sha: &str, path: &str) -> Result<String, String> {
    let spec = format!("{sha}:{path}");
    let output = Command::new("git")
        .arg("-c")
        .arg("safe.directory=*")
        .arg("show")
        .arg(&spec)
        .current_dir(repo_root)
        .output()
        .map_err(|e| format!("failed to invoke git: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "`git show {spec}` failed ({}): {}",
            output.status,
            stderr.trim()
        ));
    }
    String::from_utf8(output.stdout).map_err(|e| format!("git output not utf-8: {e}"))
}
