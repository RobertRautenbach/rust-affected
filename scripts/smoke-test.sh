#!/usr/bin/env bash
# Smoke test for the rust-affected Docker image.
#
# Runs the image the way the GitHub Actions runner runs a Docker action: the
# checkout mounted at /github/workspace, the working directory set to it, and
# the file-command files pre-created and mounted at /github/file_commands. Each
# case compares GITHUB_OUTPUT with the expected lines and checks that a job
# summary was written.
#
# Usage: scripts/smoke-test.sh IMAGE
set -euo pipefail

IMAGE="${1:?usage: $0 IMAGE}"

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FIXTURE="${REPO_ROOT}/tests/fixtures/workspace"
TMP_PARENT="${RUNNER_TEMP:-${TMPDIR:-/tmp}}"
TMP_PARENT="$(mktemp -d "${TMP_PARENT}/rust-affected-smoke-suite.XXXXXX")"
trap 'rm -rf "${TMP_PARENT}"' EXIT

FAILED=0
CASES=0

fail() {
  echo "::error::$1" >&2
  FAILED=1
}

# snapshot DIR: a deterministic listing of every path under DIR, git-ignored
# ones included, with a checksum for every file. Two snapshots differ when a
# path was added, removed, or had its content changed.
snapshot() {
  find "$1" | LC_ALL=C sort
  find "$1" -type f -exec cksum {} + | LC_ALL=C sort
}

# run_case NAME WORKSPACE CHANGED_FILES FORCE_TRIGGERS EXCLUDED_MEMBERS EXPECTED_GITHUB_OUTPUT [docker run args...]
run_case() {
  local name="$1" workspace="$2" changed="$3" force="$4" excluded="$5" expected="$6"
  shift 6
  local tmp stdout rc=0
  CASES=$((CASES + 1))
  echo "--- case: ${name}"

  tmp="$(mktemp -d "${TMP_PARENT}/rust-affected-smoke.XXXXXX")"
  # The runner pre-creates the file-command files; the binary appends to them
  # and does not create them.
  : > "${tmp}/set_output"
  : > "${tmp}/step_summary"

  stdout="$(docker run --rm --network none \
    --workdir /github/workspace \
    -v "${workspace}:/github/workspace" \
    -v "${tmp}:/github/file_commands" \
    -e HOME=/github/home -e CARGO_HOME=/github/cargo-home -e CARGO_NET_OFFLINE=true \
    -e GITHUB_OUTPUT=/github/file_commands/set_output \
    -e GITHUB_STEP_SUMMARY=/github/file_commands/step_summary \
    -e "CHANGED_FILES=${changed}" \
    -e "FORCE_TRIGGERS=${force}" \
    -e "EXCLUDED_MEMBERS=${excluded}" \
    ${1+"$@"} \
    "${IMAGE}" 2>"${tmp}/stderr")" || rc=$?

  if [[ "${rc}" -ne 0 ]]; then
    cat "${tmp}/stderr" >&2
    fail "${name}: container exited with status ${rc}"
  fi
  if [[ "${stdout}" != "${expected}" ]]; then
    fail "${name}: stdout does not match the expected output lines: ${stdout}"
  fi
  if ! diff -u <(printf '%s\n' "${expected}") "${tmp}/set_output"; then
    fail "${name}: GITHUB_OUTPUT differs from the expected lines (diff above)"
  fi
  if ! grep -q '^## rust-affected$' "${tmp}/step_summary"; then
    fail "${name}: GITHUB_STEP_SUMMARY has no '## rust-affected' heading"
  fi
  local annotations
  annotations="$(sed -n '/^::/p' "${tmp}/stderr")"
  if [[ "${annotations}" != "${EXPECTED_ANNOTATIONS:-}" ]]; then
    cat "${tmp}/stderr" >&2
    fail "${name}: unexpected annotations"
  fi
  rm -rf "${tmp}"
}

