#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE_REPO="${IMAGE_REPO:-agent-audit}"
IMAGE_TAG="${IMAGE_TAG:-2.0.0}"
IMAGE="${IMAGE_REPO}:${IMAGE_TAG}"
ENV_FILE="${ROOT_DIR}/.env"
RUNS_DIR="${ROOT_DIR}/runs"

usage() {
  cat >&2 <<'EOF'
usage: ./docker/run.sh [--prompt <text>]

examples:
  ./docker/run.sh --prompt "Audit 0x0000000000000000000000000000000000000000 on eth."
  FULL_PROMPT="Audit 0x0000000000000000000000000000000000000000 on base." ./docker/run.sh
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  usage
  exit 0
fi

if [[ $# -eq 0 && -z "${FULL_PROMPT:-}" ]]; then
  usage
  exit 2
fi

if [[ ! -f "${ENV_FILE}" && -z "${DEEPSEEK_API_KEY:-}" ]]; then
  echo "${ENV_FILE} does not exist and DEEPSEEK_API_KEY is not set" >&2
  exit 2
fi

if [[ -f "${ENV_FILE}" && -z "${DEEPSEEK_API_KEY:-}" ]] \
  && ! grep -Eq '^[[:space:]]*(export[[:space:]]+)?DEEPSEEK_API_KEY=[^[:space:]]+' "${ENV_FILE}"; then
  echo "DEEPSEEK_API_KEY is not configured in ${ENV_FILE} or the environment" >&2
  exit 2
fi

mkdir -p "${RUNS_DIR}"

docker_args=(
  run
  --rm
  -v "${RUNS_DIR}:/opt/agent-audit/runs"
)

if [[ -f "${ENV_FILE}" ]]; then
  docker_args+=(-v "${ENV_FILE}:/opt/agent-audit/.env:ro")
fi

for name in \
  DEEPSEEK_API_KEY \
  DSH_MODEL \
  DSH_REASONING_EFFORT \
  ETHERSCAN_API_KEY \
  ETHERSCAN_API_URL \
  RPC_URL \
  TASK_ID \
  RUN_ID \
  AGENT_AUDIT_PERSIST_DIR; do
  if [[ -n "${!name:-}" ]]; then
    docker_args+=(-e "${name}")
  fi
done

if [[ -n "${FULL_PROMPT:-}" ]]; then
  docker_args+=(-e FULL_PROMPT)
fi

docker_args+=(
  "${IMAGE}"
  "$@"
)

exec docker "${docker_args[@]}"
