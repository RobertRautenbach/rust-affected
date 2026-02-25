# rust-affected

A GitHub Action that detects which packages in a Rust workspace are affected by a push, using the Cargo dependency graph.

Given a set of changed files, it determines:
- **`changed_crates`** — packages with files directly modified
- **`affected_library_members`** — those packages plus any workspace members that (transitively) depend on them
- **`affected_binary_members`** — the subset of affected packages that have a binary target
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
        with:
          fetch-depth: 0

      - name: Detect affected packages
        id: affected
        uses: robertrautenbach/rust-affected@v2.1.4
        with:
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

## Inputs

| Input | Required | Description |
|---|---|---|
| `base_sha` | No | The SHA to diff against. On `pull_request` events defaults to `github.event.pull_request.base.sha` (the base branch tip), so every push to a PR always diffs against main. On `push` events defaults to `github.event.before` (the commit that was HEAD before the push). Override to use any SHA. |
| `force_triggers` | No | Space- or newline-separated list of glob patterns that trigger a full rebuild when any matching file changes. Supports `*`, `**`, and `?`. A bare name (e.g. `Cargo.lock`) matches that exact path only. A trailing slash (e.g. `.github/`) matches the directory and everything inside it. Full globs are also supported (e.g. `**/*.sql`, `migrations/**`). If omitted, `force_all` is never set. |
| `excluded_members` | No | Space- or newline-separated list of workspace member names **or path prefixes** to exclude from all outputs. A plain name (e.g. `my-tool`) matches the crate name directly. An entry containing `/` is matched against the crate's directory relative to the workspace root: a trailing slash (e.g. `tools/`) excludes every crate under that directory, while an exact relative path (e.g. `tools/my-tool`) excludes only that crate. Useful for internal tooling or helper crates that should never appear in CI results. If omitted, no members are excluded. |

## Outputs

| Output | Description |
|---|---|
| `changed_crates` | JSON array of crate names with directly changed files |
| `affected_library_members` | JSON array of all affected workspace members (changed + transitive dependents) |
| `affected_binary_members` | JSON array of affected workspace members that have a binary target |
| `force_all` | `"true"` if a force-trigger file changed, otherwise `"false"` |

## How `base_sha` works

The diff base is chosen automatically depending on the event type:

| Scenario | Default base |
|---|---|
| Push to a PR branch (any push, not just the first) | `github.event.pull_request.base.sha` — the tip of the base branch. Every push to the PR is always diffed against main, so no changes are ever missed across multiple pushes. |
| Direct push to main (or any branch outside a PR) | `github.event.before` — the commit that was HEAD before the push, giving an exact diff of only what landed in this push. |
| First push to a new branch / force-push (null SHA) | Falls back to `git merge-base HEAD origin/main` so the diff covers everything introduced on the branch. |

### Overriding the default

Pass an explicit `base_sha` to compare against any commit:

```yaml
- uses: robertrautenbach/rust-affected@v2.1.4
  with:
    base_sha: ${{ github.event.before }}   # always use previous-push diff, even on PRs
```

```yaml
- uses: robertrautenbach/rust-affected@v2.1.4
  with:
    base_sha: ${{ github.sha }}~1          # always compare to the immediate parent
```

