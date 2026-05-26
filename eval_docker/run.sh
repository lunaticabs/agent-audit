#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE_REPO="${IMAGE_REPO:-agent-audit-eval}"
IMAGE_TAG="${IMAGE_TAG:-0.1}"
IMAGE="${IMAGE_REPO}:${IMAGE_TAG}"
ENV_FILE="${ROOT_DIR}/.env"

AUDIT_HOST_DIR="${AUDIT_DIR:-}"
SUBMISSION_HOST_DIR="${SUBMISSION_DIR:-${ROOT_DIR}/eval_docker/submission}"
LOGS_HOST_DIR="${LOGS_DIR:-${ROOT_DIR}/eval_docker/logs}"

usage() {
  cat >&2 <<'EOF'
usage: ./eval_docker/run.sh --audit-dir <path> [--submission-dir <path>] [--logs-dir <path>]

Runs the EVMbench Detect entrypoint in the eval image. The report is written to
the mounted submission directory as audit.md.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --audit-dir)
      if [[ $# -lt 2 ]]; then
        echo "--audit-dir requires a path" >&2
        exit 2
      fi
      AUDIT_HOST_DIR="${2:-}"
      shift 2
      ;;
    --submission-dir)
      if [[ $# -lt 2 ]]; then
        echo "--submission-dir requires a path" >&2
        exit 2
      fi
      SUBMISSION_HOST_DIR="${2:-}"
      shift 2
      ;;
    --logs-dir)
      if [[ $# -lt 2 ]]; then
        echo "--logs-dir requires a path" >&2
        exit 2
      fi
      LOGS_HOST_DIR="${2:-}"
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage
      exit 2
      ;;
  esac
done

if [[ -z "${AUDIT_HOST_DIR}" ]]; then
  echo "--audit-dir is required" >&2
  usage
  exit 2
fi

if [[ ! -d "${AUDIT_HOST_DIR}" ]]; then
  echo "audit directory does not exist: ${AUDIT_HOST_DIR}" >&2
  exit 2
fi

if [[ ! -f "${ENV_FILE}" && -z "${APIAPI_API_KEY:-}" ]]; then
  echo "set APIAPI_API_KEY or create ${ENV_FILE}" >&2
  exit 2
fi

mkdir -p "${SUBMISSION_HOST_DIR}" "${LOGS_HOST_DIR}"

docker_args=(
  run
  --rm
  -v "${AUDIT_HOST_DIR}:/home/agent/audit"
  -v "${SUBMISSION_HOST_DIR}:/home/agent/submission"
  -v "${LOGS_HOST_DIR}:/home/logs"
  -e "AGENT_DIR=/home/agent"
  -e "AUDIT_DIR=/home/agent/audit"
  -e "SUBMISSION_DIR=/home/agent/submission"
  -e "LOGS_DIR=/home/logs"
)

if [[ -f "${ENV_FILE}" ]]; then
  docker_args+=(-v "${ENV_FILE}:/opt/agent-audit/.env:ro")
fi

for name in APIAPI_API_KEY MODEL REASONING_EFFORT TASK_ID; do
  if [[ -n "${!name:-}" ]]; then
    docker_args+=(-e "${name}=${!name}")
  fi
done

docker_args+=("${IMAGE}")

exec docker "${docker_args[@]}"
