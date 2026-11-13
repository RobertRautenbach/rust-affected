#!/usr/bin/env bash
# Smoke test for the rust-affected Docker image.
#
# Runs the image the way the GitHub Actions runner runs a Docker action: the
# checkout mounted at /github/workspace, the working directory set to it, and
# the file-command files pre-created and mounted at /github/file_commands, and
# every declared input passed as INPUT_<NAME> (trust_cargo_config defaults to
# false, as in action.yml). Each
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

# run_case NAME WORKSPACE CHANGED_FILES_JSON FORCE_TRIGGERS EXCLUDED_PACKAGES EXPECTED_GITHUB_OUTPUT [docker run args...]
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
    -e "INPUT_CHANGED_FILES_JSON=${changed}" \
    -e "INPUT_FORCE_TRIGGERS=${force}" \
    -e "INPUT_EXCLUDED_PACKAGES=${excluded}" \
    -e INPUT_TRUST_CARGO_CONFIG=false \
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

# run_failing_case NAME WORKSPACE CHANGED_FILES_JSON FORCE EXCLUDED ERROR_PATTERN [docker args...]
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
    -e "INPUT_CHANGED_FILES_JSON=${changed}" -e "INPUT_FORCE_TRIGGERS=${force}" -e "INPUT_EXCLUDED_PACKAGES=${excluded}" \
    -e INPUT_TRUST_CARGO_CONFIG=false \
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
EMPTY_WARNING='::warning::changed_files_json is empty; no files will be attributed. If files did change, check the step producing changed_files_json. Workspace and configuration validation still run.'

LOCK_WARNING='::warning::No Cargo.lock in the workspace root; cargo metadata will resolve dependencies and may create one (owned by root inside the action). Commit Cargo.lock to use --locked.'

# This must name an installed toolchain, not merely happen to work online.
docker run --rm --network none --entrypoint sh "${IMAGE}" -c \
  'test -n "$RUSTUP_TOOLCHAIN" && test "$RUSTUP_AUTO_INSTALL" = 0 && rustup run "$RUSTUP_TOOLCHAIN" cargo --version && git --version'

run_case "leaf library change" "${FIXTURE}" \
  '["lib-utils/src/lib.rs"]' "" "" \
'changed_packages=["lib-utils"]
affected_libraries=["lib-core","lib-core-ext","lib-utils"]
affected_binaries=["app-alpha","app-beta","tool-alpha"]
force_all=false'

run_case "force trigger" "${FIXTURE}" \
  '["Cargo.lock"]' "Cargo.lock" "" \
'changed_packages=[]
affected_libraries=["lib-core","lib-core-ext","lib-standalone","lib-utils","lib-with-tests"]
affected_binaries=["app-alpha","app-beta","tool-alpha"]
force_all=true'

run_case "directory-only force trigger" "${FIXTURE}" \
  '["infra/"]' "infra/" "" \
'changed_packages=[]
affected_libraries=["lib-core","lib-core-ext","lib-standalone","lib-utils","lib-with-tests"]
affected_binaries=["app-alpha","app-beta","tool-alpha"]
force_all=true'

run_case "path-prefix exclusion" "${FIXTURE}" \
  '["lib-utils/src/lib.rs"]' "" "tools/" \
'changed_packages=["lib-utils"]
affected_libraries=["lib-core","lib-core-ext","lib-utils"]
affected_binaries=["app-alpha","app-beta"]
force_all=false'

EXPECTED_ANNOTATIONS="${EMPTY_WARNING}" run_case "no changed files" "${FIXTURE}" \
  '[]' "" "" \
'changed_packages=[]
affected_libraries=[]
affected_binaries=[]
force_all=false'

run_case "root member ownership" "${REPO_ROOT}/tests/fixtures/root-package" \
  '["sub-lib/src/lib.rs"]' "" "" \
'changed_packages=["sub-lib"]
affected_libraries=["sub-lib"]
affected_binaries=["root-app"]
force_all=false'

