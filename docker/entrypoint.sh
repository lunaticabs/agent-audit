#!/usr/bin/env bash
set -euo pipefail

cd /opt/agent-audit

mkdir -p runs

if [[ -f /opt/agent-audit/.env ]]; then
  set -a
  # shellcheck disable=SC1091
  source /opt/agent-audit/.env
  set +a
fi

export CODEX_HOME="${CODEX_HOME:-/root/.codex}"
export AGENT_AUDIT_PROJECT_ROOT="${AGENT_AUDIT_PROJECT_ROOT:-/opt/agent-audit}"
mkdir -p "$CODEX_HOME"

if [[ ! -f "$CODEX_HOME/config.toml" ]]; then
  cp /opt/agent-audit/.codex/config.toml "$CODEX_HOME/config.toml"
fi

if [[ $# -eq 0 ]]; then
  echo "usage: docker run ... <contract_address> [chain] [extra codex prompt]"
  exit 2
fi

ADDRESS="$1"
shift

CHAIN="eth"
if [[ $# -gt 0 ]]; then
  CHAIN="$1"
  shift
fi

EXTRA_PROMPT="$*"

PROMPT="Check AGENTS.md and audit ${ADDRESS} on ${CHAIN}."
if [[ -n "$EXTRA_PROMPT" ]]; then
  PROMPT="${PROMPT}"$'\n\n'"${EXTRA_PROMPT}"
fi

exec codex exec \
  --ephemeral \
  --color never \
  --dangerously-bypass-approvals-and-sandbox \
  --skip-git-repo-check \
  --cd /opt/agent-audit \
  "$PROMPT"
