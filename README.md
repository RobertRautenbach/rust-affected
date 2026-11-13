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
      libraries: ${{ steps.affected.outputs.affected_libraries }}
      binaries: ${{ steps.affected.outputs.affected_binaries }}
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          persist-credentials: false

      - uses: tj-actions/changed-files@9426d40962ed5378910ee2e21d5f8c6fcbf2dd96 # v47.0.6
        id: changed
        with:
          json: true
          escape_json: false
          safe_output: false
          quotepath: false
          output_renamed_files_as_deleted_and_added: true

      - uses: robertrautenbach/rust-affected@v5.0.2
        id: affected
        with:
          changed_files_json: ${{ steps.changed.outputs.all_modified_files }}
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
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
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
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          persist-credentials: false
      - env:
          PACKAGE: ${{ matrix.package }}
        run: cargo test --locked -p "$PACKAGE"
```

Empty lists skip their test jobs. Packages with both a library and a binary appear only in the binary list, where both are tested.

**Pinning.** A full version tag, as used above, is an immutable release: it cannot be moved. A major tag such as `@v5` follows the latest release of that major version. A full commit SHA is the strongest pin. Do not use `@main`: between a merge and the next release it can pair new inputs with the previous image. The other actions above are pinned to commit SHAs because a tag can be moved to different code; keep them current with Dependabot or a similar tool.

> **Safety:** By default the action runs Cargo from outside the checkout, so the repository's `.cargo/config.toml` is not read. A pull request can use that file to make Cargo run arbitrary programs. A `rust-toolchain.toml` is never used: the action runs its own pinned toolchain.
>
> If your workspace needs that configuration (`[patch]` entries, vendored sources, private registries, `net.git-fetch-with-cli`), set `trust_cargo_config: true`. Do that only where the checkout is trusted or the job is unprivileged: `pull_request` with read-only permissions and no secrets, as shown above. Never set it in a `pull_request_target` or `workflow_run` job that checks out pull request code.
>
> The default narrows the risk; it is not a sandbox. Do not run untrusted code in privileged jobs either way.

The `tj-actions/changed-files` settings above matter:

- `json: true` with `escape_json: false` produces a plain JSON array.
- `safe_output: false` and `quotepath: false` keep filenames exactly as they are in the repository. The list is passed to this action through `with:`, never interpolated into a shell script, which is what makes turning `safe_output` off safe here.

To build the list with git alone, use NUL-separated output so that no name is quoted or split:

```yaml
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          fetch-depth: 0
          persist-credentials: false

      - id: changed
        env:
          BASE: ${{ github.event.pull_request.base.sha }}
          HEAD: ${{ github.event.pull_request.head.sha }}
        run: |
          set -euo pipefail
          files=$(git diff --name-only --no-renames -z "$BASE" "$HEAD" | jq -Rsc 'split("\u0000") | map(select(. != ""))')
          echo "files=$files" >> "$GITHUB_OUTPUT"
```

Keep `set -euo pipefail`. Without it a failed `git diff` is hidden by `jq`, the step reports `[]`, and every test job is skipped.

`--no-renames` lists a renamed file as a deletion and an addition, so both paths are included. The checkout needs enough history to contain both commits; `fetch-depth: 0` fetches all of it.

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
excluded_packages: |
  internal-helper
  tools/
```

This excludes the package named `internal-helper` and packages under `tools/`. Other packages that depend on them can still be selected.

## Inputs and outputs

`changed_files_json` is a JSON array. The other lists accept spaces or newlines between entries.

| Input | Description |
|---|---|
| `changed_files_json` | **Required.** JSON array of changed paths relative to the workspace root, such as `["src/lib.rs","docs/my file.md"]`. Use `[]` when nothing changed. |
| `force_triggers` | Paths or patterns that select all non-excluded packages. |
| `excluded_packages` | Package names or directory paths to omit. |
| `trust_cargo_config` | `true` lets Cargo read the repository's `.cargo/config.toml`. Default `false`. |

The action returns these outputs and adds a summary to the workflow run:

| Output | Description |
|---|---|
| `changed_packages` | Packages whose own files changed. |
| `affected_libraries` | Affected packages without a binary target. |
| `affected_binaries` | Affected packages with a binary target. |
| `force_all` | `"true"` when a force trigger matched; otherwise `"false"`. |

Package lists are JSON arrays, such as `["shared-lib","api-server"]`. Use `fromJSON(...)` to read them in a workflow.

## Things to know

- **Include deleted and renamed files.** The quick-start example handles both.
- **Paths are exact.** Each array element is one path, so names with spaces, quotes or non-ASCII characters work. Nothing is unquoted: pass real names, not git's quoted form such as `"caf\303\251.rs"`.
- **A missing list fails the action.** `changed_files_json` must be a JSON array of strings. A blank value is treated as a broken workflow, not as "nothing changed".
- **Local crates outside the workspace count.** A change to a path dependency, `[patch]` target or vendored crate inside the repository selects the packages that depend on it. Only workspace packages are listed.
- **An empty array does not test everything.** `[]` produces a warning and empty results after validation.
- **Shared files need care.** Detection uses package directories and current Cargo dependencies. It cannot discover every build-script input or reconstruct removed packages. Use force triggers for shared inputs inside the workspace; handle outside files in your workflow.
- **Runner requirement:** Run the detection job on Linux, such as `ubuntu-latest`. Separate build and test jobs can use any operating system and their own Rust toolchain.
- **Cargo may download dependencies.** An outdated lockfile fails the action. A missing lockfile may be created.
- **The job's environment reaches Cargo.** Variables such as `CARGO_HOME`, `RUSTC`, `RUSTC_WRAPPER` and `CARGO_BUILD_*` set on the workflow or job apply inside the action. A `CARGO_HOME` or `RUSTUP_HOME` inside the checkout fails the action unless `trust_cargo_config` is `true`, because Cargo would read the repository's configuration from the first and rustup would run a toolchain from the second.
- **Inputs come only from `with:`.** A workflow or job variable named like an input, such as `TRUST_CARGO_CONFIG`, does not replace it.

Invalid configuration or Cargo failures fail the action rather than return partial results.

## Migrating from v4

v5 replaces the `changed_files` input with `changed_files_json`, stops reading the repository's Cargo configuration unless told to, and renames one input and three outputs so that they all say "packages".

| v4 | v5 |
|---|---|
| `changed_files: a.rs b.rs` | `changed_files_json: '["a.rs","b.rs"]'` |
| Empty or missing value meant "nothing changed" | `[]` means "nothing changed"; a blank value fails |
| Filenames containing whitespace unsupported | Any filename works |
| Git-quoted names were decoded | Names are taken literally |
| Repository Cargo configuration read unless `ignore_cargo_config: true` | Not read unless `trust_cargo_config: true` |
| Input `excluded_members` | `excluded_packages` |
| Output `changed_crates` | `changed_packages` |
| Output `affected_library_members` | `affected_libraries` |
| Output `affected_binary_members` | `affected_binaries` |

A workflow that still passes `changed_files`, `excluded_members` or `ignore_cargo_config` fails with an error naming the input and its replacement, so nothing is silently ignored.

Change the step that produces the list to emit JSON, as in the quick start, and rename the inputs.

If your workspace relies on `.cargo/config.toml`, add `trust_cargo_config: true`. When that configuration is needed but unread, Cargo usually fails and the error names the file. Without a committed `Cargo.lock` it may resolve differently instead; the action warns about the unread file in that case.

The renamed outputs carry the same values as before. Update every `steps.<id>.outputs.<name>` reference: an old output name now yields an empty string, which `fromJSON` rejects.
