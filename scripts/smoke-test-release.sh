#!/usr/bin/env bash
# Test both platform manifests belonging to a verified release index.
# Usage: scripts/smoke-test-release.sh IMAGE INDEX_DIGEST
set -euo pipefail

IMAGE="${1:?usage: $0 IMAGE INDEX_DIGEST}"
INDEX_DIGEST="${2:?usage: $0 IMAGE INDEX_DIGEST}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
[[ "$INDEX_DIGEST" =~ ^sha256:[0-9a-f]{64}$ ]] || { echo 'Invalid index digest' >&2; exit 1; }

INDEX=$(docker buildx imagetools inspect "${IMAGE}@${INDEX_DIGEST}" --raw)
# Resolve both before pulling anything. OS/architecture excludes provenance
# attestations; arm64's optional v8 variant does not affect selection.
DIGESTS=$(jq -er '
  [.manifests[] | select(.platform.os == "linux")] as $manifests |
  ["amd64", "arm64"] | map(
    . as $arch |
    [$manifests[] | select(.platform.architecture == $arch)] |
    if length != 1 then error("Expected exactly one linux/" + $arch + " manifest")
    else .[0].digest |
      if type != "string" then error("Missing platform digest")
      elif test("^sha256:[0-9a-f]{64}$") then .
      else error("Invalid platform digest") end
    end
  ) | .[]
' <<< "$INDEX")

echo "Smoke-testing release index ${IMAGE}@${INDEX_DIGEST}"
for ARCH in amd64 arm64; do
  DIGEST="${DIGESTS%%$'\n'*}"
  DIGESTS="${DIGESTS#*$'\n'}"
  echo "Testing linux/${ARCH}: ${IMAGE}@${DIGEST}"
  docker pull --platform "linux/${ARCH}" "${IMAGE}@${DIGEST}"
  DOCKER_DEFAULT_PLATFORM="linux/${ARCH}" "${SCRIPT_DIR}/smoke-test.sh" "${IMAGE}@${DIGEST}"
done