run_case "independent nested member" "${REPO_ROOT}/tests/fixtures/nested-package" \
  '["parent/independent/src/lib.rs"]' "" "" \
'changed_packages=["independent"]
affected_libraries=["independent"]
affected_binaries=[]
force_all=false'

run_case "normalized changed path" "${FIXTURE}" \
  '["./lib-utils/src/lib.rs"]' "" "" \
'changed_packages=["lib-utils"]
affected_libraries=["lib-core","lib-core-ext","lib-utils"]
affected_binaries=["app-alpha","app-beta","tool-alpha"]
force_all=false'

EMPTY_WS="$(mktemp -d "${TMP_PARENT}/rust-affected-empty.XXXXXX")"
run_failing_case "missing workspace" "${EMPTY_WS}" '["x"]' "" "" 'could not find'
EXPECTED_ANNOTATIONS="${EMPTY_WARNING}" run_failing_case "empty missing workspace" "${EMPTY_WS}" '[]' "" "" 'could not find'
rm -rf "${EMPTY_WS}"
run_failing_case "invalid glob" "${FIXTURE}" '["x"]' "[abc" "" 'invalid force_triggers pattern'
EXPECTED_ANNOTATIONS="${EMPTY_WARNING}" run_failing_case "empty invalid glob" "${FIXTURE}" '[]' "[abc" "" 'invalid force_triggers pattern'

EXPECTED_ANNOTATIONS='::warning::excluded_packages entries matched no workspace package: ["tools"]. Names match package names exactly; use a trailing slash such as '"'"'tools/'"'"' to match a directory.' \
run_case "unmatched exclusion" "${FIXTURE}" '["app-alpha/src/main.rs"]' "" "tools" \
'changed_packages=["app-alpha"]
affected_libraries=[]
affected_binaries=["app-alpha"]
force_all=false'

EXPECTED_ANNOTATIONS='::warning::1 changed file(s) were ignored for attribution and force triggers: ["../x"]. Use paths relative to the workspace root or absolute paths beneath it; empty paths and '"'"'..'"'"' are not supported.' \
run_case "unusable path cannot force" "${FIXTURE}" '["../x"]' "**" "" \
'changed_packages=[]
affected_libraries=[]
affected_binaries=[]
force_all=false'

# The list is required and must be a JSON array of strings. The input removed
# in v5 is not read, so an unmigrated workflow fails instead of selecting nothing.
run_failing_case "missing changed_files_json" "${FIXTURE}" "" "" "" 'changed_files_json is required'
run_failing_case "removed CHANGED_FILES variable is unread" "${FIXTURE}" "" "" "" 'changed_files_json is required' \
  -e CHANGED_FILES=lib-utils/src/lib.rs
run_failing_case "changed_files_json is not JSON" "${FIXTURE}" "lib-utils/src/lib.rs" "" "" 'must be a JSON array of strings'
run_failing_case "changed_files_json is not an array of strings" "${FIXTURE}" '["lib-utils/src/lib.rs",1]' "" "" 'must be a JSON array of strings'

# The runner passes every `with:` key as INPUT_<NAME>, declared or not, so a v4
# workflow's removed inputs must fail instead of being ignored.
run_failing_case "removed excluded_members input" "${FIXTURE}" '["lib-utils/src/lib.rs"]' "" "" \
  'inputs removed in v5 were passed: excluded_members \(renamed to excluded_packages\)' \
  -e INPUT_EXCLUDED_MEMBERS=tools/
run_failing_case "removed ignore_cargo_config input" "${FIXTURE}" '["lib-utils/src/lib.rs"]' "" "" \
  'inputs removed in v5 were passed: ignore_cargo_config' \
  -e INPUT_IGNORE_CARGO_CONFIG=false

run_failing_case "unmatchable trigger" "${FIXTURE}" '["x"]' "/Cargo.lock" "" 'can never match'

run_case "path with a space and non-ASCII characters" "${FIXTURE}" \
  '["lib-utils/src/my café file.rs"]' "" "" \
