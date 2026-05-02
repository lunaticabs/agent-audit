#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

"${ROOT_DIR}/docker/sync-context.sh"

docker buildx build \
  --load \
  -f "${ROOT_DIR}/docker/Dockerfile" \
  -t agent-audit-codex \
  "${ROOT_DIR}"