# run_failing_case NAME WORKSPACE CHANGED FORCE EXCLUDED ERROR_PATTERN [docker args...]
run_failing_case() {
  local name="$1" workspace="$2" changed="$3" force="$4" excluded="$5" pattern="$6"
  shift 6
  local tmp rc=0
  CASES=$((CASES + 1))
  echo "--- case: ${name}"
  tmp="$(mktemp -d "${TMP_PARENT}/rust-affected-failure.XXXXXX")"
  : > "${tmp}/set_output"
  : > "${tmp}/step_summary"
  docker run --rm --network none --workdir /github/workspace \
    -v "${workspace}:/github/workspace" -v "${tmp}:/github/file_commands" \
    -e HOME=/github/home -e CARGO_HOME=/github/cargo-home -e CARGO_NET_OFFLINE=true -e GITHUB_OUTPUT=/github/file_commands/set_output \
    -e GITHUB_STEP_SUMMARY=/github/file_commands/step_summary \
    -e "CHANGED_FILES=${changed}" -e "FORCE_TRIGGERS=${force}" -e "EXCLUDED_MEMBERS=${excluded}" \
    ${1+"$@"} "${IMAGE}" >"${tmp}/stdout" 2>"${tmp}/stderr" || rc=$?
  if [[ "${rc}" -ne 1 ]] || [[ -s "${tmp}/stdout" ]] || [[ -s "${tmp}/set_output" ]] || [[ -s "${tmp}/step_summary" ]]; then
    cat "${tmp}/stderr" >&2
    fail "${name}: expected exit 1 and untouched outputs; got exit ${rc}"
  fi
  if [[ "$(grep -c '^::error::' "${tmp}/stderr" || true)" -ne 1 ]] || ! grep -Eq "^::error::.*${pattern}" "${tmp}/stderr"; then
    cat "${tmp}/stderr" >&2
    fail "${name}: expected one matching error annotation"
  fi
  local warnings
  warnings="$(sed -n '/^::/p' "${tmp}/stderr" | sed '/^::error::/d')"
  if [[ "${warnings}" != "${EXPECTED_ANNOTATIONS:-}" ]]; then
    cat "${tmp}/stderr" >&2
    fail "${name}: unexpected annotations alongside the error"
  fi
  rm -rf "${tmp}"
}

# Builds a workspace whose only member depends on a git repository and whose
# .cargo/config.toml sets `net.git-fetch-with-cli = true`. With that setting
# cargo fetches git dependencies by spawning the git CLI, so the runtime image
# has to ship git. Sets GITDEP (the dependency repository) and GITWS (the
# workspace).
make_git_dependency_fixture() {
  GITDEP="$(mktemp -d "${TMP_PARENT}/rust-affected-gitdep.XXXXXX")"
  mkdir -p "${GITDEP}/src"
  printf '[package]\nname = "gitdep"\nversion = "0.1.0"\nedition = "2021"\n' > "${GITDEP}/Cargo.toml"
  : > "${GITDEP}/src/lib.rs"
  git -C "${GITDEP}" init -q
  git -C "${GITDEP}" add -A
  git -C "${GITDEP}" -c user.name=smoke -c user.email=smoke@example.com -c commit.gpgsign=false \
    commit -q -m "gitdep"

  GITWS="$(mktemp -d "${TMP_PARENT}/rust-affected-gitws.XXXXXX")"
  mkdir -p "${GITWS}/consumer/src" "${GITWS}/.cargo"
  printf '[workspace]\nmembers = ["consumer"]\nresolver = "2"\n' > "${GITWS}/Cargo.toml"
  printf '[package]\nname = "consumer"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\ngitdep = { git = "file:///github/gitdep" }\n' \
    > "${GITWS}/consumer/Cargo.toml"
  : > "${GITWS}/consumer/src/lib.rs"
  printf '[net]\ngit-fetch-with-cli = true\n' > "${GITWS}/.cargo/config.toml"
}

