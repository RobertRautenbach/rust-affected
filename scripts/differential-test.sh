#!/usr/bin/env bash
# Compare Stage 4 and candidate binaries, including all five observable channels.
# Usage: RUSTUP_TOOLCHAIN=<installed version> scripts/differential-test.sh BASELINE CANDIDATE
set -euo pipefail
if [[ "$#" -ne 2 ]]; then
  echo "usage: $0 BASELINE CANDIDATE" >&2
  exit 2
fi
BASELINE="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
CANDIDATE="$(cd "$(dirname "$2")" && pwd)/$(basename "$2")"
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FIX="$REPO/tests/fixtures"
TMP="$(mktemp -d "${TMPDIR:-/tmp}/rust-affected-differential.XXXXXX")"
trap 'rm -rf "$TMP"' EXIT
export CARGO_NET_OFFLINE=true
export CARGO_CACHE_RUSTC_INFO=0
export CARGO_HOME="$TMP/cargo-home"
mkdir -p "$CARGO_HOME"
CASES=(
  "workspace|lib-utils/src/lib.rs||"
  "workspace|Cargo.lock|Cargo.lock|"
  "workspace|lib-utils/src/lib.rs||tools/"
  "workspace|||"
  "workspace|./lib-utils/src/lib.rs||"
  "workspace|../x|**|"
  "workspace|app-alpha/src/main.rs||tools"
  "workspace|lib-core/src/lib.rs infra/deploy.yml|infra/|app-alpha lib-core-ext"
  "workspace|lib-with-tests/src/lib.rs||"
  "workspace|README.md||"
  "workspace|x|[abc|"
  "workspace|lib-utils/src/lib.rs lib-utils/Cargo.toml lib-utils/Cargo.toml||"
  "root-package|sub-lib/src/lib.rs src/main.rs||"
  "root-package|Cargo.lock|Cargo.lock|"
  "root-package|||"
  "root-package|sub-lib/src/lib.rs||sub-lib"
  "root-package|../bad%path sub-lib/src/lib.rs||unknown"
  "root-package|README.md||"
  "root-package|sub-lib/src/lib.rs||root-app"
  "root-package|independent-lib/src/lib.rs||"
  "nested-package|parent/nested/src/lib.rs||"
  "nested-package|parent/src/lib.rs||"
  "nested-package|parent/independent/src/lib.rs||"
  "nested-package|parent/nested/src/lib.rs||parent/nested"
  "nested-package|Cargo.toml|Cargo.toml|./parent/independent/"
  "name-collision/workspace|member/src/lib.rs||"
  "name-collision/workspace|Cargo.toml|Cargo.toml|"
  "name-collision/workspace|Cargo.toml|Cargo.toml|shared"
  "name-collision/workspace|consumer/src/lib.rs||"
  "name-collision/workspace|member/src/lib.rs||shared"
  "edge-kinds|graph-core/src/lib.rs||"
  "edge-kinds|graph-core/src/lib.rs||normal optional"
  "edge-kinds|Cargo.toml|Cargo.toml|"
)
for fixture in patch-selected patch-config patch-unused bridge; do
  CASES+=("$fixture|core/src/lib.rs||" "$fixture|app/src/main.rs||"
          "$fixture|core/src/lib.rs||resolution-core" "$fixture|Cargo.toml|Cargo.toml|app")
done
CASES+=("bridge|second/src/main.rs||")
record() {
  local BIN="$1" OUT="$2" spec fixture changed force excluded mode dir rc expected
  local -a envs
  n=0
  for spec in "${CASES[@]}"; do
    IFS='|' read -r fixture changed force excluded <<< "$spec"
    for mode in file json; do
      n=$((n + 1))
      dir="$OUT/$(printf '%02d' "$n")-${fixture//\//_}-$mode"
      mkdir -p "$dir"
      printf '%s\n' "$spec" > "$dir/case"
      : > "$dir/output"
      : > "$dir/summary"
      envs=(CHANGED_FILES="$changed" FORCE_TRIGGERS="$force" EXCLUDED_MEMBERS="$excluded"
            GITHUB_STEP_SUMMARY="$dir/summary")
      if [[ "$mode" == file ]]; then
        envs+=(GITHUB_OUTPUT="$dir/output")
      fi
      rc=0
      ( cd "$FIX/$fixture" && env -u GITHUB_OUTPUT "${envs[@]}" "$BIN" >"$dir/stdout" 2>"$dir/stderr" ) || rc=$?
      expected=0
      [[ "$force" != '[abc' ]] || expected=1
      if [[ "$rc" -ne "$expected" ]]; then
        cat "$dir/stderr" >&2
        echo "Unexpected exit $rc (expected $expected): $spec" >&2
        exit 1
      fi
      printf '%s\n' "$rc" > "$dir/rc"
    done
  done
}
record "$BASELINE" "$TMP/baseline"
record "$CANDIDATE" "$TMP/candidate"
diff -ru "$TMP/baseline" "$TMP/candidate"
echo "Differential passed: $n runs per binary, all five channels identical"
