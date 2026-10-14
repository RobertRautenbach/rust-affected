# rust-affected

A GitHub Action that detects which packages in a Rust workspace are affected by a set of changed files, using the Cargo dependency graph.

Given a set of changed files, it determines:
- **`changed_crates`** — packages with files directly modified
- **`affected_library_members`** — pure library crates that are changed or (transitively) depend on a changed crate; binary crates are excluded from this list
- **`affected_binary_members`** — affected crates that have a binary target; mutually exclusive with `affected_library_members`
- **`force_all`** — whether a configured force-trigger file changed, meaning the entire workspace should be considered affected

## Usage

```yaml
jobs:
  plan:
    runs-on: ubuntu-latest
    outputs:
      affected_binary_members: ${{ steps.affected.outputs.affected_binary_members }}
      force_all: ${{ steps.affected.outputs.force_all }}
    steps:
      - uses: actions/checkout@v4

      - name: Get changed files
        id: changed
        uses: tj-actions/changed-files@v47
        with:
          quotepath: false
          output_renamed_files_as_deleted_and_added: true

      - name: Detect affected packages
        id: affected
        uses: robertrautenbach/rust-affected@v4.0.6
        with:
          changed_files: ${{ steps.changed.outputs.all_modified_files }}
          force_triggers: |
            Cargo.lock
            Cargo.toml
            rust-toolchain.toml
            .cargo/config.toml
            .github/

  deploy:
    needs: plan
    if: contains(needs.plan.outputs.affected_binary_members, 'my-service') || needs.plan.outputs.force_all == 'true'
    runs-on: ubuntu-latest
    steps:
      - run: echo "Deploying my-service"
```

## Example output

Imagine a workspace where `lib-utils` is a shared library depended on by `lib-core`, `lib-core-ext`, `app-alpha`, `app-beta`, and `tool-alpha`. A PR that changes `lib-utils/src/lib.rs` produces:

```
changed_crates=["lib-utils"]
affected_library_members=["lib-core","lib-core-ext","lib-utils"]
affected_binary_members=["app-alpha","app-beta","tool-alpha"]
force_all=false
```

The action also writes a job summary to the workflow run:

> ## rust-affected
>
> | | Crates |
> |---|---|
> | **Changed** | `lib-utils` |
> | **Affected libraries** | `lib-core` `lib-core-ext` `lib-utils` |
> | **Affected binaries** | `app-alpha` `app-beta` `tool-alpha` |
>
> ### Changed crates
> - `lib-utils`
>
> ### Affected library members
> - `lib-core`
> - `lib-core-ext`
> - `lib-utils`
>
> ### Affected binary members
> - `app-alpha`
> - `app-beta`
> - `tool-alpha`

## Getting changed files

You provide the list of changed files — here are common approaches:

### tj-actions/changed-files

```yaml
- name: Get changed files
  id: changed
  uses: tj-actions/changed-files@v47
  with:
    quotepath: false
    output_renamed_files_as_deleted_and_added: true

- uses: robertrautenbach/rust-affected@v4.0.6
  with:
    changed_files: ${{ steps.changed.outputs.all_modified_files }}
```

Use `all_modified_files` because `all_changed_files` omits deletions. The rename option includes both the old and new paths, so moving a file between crates marks both. `quotepath: false` preserves non-ASCII filenames.

### git diff (no extra dependencies)

```yaml
- name: Get changed files
  id: changed
  env:
    BASE_SHA: ${{ github.event.pull_request.base.sha }}
  shell: bash
  run: |
    set -euo pipefail
    files="$(git -c core.quotepath=false diff --no-renames --name-only "$BASE_SHA"..HEAD | tr '\n' ' ')"
    printf 'files=%s\n' "$files" >> "$GITHUB_OUTPUT"

- uses: robertrautenbach/rust-affected@v4.0.6
  with:
    changed_files: ${{ steps.changed.outputs.files }}
```

> **Note:** Requires `fetch-depth: 0` on `actions/checkout` so the base SHA is available locally.

