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

      - name: Detect affected packages
        id: affected
        uses: robertrautenbach/rust-affected@v4.0.2
        with:
          changed_files: ${{ steps.changed.outputs.all_changed_files }}
          force_triggers: |
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

- uses: robertrautenbach/rust-affected@v4.0.2
  with:
    changed_files: ${{ steps.changed.outputs.all_changed_files }}
```

### git diff (no extra dependencies)

```yaml
- name: Get changed files
  id: changed
  run: |
    echo "files=$(git diff --name-only ${{ github.event.pull_request.base.sha }}..HEAD | tr '\n' ' ')" >> "$GITHUB_OUTPUT"

- uses: robertrautenbach/rust-affected@v4.0.2
  with:
    changed_files: ${{ steps.changed.outputs.files }}
```

> **Note:** Requires `fetch-depth: 0` on `actions/checkout` so the base SHA is available locally.

## Inputs

| Input | Required | Description |
|---|---|---|
| `changed_files` | **Yes** | Space- or newline-separated list of changed file paths relative to the workspace root. |
| `force_triggers` | No | Space- or newline-separated list of glob patterns that trigger a full rebuild when any matching file changes. Supports `*`, `**`, and `?`. A bare name (e.g. `Cargo.lock`) matches that exact path only. A trailing slash (e.g. `.github/`) matches the directory and everything inside it. Full globs are also supported (e.g. `**/*.sql`, `migrations/**`). If omitted, `force_all` is never set. |
| `excluded_members` | No | Space- or newline-separated list of workspace member names **or path prefixes** to exclude from all outputs. A plain name (e.g. `my-tool`) matches the crate name directly. An entry containing `/` is matched against the crate's directory relative to the workspace root: a trailing slash (e.g. `tools/`) excludes every crate under that directory, while an exact relative path (e.g. `tools/my-tool`) excludes only that crate. Useful for internal tooling or helper crates that should never appear in CI results. If omitted, no members are excluded. |

## Outputs

| Output | Description |
|---|---|
| `changed_crates` | JSON array of crate names with directly changed files |
| `affected_library_members` | JSON array of affected workspace members that are pure library crates (no binary target) |
| `affected_binary_members` | JSON array of affected workspace members that have a binary target; mutually exclusive with `affected_library_members` |
| `force_all` | `"true"` if a force-trigger file changed, otherwise `"false"` |

## How `Cargo.lock` changes are handled

When `Cargo.lock` is among the changed files, the action diffs the old and new lockfile (the old one is fetched with `git show $BASE_SHA:Cargo.lock`) and walks each workspace member's transitive dependency set in both. Members whose transitive set differs by `(name, version, source, checksum)` are marked affected; the rest are skipped. Reverse-dependency traversal in the workspace graph then unions these with any file-based direct changes, so an external dep bump that only touches one transitive crate doesn't rebuild the rest of the workspace.

If you'd rather keep the old "always rebuild everything on Cargo.lock change" behavior, add `Cargo.lock` to your `force_triggers` list — the diff is skipped whenever the lockfile is force-triggered.

The diff falls back to `force_all=true` (the safe over-build direction) when:
- `BASE_SHA` is not supplied
- The old `Cargo.lock` cannot be fetched at that SHA (shallow clone, missing file)
- Either lockfile fails to parse

### Known limitations

- **Dev-deps, build-deps, and proc-macro deps are not distinguished from normal deps.** `Cargo.lock` does not preserve dependency kind, so a dev-dep-only bump triggers a rebuild of dependents that strictly wouldn't need one. Conservative over-build, no false negatives.
- **Target-specific (`cfg(...)`) deps are treated as universally affecting.** A Windows-only dep version bump will mark Linux-only crates affected. Same conservative direction.
- Everything else — registry version bumps, git revision changes, path-dep rewrites, `[patch]`/`[replace]` redirects, multi-version resolutions, checksum changes (yanked re-uploads) — is captured exactly.

## How root `Cargo.toml` changes are handled

When the root `Cargo.toml` is among the changed files, the action parses the old and new manifest and classifies each changed key as either build-affecting or safe-to-ignore. Member-list edits, metadata churn, and workspace-dep version bumps (which `Cargo.lock` already catches) don't force a rebuild. Build-affecting changes — profile flags, the resolver, edition bumps, lint levels — set `force_all=true`.

Add `Cargo.toml` to your `force_triggers` list to opt out and keep the old "any change forces full rebuild" behavior.

The diff falls back to `force_all=true` (the safe over-build direction) on the same conditions as the lockfile diff: missing `BASE_SHA`, unfetchable old manifest, or parse error.

### Allow-listed key paths (no force_all)

- `workspace.members`, `workspace.exclude`, `workspace.default-members` — add/remove members; the file-based path picks up new crates
- `workspace.metadata.*` — Cargo doesn't read these for build
- `workspace.package.{description,authors,license,license-file,repository,homepage,documentation,readme,keywords,categories,publish,include,exclude}` — metadata-only
- `workspace.dependencies.<name>.{version,git,rev,branch,tag,path,registry}` — lockfile-reflected; the Cargo.lock-aware diff handles them
- `patch.<source>.<name>.<field>` and `replace.<name>.<field>` — same lockfile-reflected sub-fields
- For non-virtual workspaces (root has `[package]`): all root-package keys, since the file-based path already attributes them to the root crate

### Build-affecting changes (force_all=true)

- `[profile.*]` — compile flags
- `[lints]`, `[workspace.lints]` — lint levels can flip build success
- `workspace.resolver` — feature unification rules
- `workspace.package.{edition,rust-version,version,links}` — inherited by members
- `cargo-features = [...]` at the top level — unstable Cargo features
- `workspace.target.*` — currently classified as force-all in v1 (rare, complex resolver interactions)
- `workspace.dependencies.<name>.{features,default-features,optional,package}` — feature toggles and renames can change compilation without changing `Cargo.lock`
- Any unrecognized top-level section (defensive default)

## How `base_sha` works

v4 is a breaking change. The action no longer detects changed files itself — you provide them via the `changed_files` input.

| v3 | v4 |
|---|---|
| `base_sha` input (optional) | Removed — compute your own diff |
| Changed files detected automatically | `changed_files` input (**required**) |
| All outputs | Unchanged |

**Before (v3):**
```yaml
- uses: robertrautenbach/rust-affected@v3.1.0
  with:
    base_sha: ${{ github.event.before }}   # always use previous-push diff, even on PRs
```

**After (v4):**
```yaml
- uses: robertrautenbach/rust-affected@v3.1.0
- name: Get changed files
  id: changed
  uses: tj-actions/changed-files@v47

- uses: robertrautenbach/rust-affected@v4.0.2
  with:
    changed_files: ${{ steps.changed.outputs.all_changed_files }}
```
