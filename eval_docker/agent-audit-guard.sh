#!/usr/bin/env bash
set -euo pipefail

REAL_AGENT_AUDIT="${AGENT_AUDIT_REAL_BIN:-/opt/agent-audit/bin/agent-audit-real}"

usage() {
  cat <<'EOF'
agent-audit is disabled in the EVMbench Detect runtime.

EVMbench provides a local audit repository through AUDIT_DIR / CODEX_WORKDIR,
and the grader consumes only submission/audit.md. Use shell tools, Slither,
Foundry, Cast, Anvil, and Echidna directly against that local repository.

Production address/chain pipeline commands such as init-run, fetch-source,
run-dependency, run-chain, run-static, run-dynamic, prepare-slither,
prepare-tooling, aggregate-materials, and sync-run are intentionally blocked in
this image.
EOF
}

if [[ $# -eq 0 || "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

case "${1:-}" in
  init-run|fetch-source|run-dependency|run-chain|run-static|run-dynamic|prepare-slither|prepare-tooling|aggregate-materials|sync-run)
    printf 'agent-audit %s is disabled in EVMbench Detect; use the local audit repository and write submission/audit.md.\n' "$1" >&2
    exit 2
    ;;
  *)
    printf 'agent-audit production CLI is disabled in EVMbench Detect: %s\n' "$*" >&2
    if [[ -x "${REAL_AGENT_AUDIT}" ]]; then
      printf 'Real binary is retained for image debugging at %s, but is not on the eval path.\n' "${REAL_AGENT_AUDIT}" >&2
    fi
    exit 2
    ;;
esac
