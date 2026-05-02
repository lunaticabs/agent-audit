#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DOCKER_DIR="${ROOT_DIR}/docker"

# if ! docker buildx version >/dev/null 2>&1; then
#   echo "docker buildx is not available; install the Docker buildx CLI plugin first (for official Docker packages on Ubuntu, this is usually docker-buildx-plugin)." >&2
#   exit 1
# fi

"${DOCKER_DIR}/sync-context.sh"

docker build \
  -f "${DOCKER_DIR}/Dockerfile" \
  -t agent-audit-codex \
  "$@" \
  "${DOCKER_DIR}"
