#!/usr/bin/env bash
# Smoke test for the rust-affected Docker image.
#
# Runs the image the way the GitHub Actions runner runs a Docker action: the
# checkout mounted at /github/workspace (here: the fixture workspace), the
# working directory set to it, and the file-command files pre-created and
# mounted at /github/file_commands. Each case compares GITHUB_OUTPUT with the
# expected lines and checks that a job summary was written.
#
# Usage: scripts/smoke-test.sh IMAGE
set -euo pipefail

IMAGE="${1:?usage: $0 IMAGE}"

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORKSPACE="${REPO_ROOT}/tests/fixtures/workspace"
TMP_PARENT="${RUNNER_TEMP:-${TMPDIR:-/tmp}}"

FAILED=0
CASES=0

fail() {
  echo "::error::$1" >&2
  FAILED=1
}

# run_case NAME CHANGED_FILES FORCE_TRIGGERS EXCLUDED_MEMBERS EXPECTED_GITHUB_OUTPUT
run_case() {
  local name="$1" changed="$2" force="$3" excluded="$4" expected="$5"
  local tmp stdout rc=0
  CASES=$((CASES + 1))
  echo "--- case: ${name}"

  tmp="$(mktemp -d "${TMP_PARENT}/rust-affected-smoke.XXXXXX")"
  # The runner pre-creates the file-command files; the binary appends to them
  # and does not create them.
  : > "${tmp}/set_output"
  : > "${tmp}/step_summary"

  stdout="$(docker run --rm \
    --workdir /github/workspace \
    -v "${WORKSPACE}:/github/workspace" \
    -v "${tmp}:/github/file_commands" \
    -e HOME=/github/home \
    -e GITHUB_OUTPUT=/github/file_commands/set_output \
    -e GITHUB_STEP_SUMMARY=/github/file_commands/step_summary \
    -e "CHANGED_FILES=${changed}" \
    -e "FORCE_TRIGGERS=${force}" \
    -e "EXCLUDED_MEMBERS=${excluded}" \
    "${IMAGE}")" || rc=$?

  if [[ "${rc}" -ne 0 ]]; then
    fail "${name}: container exited with status ${rc}"
  fi
  if [[ -n "${stdout}" ]]; then
    fail "${name}: expected empty stdout when GITHUB_OUTPUT is set, got: ${stdout}"
  fi
  if ! diff -u <(printf '%s\n' "${expected}") "${tmp}/set_output"; then
    fail "${name}: GITHUB_OUTPUT differs from the expected lines (diff above)"
  fi
  if ! grep -q '^## rust-affected$' "${tmp}/step_summary"; then
    fail "${name}: GITHUB_STEP_SUMMARY has no '## rust-affected' heading"
  fi
  rm -rf "${tmp}"
}

run_case "leaf library change" \
  "lib-utils/src/lib.rs" "" "" \
'changed_crates=["lib-utils"]
affected_library_members=["lib-core","lib-core-ext","lib-utils"]
affected_binary_members=["app-alpha","app-beta","tool-alpha"]
force_all=false'

run_case "force trigger" \
  "Cargo.lock" "Cargo.lock" "" \
'changed_crates=[]
affected_library_members=["lib-core","lib-core-ext","lib-standalone","lib-utils","lib-with-tests"]
affected_binary_members=["app-alpha","app-beta","tool-alpha"]
force_all=true'

run_case "path-prefix exclusion" \
  "lib-utils/src/lib.rs" "" "tools/" \
'changed_crates=["lib-utils"]
affected_library_members=["lib-core","lib-core-ext","lib-utils"]
affected_binary_members=["app-alpha","app-beta"]
force_all=false'

run_case "no changed files" \
  "" "" "" \
'changed_crates=[]
affected_library_members=[]
affected_binary_members=[]
force_all=false'

# The container runs as root inside the mounted checkout. It must leave the
# fixture workspace untouched: no Cargo.lock rewrite and no stray files.
dirty="$(git -C "${REPO_ROOT}" status --porcelain -- tests/fixtures/workspace)"
if [[ -n "${dirty}" ]]; then
  echo "${dirty}" >&2
  fail "the container modified the fixture workspace (git status above)"
fi

if [[ "${FAILED}" -ne 0 ]]; then
  echo "smoke test FAILED" >&2
  exit 1
fi
echo "smoke test passed (${CASES} cases)"