fixture_before="$(snapshot "${REPO_ROOT}/tests/fixtures")"
EMPTY_WARNING='::warning::changed_files is empty; no files will be attributed. If files did change, check the step producing changed_files. Workspace and configuration validation still run.'

LOCK_WARNING='::warning::No Cargo.lock in the workspace root; cargo metadata will resolve dependencies and may create one (owned by root inside the action). Commit Cargo.lock to use --locked.'

# This must name an installed toolchain, not merely happen to work online.
docker run --rm --network none --entrypoint sh "${IMAGE}" -c \
  'test "$RUSTUP_TOOLCHAIN" = "$RUST_VERSION" && test "$RUSTUP_AUTO_INSTALL" = 0 && rustup run "$RUSTUP_TOOLCHAIN" cargo --version && git --version'

run_case "leaf library change" "${FIXTURE}" \
  "lib-utils/src/lib.rs" "" "" \
'changed_crates=["lib-utils"]
affected_library_members=["lib-core","lib-core-ext","lib-utils"]
affected_binary_members=["app-alpha","app-beta","tool-alpha"]
force_all=false'

run_case "force trigger" "${FIXTURE}" \
  "Cargo.lock" "Cargo.lock" "" \
'changed_crates=[]
affected_library_members=["lib-core","lib-core-ext","lib-standalone","lib-utils","lib-with-tests"]
affected_binary_members=["app-alpha","app-beta","tool-alpha"]
force_all=true'

run_case "path-prefix exclusion" "${FIXTURE}" \
  "lib-utils/src/lib.rs" "" "tools/" \
'changed_crates=["lib-utils"]
affected_library_members=["lib-core","lib-core-ext","lib-utils"]
affected_binary_members=["app-alpha","app-beta"]
force_all=false'

EXPECTED_ANNOTATIONS="${EMPTY_WARNING}" run_case "no changed files" "${FIXTURE}" \
  "" "" "" \
'changed_crates=[]
affected_library_members=[]
affected_binary_members=[]
force_all=false'

run_case "root member ownership" "${REPO_ROOT}/tests/fixtures/root-package" \
  "sub-lib/src/lib.rs" "" "" \
'changed_crates=["sub-lib"]
affected_library_members=["sub-lib"]
affected_binary_members=["root-app"]
force_all=false'

run_case "independent nested member" "${REPO_ROOT}/tests/fixtures/nested-package" \
  "parent/independent/src/lib.rs" "" "" \
'changed_crates=["independent"]
affected_library_members=["independent"]
affected_binary_members=[]
force_all=false'

run_case "normalized changed path" "${FIXTURE}" \
  "./lib-utils/src/lib.rs" "" "" \
'changed_crates=["lib-utils"]
affected_library_members=["lib-core","lib-core-ext","lib-utils"]
affected_binary_members=["app-alpha","app-beta","tool-alpha"]
force_all=false'

EMPTY_WS="$(mktemp -d "${TMP_PARENT}/rust-affected-empty.XXXXXX")"
run_failing_case "missing workspace" "${EMPTY_WS}" "x" "" "" 'could not find'
EXPECTED_ANNOTATIONS="${EMPTY_WARNING}" run_failing_case "empty missing workspace" "${EMPTY_WS}" "" "" "" 'could not find'
rm -rf "${EMPTY_WS}"
run_failing_case "invalid glob" "${FIXTURE}" "x" "[abc" "" 'invalid force_triggers pattern'
EXPECTED_ANNOTATIONS="${EMPTY_WARNING}" run_failing_case "empty invalid glob" "${FIXTURE}" "" "[abc" "" 'invalid force_triggers pattern'

EXPECTED_ANNOTATIONS='::warning::excluded_members entries matched no workspace member: ["tools"]. Names match crate names exactly; use a trailing slash such as '"'"'tools/'"'"' to match a directory.' \
run_case "unmatched exclusion" "${FIXTURE}" "app-alpha/src/main.rs" "" "tools" \
'changed_crates=["app-alpha"]
affected_library_members=[]
affected_binary_members=["app-alpha"]
force_all=false'

