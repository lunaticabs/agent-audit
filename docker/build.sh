#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if ! docker buildx version >/dev/null 2>&1; then
  echo "docker buildx is not available; install or enable the Docker buildx plugin first." >&2
  exit 1
fi

"${ROOT_DIR}/docker/sync-context.sh"

docker build \
  -f "${ROOT_DIR}/docker/Dockerfile" \
  -t agent-audit-codex \
  "$@" \
  "${ROOT_DIR}"