'changed_packages=["lib-utils"]
affected_libraries=["lib-core","lib-core-ext","lib-utils"]
affected_binaries=["app-alpha","app-beta","tool-alpha"]
force_all=false'

EXPECTED_ANNOTATIONS='::warning::workspace-wide files changed but belong to no package and matched no force trigger, so nothing was selected for them: ["Cargo.lock"]. They can affect every package; add them to force_triggers.' \
run_case "uncovered workspace-wide file" "${FIXTURE}" '["Cargo.lock"]' "" "" \
'changed_packages=[]
affected_libraries=[]
affected_binaries=[]
force_all=false'

run_case "non-member path dependency owns its files" "${REPO_ROOT}/tests/fixtures/bridge" \
  '["external/adapter/src/lib.rs"]' "" "" \
'changed_packages=[]
affected_libraries=[]
affected_binaries=["app"]
force_all=false'

copy_fixture() {
  mkdir -p "$1"
  tar -C "${FIXTURE}" --exclude=target -cf - . | tar -C "$1" -xf -
}

LOCK_WS="${TMP_PARENT}/lock-workspace"
copy_fixture "${LOCK_WS}"
printf 'version = 4\n' > "${LOCK_WS}/Cargo.lock"
lock_before="$(snapshot "${LOCK_WS}")"
run_failing_case "stale lock" "${LOCK_WS}" '["lib-utils/src/lib.rs"]' "" "" 'lock file' --workdir /github/workspace/app-alpha
EXPECTED_ANNOTATIONS="${EMPTY_WARNING}" run_failing_case "empty input with stale lock" "${LOCK_WS}" '[]' "" "" 'lock file'
if [[ "${lock_before}" != "$(snapshot "${LOCK_WS}")" ]]; then
  fail "stale lock failures modified the workspace"
fi
# Use a fresh mount: Docker Desktop can retain stale stat data after host deletion.
MISSING_LOCK_WS="${TMP_PARENT}/missing-lock-workspace"
copy_fixture "${MISSING_LOCK_WS}"
rm "${MISSING_LOCK_WS}/Cargo.lock"
missing_before="$(snapshot "${MISSING_LOCK_WS}")"
EXPECTED_ANNOTATIONS="${LOCK_WARNING}" run_case "missing lock" "${MISSING_LOCK_WS}" '["app-alpha/src/main.rs"]' "" "" \
'changed_packages=["app-alpha"]
affected_libraries=[]
affected_binaries=["app-alpha"]
force_all=false'
[[ -f "${MISSING_LOCK_WS}/Cargo.lock" ]] || fail "missing-lock fallback did not create a lockfile"
mv "${MISSING_LOCK_WS}/Cargo.lock" "${TMP_PARENT}/created.lock"
[[ "${missing_before}" = "$(snapshot "${MISSING_LOCK_WS}")" ]] || fail "missing-lock fallback changed more than Cargo.lock"
EMPTY_LOCK_WS="${TMP_PARENT}/empty-missing-lock-workspace"
copy_fixture "${EMPTY_LOCK_WS}"
rm "${EMPTY_LOCK_WS}/Cargo.lock"
empty_before="$(snapshot "${EMPTY_LOCK_WS}")"
EXPECTED_ANNOTATIONS="${EMPTY_WARNING}"$'\n'"${LOCK_WARNING}" run_case "empty input with missing lock" "${EMPTY_LOCK_WS}" '[]' "" "" \
'changed_packages=[]
affected_libraries=[]
affected_binaries=[]
force_all=false' --workdir /github/workspace/app-alpha
[[ -f "${EMPTY_LOCK_WS}/Cargo.lock" ]] || fail "empty input did not create a lockfile"
mv "${EMPTY_LOCK_WS}/Cargo.lock" "${TMP_PARENT}/empty-created.lock"
[[ "${empty_before}" = "$(snapshot "${EMPTY_LOCK_WS}")" ]] || fail "empty fallback changed more than Cargo.lock"