EXPECTED_ANNOTATIONS='::warning::1 changed file(s) were ignored for attribution and force triggers: ["../x"]. Use paths relative to the workspace root or absolute paths beneath it; empty paths and '"'"'..'"'"' are not supported.' \
run_case "unusable path cannot force" "${FIXTURE}" "../x" "**" "" \
'changed_crates=[]
affected_library_members=[]
affected_binary_members=[]
force_all=false'

copy_fixture() {
  mkdir -p "$1"
  tar -C "${FIXTURE}" --exclude=target -cf - . | tar -C "$1" -xf -
}

LOCK_WS="${TMP_PARENT}/lock-workspace"
copy_fixture "${LOCK_WS}"
printf 'version = 4\n' > "${LOCK_WS}/Cargo.lock"
lock_before="$(snapshot "${LOCK_WS}")"
run_failing_case "stale lock" "${LOCK_WS}" "lib-utils/src/lib.rs" "" "" 'lock file' --workdir /github/workspace/app-alpha
EXPECTED_ANNOTATIONS="${EMPTY_WARNING}" run_failing_case "empty input with stale lock" "${LOCK_WS}" "" "" "" 'lock file'
if [[ "${lock_before}" != "$(snapshot "${LOCK_WS}")" ]]; then
  fail "stale lock failures modified the workspace"
fi
# Use a fresh mount: Docker Desktop can retain stale stat data after host deletion.
MISSING_LOCK_WS="${TMP_PARENT}/missing-lock-workspace"
copy_fixture "${MISSING_LOCK_WS}"
rm "${MISSING_LOCK_WS}/Cargo.lock"
missing_before="$(snapshot "${MISSING_LOCK_WS}")"
EXPECTED_ANNOTATIONS="${LOCK_WARNING}" run_case "missing lock" "${MISSING_LOCK_WS}" "app-alpha/src/main.rs" "" "" \
'changed_crates=["app-alpha"]
affected_library_members=[]
affected_binary_members=["app-alpha"]
force_all=false'
[[ -f "${MISSING_LOCK_WS}/Cargo.lock" ]] || fail "missing-lock fallback did not create a lockfile"
mv "${MISSING_LOCK_WS}/Cargo.lock" "${TMP_PARENT}/created.lock"
[[ "${missing_before}" = "$(snapshot "${MISSING_LOCK_WS}")" ]] || fail "missing-lock fallback changed more than Cargo.lock"
EMPTY_LOCK_WS="${TMP_PARENT}/empty-missing-lock-workspace"
copy_fixture "${EMPTY_LOCK_WS}"
rm "${EMPTY_LOCK_WS}/Cargo.lock"
empty_before="$(snapshot "${EMPTY_LOCK_WS}")"
EXPECTED_ANNOTATIONS="${EMPTY_WARNING}"$'\n'"${LOCK_WARNING}" run_case "empty input with missing lock" "${EMPTY_LOCK_WS}" "" "" "" \
'changed_crates=[]
affected_library_members=[]
affected_binary_members=[]
force_all=false' --workdir /github/workspace/app-alpha
[[ -f "${EMPTY_LOCK_WS}/Cargo.lock" ]] || fail "empty input did not create a lockfile"
mv "${EMPTY_LOCK_WS}/Cargo.lock" "${TMP_PARENT}/empty-created.lock"
[[ "${empty_before}" = "$(snapshot "${EMPTY_LOCK_WS}")" ]] || fail "empty fallback changed more than Cargo.lock"

