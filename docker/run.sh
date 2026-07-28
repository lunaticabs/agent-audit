#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE_REPO="${IMAGE_REPO:-agent-audit}"
IMAGE_TAG="${IMAGE_TAG:-0.1}"
IMAGE="${IMAGE_REPO}:${IMAGE_TAG}"
ENV_FILE="${ROOT_DIR}/.env"

usage() {
  cat >&2 <<'EOF'
usage: ./docker/run.sh [--prompt <text>]

examples:
  ./docker/run.sh --prompt "Check AGENTS.md and audit 0x0000000000000000000000000000000000000000 on eth."
  FULL_PROMPT="Check AGENTS.md and audit 0x0000000000000000000000000000000000000000 on eth." ./docker/run.sh
EOF
}

if [[ $# -eq 0 && -z "${FULL_PROMPT:-}" ]]; then
  usage
  exit 2
fi

if [[ "$1" == "--help" || "$1" == "-h" ]]; then
  usage
  exit 0
fi

if [[ ! -f "${ENV_FILE}" ]]; then
  echo "${ENV_FILE} does not exist" >&2
  exit 2
fi

if ! grep -Eq '^[[:space:]]*APIAPI_API_KEY=' "${ENV_FILE}"; then
  echo "APIAPI_API_KEY is not configured in ${ENV_FILE}" >&2
  exit 2
fi

docker_args=(
  run
  --rm
  -v "${ENV_FILE}:/opt/agent-audit/.env:ro"
)

if [[ -n "${FULL_PROMPT:-}" ]]; then
  docker_args+=(
    -e "FULL_PROMPT=${FULL_PROMPT}"
  )
fi

if [[ -n "${TASK_ID:-}" ]]; then
  docker_args+=(
    -e "TASK_ID=${TASK_ID}"
  )
fi

docker_args+=(
  "${IMAGE}"
  "$@"
)

exec docker "${docker_args[@]}"