MALFORMED_WS="${TMP_PARENT}/malformed-lock-workspace"
copy_fixture "${MALFORMED_WS}"
printf 'not TOML\n' > "${MALFORMED_WS}/Cargo.lock"
malformed_before="$(snapshot "${MALFORMED_WS}")"
run_failing_case "malformed lock" "${MALFORMED_WS}" '["app-alpha/src/main.rs"]' "" "" 'cargo metadata failed'
[[ "${malformed_before}" = "$(snapshot "${MALFORMED_WS}")" ]] || fail "malformed-lock failure changed workspace"

for fixture in patch-selected patch-config patch-unused bridge; do
  binaries='["app"]'
  case "${fixture}" in
    patch-unused) binaries='[]' ;;
    bridge) binaries='["app","second"]' ;;
  esac
  # patch-config keeps its [patch] in .cargo/config.toml, which is read only when trusted.
  trust=false
  [[ "${fixture}" != patch-config ]] || trust=true
  run_case "resolved ${fixture}" "${REPO_ROOT}/tests/fixtures/${fixture}" '["core/src/lib.rs"]' "" "" \
"changed_packages=[\"resolution-core\"]
affected_libraries=[\"resolution-core\"]
affected_binaries=${binaries}
force_all=false" -e "INPUT_TRUST_CARGO_CONFIG=${trust}"
done
# Untrusted, that [patch] is unread, the lock is stale, and the error names the file.
run_failing_case "unread config is named when cargo fails" "${REPO_ROOT}/tests/fixtures/patch-config" \
  '["core/src/lib.rs"]' "" "" '\.cargo/config\.toml was not read because trust_cargo_config is false'

TOOLCHAIN_WS="${TMP_PARENT}/toolchain-workspace"
copy_fixture "${TOOLCHAIN_WS}"
printf '[toolchain]\nchannel = "1.80.0"\n' > "${TOOLCHAIN_WS}/rust-toolchain.toml"
# Include an existing cache so the snapshot detects even metadata-only writes.
mkdir -p "${TOOLCHAIN_WS}/target"
printf '{"rustc_fingerprint":0,"outputs":{},"successes":{}}' > "${TOOLCHAIN_WS}/target/.rustc_info.json"
toolchain_before="$(snapshot "${TOOLCHAIN_WS}")"
run_case "consumer toolchain ignored offline" "${TOOLCHAIN_WS}" '["lib-utils/src/lib.rs"]' "" "" \
'changed_packages=["lib-utils"]
affected_libraries=["lib-core","lib-core-ext","lib-utils"]
affected_binaries=["app-alpha","app-beta","tool-alpha"]
force_all=false' \
  -e INPUT_TRUST_CARGO_CONFIG=true
run_failing_case "unpinned toolchain fails without installing" "${TOOLCHAIN_WS}" '["lib-utils/src/lib.rs"]' "" "" 'is not installed' \
  -e RUSTUP_TOOLCHAIN= -e INPUT_TRUST_CARGO_CONFIG=true
# Started outside the checkout, rustup cannot see the toolchain file either.
run_case "toolchain file unread when the checkout is untrusted" "${TOOLCHAIN_WS}" '["lib-utils/src/lib.rs"]' "" "" \
'changed_packages=["lib-utils"]
affected_libraries=["lib-core","lib-core-ext","lib-utils"]
affected_binaries=["app-alpha","app-beta","tool-alpha"]
force_all=false' \
  -e RUSTUP_TOOLCHAIN=
if [[ "${toolchain_before}" != "$(snapshot "${TOOLCHAIN_WS}")" ]]; then
  fail "toolchain cases modified the workspace"
fi

# A checkout's .cargo/config.toml can point Cargo at any program, and Cargo runs
# it during detection. It may run only when trust_cargo_config says so.
CONFIG_WS="${TMP_PARENT}/config-workspace"
copy_fixture "${CONFIG_WS}"
mkdir -p "${CONFIG_WS}/.cargo"
printf '#!/bin/sh\ntouch /github/workspace/EXECUTED\nexec rustc "$@"\n' > "${CONFIG_WS}/planted-rustc"
chmod +x "${CONFIG_WS}/planted-rustc"
printf '[build]\nrustc = "/github/workspace/planted-rustc"\n' > "${CONFIG_WS}/.cargo/config.toml"
CONFIG_EXPECTED='changed_packages=["lib-utils"]
affected_libraries=["lib-core","lib-core-ext","lib-utils"]
affected_binaries=["app-alpha","app-beta","tool-alpha"]
force_all=false'
run_case "repository cargo config honoured when trusted" "${CONFIG_WS}" '["lib-utils/src/lib.rs"]' "" "" "${CONFIG_EXPECTED}" \
  -e INPUT_TRUST_CARGO_CONFIG=true
# Without this, the cases below would pass even if the isolation did nothing.
[[ -f "${CONFIG_WS}/EXECUTED" ]] || fail "the planted program did not run in a trusted checkout"
rm -f "${CONFIG_WS}/EXECUTED"
config_before="$(snapshot "${CONFIG_WS}")"
run_case "repository cargo config unread by default" "${CONFIG_WS}" '["lib-utils/src/lib.rs"]' "" "" "${CONFIG_EXPECTED}"
run_case "repository cargo config unread from a member" "${CONFIG_WS}" '["lib-utils/src/lib.rs"]' "" "" "${CONFIG_EXPECTED}" \
  --workdir /github/workspace/app-alpha
# The switch removed in v5 must not turn the isolation off.
run_case "removed IGNORE_CARGO_CONFIG variable is unread" "${CONFIG_WS}" '["lib-utils/src/lib.rs"]' "" "" "${CONFIG_EXPECTED}" \
  -e IGNORE_CARGO_CONFIG=false
# A variable the job defines under an input's bare name must not replace the input.
run_case "job variable cannot replace the trust input" "${CONFIG_WS}" '["lib-utils/src/lib.rs"]' "" "" "${CONFIG_EXPECTED}" \
  -e TRUST_CARGO_CONFIG=true
# Cargo reads $CARGO_HOME/config.toml wherever it starts, so a home inside an
# untrusted checkout is refused before Cargo runs.
run_failing_case "CARGO_HOME inside an untrusted checkout" "${CONFIG_WS}" '["lib-utils/src/lib.rs"]' "" "" \
  'CARGO_HOME \(/github/workspace/\.cargo\) is inside the checkout' \
  -e CARGO_HOME=/github/workspace/.cargo
# rustup runs cargo and rustc from $RUSTUP_HOME/toolchains, so that home is
# refused there as well. Had rustup started, it would have created the directory.
run_failing_case "RUSTUP_HOME inside an untrusted checkout" "${CONFIG_WS}" '["lib-utils/src/lib.rs"]' "" "" \
  'RUSTUP_HOME \(/github/workspace/\.rustup\) is inside the checkout' \
  -e RUSTUP_HOME=/github/workspace/.rustup
[[ "${config_before}" = "$(snapshot "${CONFIG_WS}")" ]] || fail "an untrusted checkout still ran the planted program or changed the workspace"
run_failing_case "invalid trust_cargo_config" "${FIXTURE}" '["x"]' "" "" "TRUST_CARGO_CONFIG must be" -e INPUT_TRUST_CARGO_CONFIG=yes

# The dependency repository is owned by the host user while the container runs
# as root, and git refuses to serve a repository owned by someone else unless it
# is marked safe. Consumers fetch remote repositories, so this is a test-only
# concern; the GIT_CONFIG_* variables mark every directory safe for this run.
make_git_dependency_fixture
EXPECTED_ANNOTATIONS="${LOCK_WARNING}" run_case "git dependency fetched with the git CLI" "${GITWS}" \
  '["consumer/src/lib.rs"]' "" "" \
'changed_packages=["consumer"]
affected_libraries=["consumer"]
affected_binaries=[]
force_all=false' \
  -e CARGO_NET_OFFLINE=false -e INPUT_TRUST_CARGO_CONFIG=true -v "${GITDEP}:/github/gitdep:ro" \
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