MALFORMED_WS="${TMP_PARENT}/malformed-lock-workspace"
copy_fixture "${MALFORMED_WS}"
printf 'not TOML\n' > "${MALFORMED_WS}/Cargo.lock"
malformed_before="$(snapshot "${MALFORMED_WS}")"
run_failing_case "malformed lock" "${MALFORMED_WS}" "app-alpha/src/main.rs" "" "" 'cargo metadata failed'
[[ "${malformed_before}" = "$(snapshot "${MALFORMED_WS}")" ]] || fail "malformed-lock failure changed workspace"

for fixture in patch-selected patch-config patch-unused bridge; do
  binaries='["app"]'
  case "${fixture}" in
    patch-unused) binaries='[]' ;;
    bridge) binaries='["app","second"]' ;;
  esac
  run_case "resolved ${fixture}" "${REPO_ROOT}/tests/fixtures/${fixture}" "core/src/lib.rs" "" "" \
"changed_crates=[\"resolution-core\"]
affected_library_members=[\"resolution-core\"]
affected_binary_members=${binaries}
force_all=false"
done

TOOLCHAIN_WS="${TMP_PARENT}/toolchain-workspace"
copy_fixture "${TOOLCHAIN_WS}"
printf '[toolchain]\nchannel = "1.80.0"\n' > "${TOOLCHAIN_WS}/rust-toolchain.toml"
# Include an existing cache so the snapshot detects even metadata-only writes.
mkdir -p "${TOOLCHAIN_WS}/target"
printf '{"rustc_fingerprint":0,"outputs":{},"successes":{}}' > "${TOOLCHAIN_WS}/target/.rustc_info.json"
toolchain_before="$(snapshot "${TOOLCHAIN_WS}")"
run_case "consumer toolchain ignored offline" "${TOOLCHAIN_WS}" "lib-utils/src/lib.rs" "" "" \
'changed_crates=["lib-utils"]
affected_library_members=["lib-core","lib-core-ext","lib-utils"]
affected_binary_members=["app-alpha","app-beta","tool-alpha"]
force_all=false'
run_failing_case "unpinned toolchain fails without installing" "${TOOLCHAIN_WS}" "lib-utils/src/lib.rs" "" "" 'is not installed' \
  -e RUSTUP_TOOLCHAIN=
if [[ "${toolchain_before}" != "$(snapshot "${TOOLCHAIN_WS}")" ]]; then
  fail "toolchain cases modified the workspace"
fi

# The dependency repository is owned by the host user while the container runs
# as root, and git refuses to serve a repository owned by someone else unless it
# is marked safe. Consumers fetch remote repositories, so this is a test-only
# concern; the GIT_CONFIG_* variables mark every directory safe for this run.
make_git_dependency_fixture
EXPECTED_ANNOTATIONS="${LOCK_WARNING}" run_case "git dependency fetched with the git CLI" "${GITWS}" \
  "consumer/src/lib.rs" "" "" \
'changed_crates=["consumer"]
affected_library_members=["consumer"]
affected_binary_members=[]
force_all=false' \
  -e CARGO_NET_OFFLINE=false -v "${GITDEP}:/github/gitdep:ro" \
  -e GIT_CONFIG_COUNT=1 -e GIT_CONFIG_KEY_0=safe.directory -e GIT_CONFIG_VALUE_0='*'
rm -rf "${GITDEP}" "${GITWS}" || echo "warning: could not remove ${GITDEP} ${GITWS}" >&2

# The container runs as root inside the mounted checkout. It must leave the
# fixture workspace exactly as it found it: no Cargo.lock rewrite and no stray
# files, including under git-ignored paths such as target/.
fixture_after="$(snapshot "${REPO_ROOT}/tests/fixtures")"
if [[ "${fixture_before}" != "${fixture_after}" ]]; then
  diff -u <(printf '%s\n' "${fixture_before}") <(printf '%s\n' "${fixture_after}") >&2 || true
  fail "the container modified the fixture workspace (snapshot diff above)"
fi

if [[ "${FAILED}" -ne 0 ]]; then
  echo "smoke test FAILED" >&2
  exit 1
fi
echo "smoke test passed (${CASES} cases)"
