#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DOCKER_DIR="${ROOT_DIR}/docker"
IMAGE_REPO="${IMAGE_REPO:-agent-audit-dispatcher}"
IMAGE_TAG="${IMAGE_TAG:-0.1}"

docker build \
  -f "${DOCKER_DIR}/dispatcher.Dockerfile" \
  --target smoke-test \
  -t "${IMAGE_REPO}:smoke-test" \
  "$@" \
  "${ROOT_DIR}"

docker build \
  -f "${DOCKER_DIR}/dispatcher.Dockerfile" \
  -t "${IMAGE_REPO}:${IMAGE_TAG}" \
  "$@" \
  "${ROOT_DIR}"
