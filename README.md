# rust-affected

Find which Rust packages need testing after a change.

If a shared library changes, this action selects that library and any packages that depend on it. Your workflow can then skip tests for unaffected packages.

You provide the changed files. The action returns package names.

## Quick start

Add this workflow to `.github/workflows/test.yml`. It tests each affected package in a separate job.

The example requires a committed `Cargo.lock` and the Rust toolchain your project needs on the runners.

```yaml
name: Test affected packages

on:
  pull_request:

permissions:
  contents: read

jobs:
  affected:
    runs-on: ubuntu-latest
    outputs:
      libraries: ${{ steps.affected.outputs.affected_library_members }}
      binaries: ${{ steps.affected.outputs.affected_binary_members }}
    steps:
      - uses: actions/checkout@v4
        with:
          persist-credentials: false

      - uses: tj-actions/changed-files@v47
        id: changed
        with:
          quotepath: false
          output_renamed_files_as_deleted_and_added: true

      - uses: robertrautenbach/rust-affected@v4.3.3
        id: affected
        with:
          changed_files: ${{ steps.changed.outputs.all_modified_files }}
          force_triggers: |
            Cargo.lock
            Cargo.toml
            rust-toolchain.toml
            .cargo/config.toml
            .github/

  test-libraries:
    needs: affected
    if: needs.affected.outputs.libraries != '[]'
    runs-on: ubuntu-latest
    strategy:
      fail-fast: false
      matrix:
        package: ${{ fromJSON(needs.affected.outputs.libraries) }}
    steps:
      - uses: actions/checkout@v4
        with:
          persist-credentials: false
      - env:
          PACKAGE: ${{ matrix.package }}
        run: cargo test --locked -p "$PACKAGE"

  test-binaries:
    needs: affected
    if: needs.affected.outputs.binaries != '[]'
    runs-on: ubuntu-latest
    strategy:
      fail-fast: false
      matrix:
        package: ${{ fromJSON(needs.affected.outputs.binaries) }}
    steps:
      - uses: actions/checkout@v4
        with:
          persist-credentials: false
      - env:
          PACKAGE: ${{ matrix.package }}
        run: cargo test --locked -p "$PACKAGE"
```

Empty lists skip their test jobs. Packages with both a library and a binary appear only in the binary list, where both are tested.

> **Safety:** The action runs Cargo, which can execute repository-configured programs. For untrusted pull requests, use `pull_request` with read-only permissions and no secrets, as shown above. Do not run untrusted PR code in privileged `pull_request_target` or `workflow_run` jobs.
>
> Set `ignore_cargo_config: true` to stop Cargo reading the repository's `.cargo/config.toml` and `rust-toolchain.toml`, which is how a pull request makes it run programs. `[patch]` entries, vendored sources and private registries defined only in that configuration are then ignored, so the action fails if your workspace needs them. This narrows the risk; it does not make a privileged job safe for untrusted code.

## Configure shared files

Some changes should test everything. Add those files or directories to `force_triggers`:

```yaml
force_triggers: |
  Cargo.lock
  Cargo.toml
  shared/
  **/*.sql
```

`shared/` matches everything inside that directory. `**/*.sql` matches SQL files anywhere in the workspace.

Patterns are relative to the workspace root. A pattern that could never match (empty, absolute such as `/Cargo.lock`, or containing `..`) fails the action instead of being silently ignored.

**There are no default triggers.** Adapt the quick-start list to include shared configuration, sources, and other files your builds read. The action warns when a root `Cargo.toml`, `Cargo.lock`, `.cargo/config.toml` or `rust-toolchain.toml` changes without belonging to a package or matching a trigger, because nothing is selected for it.

**Keep `Cargo.lock` as a trigger, and commit it.** Each file belongs to the deepest package directory that contains it. A pull request could add a package inside another package's source directory and take ownership of those files, hiding the change from the packages that depend on the original. Adding a package always changes `Cargo.lock`, so a `Cargo.lock` trigger turns that attempt into a full run. Without a committed lockfile there is nothing to catch it.

To leave packages out of the results:

```yaml
excluded_members: |
  internal-helper
  tools/
```

This excludes the package named `internal-helper` and packages under `tools/`. Other packages that depend on them can still be selected.

## Inputs and outputs

Input lists accept spaces or newlines between entries.

| Input | Description |
|---|---|
| `changed_files` | **Required.** Changed paths relative to the workspace root. |
| `force_triggers` | Paths or patterns that select all non-excluded packages. |
| `excluded_members` | Package names or directory paths to omit. |
| `ignore_cargo_config` | `true` runs Cargo without the repository's Cargo configuration. Default `false`. |

The action returns these outputs and adds a summary to the workflow run:

| Output | Description |
|---|---|
| `changed_crates` | Packages whose own files changed. |
| `affected_library_members` | Affected packages without a binary target. |
| `affected_binary_members` | Affected packages with a binary target. |
| `force_all` | `"true"` when a force trigger matched; otherwise `"false"`. |

Package lists are JSON arrays, such as `["shared-lib","api-server"]`. Use `fromJSON(...)` to read them in a workflow.

## Things to know

- **Include deleted and renamed files.** The quick-start example handles both. Filenames containing whitespace are unsupported.
- **Git-quoted paths are decoded.** `git diff --name-only` quotes unusual names, such as `"caf\303\251.rs"`. These are read correctly. A quoted name that cannot be decoded, such as one containing a space, is ignored with a warning.
- **Local crates outside the workspace count.** A change to a path dependency, `[patch]` target or vendored crate inside the repository selects the packages that depend on it. Only workspace packages are listed.
- **Empty input does not test everything.** It produces a warning and empty results after validation.
- **Shared files need care.** Detection uses package directories and current Cargo dependencies. It cannot discover every build-script input or reconstruct removed packages. Use force triggers for shared inputs inside the workspace; handle outside files in your workflow.
- **Runner requirement:** Run the detection job on Linux, such as `ubuntu-latest`. Separate build and test jobs can use any operating system and their own Rust toolchain.
- **Cargo may download dependencies.** An outdated lockfile fails the action. A missing lockfile may be created.

Invalid configuration or Cargo failures fail the action rather than return partial results.
