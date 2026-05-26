#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DOCKER_DIR="${ROOT_DIR}/eval_docker"
IMAGE_REPO="${IMAGE_REPO:-agent-audit-eval}"
IMAGE_TAG="${IMAGE_TAG:-0.1}"

docker build \
  -f "${DOCKER_DIR}/Dockerfile" \
  -t "${IMAGE_REPO}:${IMAGE_TAG}" \
  "$@" \
  "${ROOT_DIR}"
