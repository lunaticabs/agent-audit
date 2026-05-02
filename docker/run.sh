#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE_REPO="${IMAGE_REPO:-agent-audit}"
IMAGE_TAG="${IMAGE_TAG:-0.1}"
IMAGE="${IMAGE_REPO}:${IMAGE_TAG}"

if [[ $# -lt 1 ]]; then
  echo "usage: ./docker/run.sh <contract_address> [chain] [extra codex prompt]" >&2
  exit 2
fi

if [[ -z "${APIAPI_API_KEY:-}" ]]; then
  echo "APIAPI_API_KEY is not set in the host environment" >&2
  exit 2
fi

docker_args=(
  run
  --rm
  -e "APIAPI_API_KEY=${APIAPI_API_KEY}"
)

if [[ -f "${ROOT_DIR}/.env" ]]; then
  docker_args+=(
    -v "${ROOT_DIR}/.env:/opt/agent-audit/.env:ro"
  )
fi

docker_args+=(
  "${IMAGE}"
  "$@"
)

exec docker "${docker_args[@]}"
