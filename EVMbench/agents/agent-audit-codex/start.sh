#!/usr/bin/env bash
set -euo pipefail

log() {
  printf '[agent-audit-codex] %s\n' "$*" >&2
}

ENTRYPOINT="${AGENT_AUDIT_EVAL_ENTRYPOINT:-/opt/agent-audit/eval/start.sh}"

if [[ -x "${ENTRYPOINT}" ]]; then
  exec "${ENTRYPOINT}" "$@"
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ENTRYPOINT="${SCRIPT_DIR}/../../../eval_docker/start.sh"

if [[ -x "${REPO_ENTRYPOINT}" ]]; then
  exec "${REPO_ENTRYPOINT}" "$@"
fi

log "missing eval Docker entrypoint: ${ENTRYPOINT}"
log "build the eval image from eval_docker/Dockerfile, or make ${ENTRYPOINT} available in the EVMbench audit container"
exit 2
