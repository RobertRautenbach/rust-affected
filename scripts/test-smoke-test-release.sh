#!/usr/bin/env bash
# Focused selection/failure tests. Real Docker coverage lives in CI.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
cp "$ROOT/smoke-test-release.sh" "$TMP/"
export PATH="$TMP:$PATH"
export CALLS="$TMP/calls" MANIFEST="$TMP/manifest.json"
export FAIL_AT=''
INDEX="sha256:$(printf '%064d' 1)"
AMD="sha256:$(printf '%064d' 2)"
ARM="sha256:$(printf '%064d' 3)"
export INDEX AMD ARM
cat > "$TMP/docker" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
echo "docker $*" >> "$CALLS"
if [[ "$1" == buildx ]]; then
  [[ "$FAIL_AT" != inspect ]] || exit 17
  cat "$MANIFEST"
elif [[ "$1" == pull ]]; then
  [[ "$FAIL_AT" != "pull-$3" ]] || exit 18
else
  exit 99
fi
STUB
cat > "$TMP/smoke-test.sh" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
echo "smoke $DOCKER_DEFAULT_PLATFORM $*" >> "$CALLS"
[[ "$FAIL_AT" != "smoke-$DOCKER_DEFAULT_PLATFORM" ]] || exit 19
STUB
chmod +x "$TMP/docker" "$TMP/smoke-test.sh"

fixture() {
  jq -n --arg amd "$AMD" --arg arm "$ARM" '{manifests: [
    {digest: $arm, platform: {os: "linux", architecture: "arm64", variant: "v8"}},
    {digest: "attestation", platform: {os: "unknown", architecture: "unknown"}},
    {digest: $amd, platform: {os: "linux", architecture: "amd64"}}
  ]}' > "$MANIFEST"
}
run_helper() {
  : > "$CALLS"
  "$TMP/smoke-test-release.sh" example/image "$INDEX" > "$TMP/output" 2>&1
}
expect_failure() {
  local expected="$1" status=0
  run_helper || status=$?
  if [[ "$status" == 0 || "$(wc -l < "$CALLS" | tr -d ' ')" != "$expected" ]]; then
    cat "$TMP/output" "$CALLS" >&2
    echo 'Unexpected success or subsequent work after failure' >&2
    exit 1
  fi
}
fixture
run_helper
cat > "$TMP/expected" <<EXPECTED
docker buildx imagetools inspect example/image@$INDEX --raw
docker pull --platform linux/amd64 example/image@$AMD
smoke linux/amd64 example/image@$AMD
docker pull --platform linux/arm64 example/image@$ARM
smoke linux/arm64 example/image@$ARM
EXPECTED
diff -u "$TMP/expected" "$CALLS"

# Every invalid selection must fail before either platform is pulled.
for filter in \
  'del(.manifests[0])' \
  '.manifests += [.manifests[0]]' \
  '.manifests[0].digest = "sha256:bad"' \
  'del(.manifests[0].digest)' \
  '.manifests[0].digest = 42' \
  '.manifests[0].platform.os = "windows"' \
  'del(.manifests)'; do
  fixture
  jq "$filter" "$MANIFEST" > "$TMP/edited"
  mv "$TMP/edited" "$MANIFEST"
  expect_failure 1
done
printf 'invalid json\n' > "$MANIFEST"
expect_failure 1
fixture
for FAIL_AT in inspect pull-linux/amd64 smoke-linux/amd64 pull-linux/arm64 smoke-linux/arm64; do
  case "$FAIL_AT" in
    inspect) count=1 ;;
    pull-linux/amd64) count=2 ;;
    smoke-linux/amd64) count=3 ;;
    pull-linux/arm64) count=4 ;;
    smoke-linux/arm64) count=5 ;;
  esac
  expect_failure "$count"
done
FAIL_AT=''
INDEX=invalid
expect_failure 0
echo 'Release helper selection and failure tests passed'
