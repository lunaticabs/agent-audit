#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DOCKER_DIR="${ROOT_DIR}/docker"

docker build \
  -f "${DOCKER_DIR}/Dockerfile" \
  -t agent-audit-codex \
  "$@" \
  "${ROOT_DIR}"
