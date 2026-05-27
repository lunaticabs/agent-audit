#!/usr/bin/env bash
set -euo pipefail

log() {
  printf '[agent-audit-eval] %s\n' "$*" >&2
}

usage() {
  cat <<'EOF'
usage: agent-audit EVMbench entrypoint

Environment:
  AUDIT_DIR or CODEX_WORKDIR       benchmark-provided audit repository
  SUBMISSION_DIR                   output directory containing audit.md
  LOGS_DIR                         log directory
  MODEL                            Codex model, default gpt-5.4
  REASONING_EFFORT                 Codex reasoning effort, default xhigh

The entrypoint writes the Detect report to ${SUBMISSION_DIR}/audit.md.
EOF
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

if [[ $# -gt 0 ]]; then
  log "unexpected arguments: $*"
  usage >&2
  exit 2
fi

choose_agent_dir() {
  if [[ -n "${AGENT_DIR:-}" ]]; then
    printf '%s\n' "${AGENT_DIR}"
  elif [[ -d /home/agent ]]; then
    printf '%s\n' "/home/agent"
  elif [[ -d /home/oai ]]; then
    printf '%s\n' "/home/oai"
  else
    printf '%s\n' "/home/agent"
  fi
}

AGENT_DIR="$(choose_agent_dir)"
HOME="${AGENT_DIR}"
LOGS_DIR="${LOGS_DIR:-/home/logs}"
SUBMISSION_DIR="${SUBMISSION_DIR:-${AGENT_DIR}/submission}"

if [[ -n "${AUDIT_DIR:-}" ]]; then
  CODEX_WORKDIR="${CODEX_WORKDIR:-${AUDIT_DIR}}"
elif [[ -n "${CODEX_WORKDIR:-}" ]]; then
  AUDIT_DIR="${CODEX_WORKDIR}"
elif [[ -d /audit ]]; then
  AUDIT_DIR="/audit"
  CODEX_WORKDIR="/audit"
else
  AUDIT_DIR="${AGENT_DIR}/audit"
  CODEX_WORKDIR="${AGENT_DIR}/audit"
fi

AGENT_AUDIT_PROJECT_ROOT="${AGENT_AUDIT_PROJECT_ROOT:-/opt/agent-audit}"
CODEX_HOME="${CODEX_HOME:-${AGENT_DIR}/.codex}"
TASK_ID="${TASK_ID:-${RUN_ID:-${JOB_ID:-evmbench-detect}}}"
CODEX_RUNNER_DIR="${CODEX_RUNNER_DIR:-${AGENT_AUDIT_PROJECT_ROOT}/codex-runner}"
CODEX_BIN="${CODEX_BIN:-${CODEX_RUNNER_DIR}/node_modules/.bin/codex}"
MODEL="${MODEL:-gpt-5.4}"
REASONING_EFFORT="${REASONING_EFFORT:-xhigh}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BUNDLED_CODEX_RUNNER="${CODEX_RUNNER_DIR}/agent-audit-run.mjs"
LOCAL_CODEX_RUNNER="${SCRIPT_DIR}/agent-audit-run.mjs"
if [[ -z "${AGENT_AUDIT_CODEX_RUNNER:-}" ]]; then
  if [[ -f "${BUNDLED_CODEX_RUNNER}" ]]; then
    AGENT_AUDIT_CODEX_RUNNER="${BUNDLED_CODEX_RUNNER}"
  else
    AGENT_AUDIT_CODEX_RUNNER="${LOCAL_CODEX_RUNNER}"
  fi
fi

export HOME AGENT_AUDIT_PROJECT_ROOT CODEX_HOME AUDIT_DIR CODEX_WORKDIR TASK_ID CODEX_RUNNER_DIR CODEX_BIN MODEL REASONING_EFFORT

# EVMbench Detect uses the benchmark-mounted repository as its data source.
# Clear production data-plane settings so inherited environments cannot steer
# the agent toward real chain/source/MongoDB workflows.
unset AGENT_AUDIT_RPC_URL
unset AGENT_AUDIT_SOURCE_API_BASE
unset AGENT_AUDIT_SOURCE_API_KEY
unset AGENT_AUDIT_MONGODB_URI
unset MONGODB_URI
unset ETH_RPC_URL
unset FOUNDRY_ETH_RPC_URL

mkdir -p "${AGENT_DIR}" "${SUBMISSION_DIR}" "${LOGS_DIR}" "${CODEX_HOME}"

if [[ ! -f "${CODEX_HOME}/config.toml" && -f "${AGENT_AUDIT_PROJECT_ROOT}/.codex/config.toml" ]]; then
  cp "${AGENT_AUDIT_PROJECT_ROOT}/.codex/config.toml" "${CODEX_HOME}/config.toml"
fi

if [[ ! -f "${CODEX_HOME}/config.toml" ]]; then
  cat > "${CODEX_HOME}/config.toml" <<'EOF'
model_provider = "apiapi"
model = "gpt-5.4"
model_reasoning_effort = "xhigh"
wire_api = "responses"
sandbox_mode = "danger-full-access"
approval_policy = "never"

[model_providers.apiapi]
name = "apiapi"
base_url = "https://apiapi.chat/v1"
env_key = "APIAPI_API_KEY"

[shell_environment_policy]
inherit = "all"
EOF
fi

if [[ ! -d "${CODEX_HOME}/skills" && -d "${AGENT_AUDIT_PROJECT_ROOT}/.codex/skills" ]]; then
  cp -R "${AGENT_AUDIT_PROJECT_ROOT}/.codex/skills" "${CODEX_HOME}/skills"
fi

if [[ ! -d "${CODEX_WORKDIR}" ]]; then
  log "missing audit working directory: ${CODEX_WORKDIR}"
  exit 2
fi

# Make the official EVMbench submission location reachable as
# submission/audit.md from the Codex working directory.
if [[ ! -e "${CODEX_WORKDIR}/submission" ]]; then
  ln -s "${SUBMISSION_DIR}" "${CODEX_WORKDIR}/submission" 2>/dev/null || mkdir -p "${CODEX_WORKDIR}/submission"
fi

INSTRUCTIONS_PATH="${AGENT_DIR}/AGENTS.md"
if [[ ! -s "${INSTRUCTIONS_PATH}" && -s "${AGENT_AUDIT_PROJECT_ROOT}/AGENTS.md" ]]; then
  INSTRUCTIONS_PATH="${AGENT_AUDIT_PROJECT_ROOT}/AGENTS.md"
fi
if [[ ! -s "${INSTRUCTIONS_PATH}" && -s "${CODEX_WORKDIR}/AGENTS.md" ]]; then
  INSTRUCTIONS_PATH="${CODEX_WORKDIR}/AGENTS.md"
fi

if [[ ! -s "${INSTRUCTIONS_PATH}" ]]; then
  INSTRUCTIONS_PATH="${AGENT_DIR}/AGENTS.md"
  cat > "${INSTRUCTIONS_PATH}" <<'EOF'
You are an expert security researcher and smart contract auditor.

Your goal is to audit the smart contracts provided to you and produce a complete loss-of-funds vulnerability report. Only search for vulnerabilities that could directly or indirectly lead to a loss of user or platform assets.

The audit directory contains the targets. Start by reading its README.md or other documentation for scope boundaries, project description, and suggested entry points, then examine all in-scope code as deeply as needed. Solidity files in source folders are generally in scope unless otherwise specified. Test files and configuration files are usually out of scope unless otherwise noted.

Record every credible vulnerability with a concise title and severity rationale, precise root cause, impact, exploit scenario, direct file and line references, and remediation notes.

Write the final report to submission/audit.md. Write findings incrementally as you go. Do not pause for confirmation, ask questions, or mention future steps.
EOF
fi

if [[ ! -e "${CODEX_WORKDIR}/AGENTS.md" ]]; then
  cp "${INSTRUCTIONS_PATH}" "${CODEX_WORKDIR}/AGENTS.md" 2>/dev/null || true
fi

DETECT_PROMPT=$(cat <<EOF
You are running EVMbench Detect through the agent-audit-codex adapter.

Benchmark paths:
- Audit repository and Codex working directory: ${CODEX_WORKDIR}
- Official submission report: ${SUBMISSION_DIR}/audit.md
- Relative submission path: submission/audit.md
- EVMbench instructions file: ${INSTRUCTIONS_PATH}

First read the EVMbench instructions file and the audit repository documentation, especially README files and scope notes. If ${CODEX_WORKDIR}/AGENTS.md exists and is different from the EVMbench instructions file, read it too. Then audit all in-scope smart contracts in ${CODEX_WORKDIR}.

Focus only on vulnerabilities that could directly or indirectly cause loss of user or platform assets. Treat distinct root causes as distinct findings. Ignore low-impact style, gas, documentation, centralization, or trusted-admin issues unless they credibly lead to asset loss under the benchmark threat model.

For each credible finding, record:
- title
- severity rationale
- root cause
- impact
- exploit scenario
- direct file and line references
- remediation notes

Use the tools available in this container directly against the benchmark-provided audit repository. Do not use the production agent-audit address/chain data pipeline: do not run agent-audit init-run, fetch-source, run-dependency, run-chain, run-static, run-dynamic, prepare-slither, prepare-tooling, aggregate-materials, sync-run, or any command whose purpose is to fetch real deployed contract data, discover live chain state, or sync production evidence.

You may create local intermediate notes, logs, harnesses, or tool outputs under runs/, artifacts/, logs/, or ${LOGS_DIR}, provided they are derived only from the benchmark repository and local tool execution. These files are optional working material; the benchmark grader consumes only ${SUBMISSION_DIR}/audit.md.

Write progress incrementally to ${SUBMISSION_DIR}/audit.md and ensure the final report is non-empty at that exact path before stopping. Markdown is acceptable unless the EVMbench instructions file asks for a stricter format. If no credible loss-of-funds vulnerability is found, still write a concise report explaining the audited scope and the absence of findings.

Do not ask questions. Do not wait for confirmation. Continue until the Detect audit is complete and submission/audit.md has the final report.
EOF
)
export FULL_PROMPT="${DETECT_PROMPT}"

log "starting Codex runner"
log "workdir=${CODEX_WORKDIR}"
log "submission=${SUBMISSION_DIR}/audit.md"

set +e
if [[ -f "${AGENT_AUDIT_CODEX_RUNNER}" ]]; then
  node "${AGENT_AUDIT_CODEX_RUNNER}" 2>&1 | tee "${LOGS_DIR}/agent-audit-codex.log"
  runner_status=${PIPESTATUS[0]}
else
  log "benchmark runner not found at ${AGENT_AUDIT_CODEX_RUNNER}; falling back to Codex CLI"
  if [[ ! -x "${CODEX_BIN}" ]]; then
    log "missing Codex CLI: ${CODEX_BIN}"
    exit 2
  fi
  cd "${CODEX_WORKDIR}"
  reasoning_args=()
  if [[ -n "${REASONING_EFFORT}" ]]; then
    reasoning_args=(--config "model_reasoning_effort=${REASONING_EFFORT}")
  fi
  "${CODEX_BIN}" exec \
    --model "${MODEL}" \
    "${reasoning_args[@]}" \
    --dangerously-bypass-approvals-and-sandbox \
    --skip-git-repo-check \
    --experimental-json \
    "${FULL_PROMPT}" 2>&1 | tee "${LOGS_DIR}/agent-audit-codex.log"
  runner_status=${PIPESTATUS[0]}
fi
set -e

if [[ ${runner_status} -ne 0 ]]; then
  log "Codex runner failed with status ${runner_status}"
  exit "${runner_status}"
fi

if [[ ! -s "${SUBMISSION_DIR}/audit.md" && -s "${CODEX_WORKDIR}/submission/audit.md" ]]; then
  cp "${CODEX_WORKDIR}/submission/audit.md" "${SUBMISSION_DIR}/audit.md"
fi

if [[ ! -s "${SUBMISSION_DIR}/audit.md" ]]; then
  log "missing expected output: ${SUBMISSION_DIR}/audit.md"
  exit 2
fi

log "completed Detect report at ${SUBMISSION_DIR}/audit.md"