## Inputs

| Input | Required | Description |
|---|---|---|
| `changed_files` | **Yes** | Space- or newline-separated list of changed paths, including deletions and both paths of renames. Relative paths, leading `./`, and absolute paths beneath the workspace root are accepted. |
| `force_triggers` | No | Space- or newline-separated list of glob patterns that trigger a full rebuild when any matching file changes. Supports `*`, `**`, and `?`. A bare name (e.g. `Cargo.lock`) matches that exact path only. A trailing slash (e.g. `.github/`) matches the directory and everything inside it. Full globs are also supported (e.g. `**/*.sql`, `migrations/**`). If omitted, `force_all` is never set. |
| `excluded_members` | No | Space- or newline-separated list of workspace member names **or path prefixes** to exclude from all outputs. A plain name (e.g. `my-tool`) matches the crate name directly. An entry containing `/` is matched against the crate's directory relative to the workspace root: a trailing slash (e.g. `tools/`) excludes every crate under that directory, while a relative path (e.g. `tools/my-tool`) excludes that crate and members nested beneath it, but not `tools/my-tool-extra`. Leading `./` and redundant separators are accepted in path prefixes. Useful for internal tooling or helper crates that should never appear in CI results. If omitted, no members are excluded. |

## How files map to crates

Each valid changed path belongs to the workspace member with the deepest matching directory. A file in `parent/nested/` belongs to that member, not also to `parent/`. Dependents are then found through the Cargo graph, including through excluded members. Exclusions filter results without transferring ownership to another member.

If the workspace root is a package, it owns paths outside other members, including root documentation and configuration. This is conservative directory attribution; it does not apply `cargo package` inclusion rules. In a virtual workspace, paths outside members have no owner unless they match a force trigger. Configure shared manifests and configuration as force triggers when they should affect the entire workspace.

Paths are normalized lexically: `./lib/src/lib.rs`, `lib//src/lib.rs`, and an absolute path beneath the workspace root are equivalent. Files need not exist, so deletions work. Symlinks are not resolved. Empty paths, root-only paths, any `..` component, and foreign absolute paths are warned about and ignored for both attribution and force triggers. Inside the Docker action, workspace-absolute paths start with `/github/workspace`; host paths such as `/home/runner/work/...` are outside that root.

Inputs are split on whitespace, so filenames containing spaces, tabs, or newlines are unsupported. Supply unquoted paths, including deleted files and both ends of renames. Removed workspace members cannot be reconstructed from current metadata.

Empty input produces a warning and empty results after validating the workspace and configuration. Exclusions matching no member also produce warnings: `tools` is a crate name, while `tools/` is a directory prefix. Invalid UTF-8 inputs, invalid globs, Cargo failures, and output-writing failures exit 1 with an `::error::` annotation. Warnings and errors are written to stderr.

## Outputs

| Output | Description |
|---|---|
| `changed_crates` | JSON array of crate names with directly changed files |
| `affected_library_members` | JSON array of affected workspace members that are pure library crates (no binary target) |
| `affected_binary_members` | JSON array of affected workspace members that have a binary target; mutually exclusive with `affected_library_members` |
| `force_all` | `"true"` if a force-trigger file changed, otherwise `"false"` |

## Migrating from v3

v4 is a breaking change. The action no longer detects changed files itself — you provide them via the `changed_files` input.

| v3 | v4 |
|---|---|
| `base_sha` input (optional) | Removed — compute your own diff |
| Changed files detected automatically | `changed_files` input (**required**) |
| All outputs | Unchanged |

**Before (v3):**
```yaml
- uses: robertrautenbach/rust-affected@v3
```

**After (v4):**
```yaml
- name: Get changed files
  id: changed
  uses: tj-actions/changed-files@v47
  with:
    quotepath: false
    output_renamed_files_as_deleted_and_added: true

- uses: robertrautenbach/rust-affected@v4.0.6
  with:
    changed_files: ${{ steps.changed.outputs.all_modified_files }}
```
