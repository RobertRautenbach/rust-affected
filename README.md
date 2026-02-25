# rust-affected

A GitHub Action that detects which packages in a Rust workspace are affected by a push, using the Cargo dependency graph.

Given a set of changed files, it determines:
- **`changed_packages`** — packages with files directly modified
- **`affected_packages`** — those packages plus any workspace members that (transitively) depend on them
- **`affected_services`** — the subset of affected packages that are deployable services (binaries under a `services/` directory)
- **`force_all`** — whether a configured force-trigger file changed, meaning the entire workspace should be considered affected

## Usage

```yaml
jobs:
  plan:
    runs-on: ubuntu-latest
    outputs:
      affected_services: ${{ steps.affected.outputs.affected_services }}
      force_all: ${{ steps.affected.outputs.force_all }}
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0

      - name: Detect affected packages
        id: affected
        uses: robertrautenbach/rust-affected@main
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
| `base_sha` | No | The SHA to diff against. Defaults to `github.event.before` (the commit before the push). |
| `force_triggers` | No | Space- or newline-separated list of files or directory prefixes (e.g. `.github/`) that cause `force_all` to be `true` when changed. If omitted, `force_all` is never set. |

## Outputs

| Output | Description |
|---|---|
| `changed_crates` | JSON array of crate names with directly changed files |
| `affected_library_members` | JSON array of all affected workspace members (changed + transitive dependents) |
| `affected_binary_members` | JSON array of affected deployable binaries (packages under `services/`) |
| `force_all` | `"true"` if a force-trigger file changed, otherwise `"false"` |

## How `base_sha` works

- **Direct push to a branch** — `github.event.before` is the SHA that was `HEAD` before the push, giving an exact diff of what changed.
- **Merge commit** — `github.event.before` is the tip of the base branch before the merge, so the diff covers all changes introduced by the merge.
- **First push to a new branch** — `github.event.before` will be all zeros. In this case `tj-actions/changed-files` returns all files, which triggers a full build. You can supply an explicit `base_sha` to override this behaviour.

## Requirements

No special runner setup needed. The action runs `cargo metadata` inside a bundled Docker container — Rust does not need to be installed on the runner.
