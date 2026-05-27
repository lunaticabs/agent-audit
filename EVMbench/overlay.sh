#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
usage: EVMbench/overlay.sh --evmbench-dir <path> [--dockerfile <path>] [--no-patch-dockerfile]

Copies the agent-audit-codex adapter and eval runtime overlay into an EVMbench
checkout. Run this from the agent-audit repository root.
EOF
}

EVMBENCH_DIR=""
DOCKERFILE_PATH=""
PATCH_DOCKERFILE=1

while [[ $# -gt 0 ]]; do
  case "$1" in
    --evmbench-dir)
      if [[ $# -lt 2 ]]; then
        echo "--evmbench-dir requires a path" >&2
        exit 2
      fi
      EVMBENCH_DIR="$2"
      shift 2
      ;;
    --dockerfile)
      if [[ $# -lt 2 ]]; then
        echo "--dockerfile requires a path" >&2
        exit 2
      fi
      DOCKERFILE_PATH="$2"
      shift 2
      ;;
    --no-patch-dockerfile)
      PATCH_DOCKERFILE=0
      shift
      ;;
    -h|--help)
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

if [[ -z "${EVMBENCH_DIR}" ]]; then
  echo "--evmbench-dir is required" >&2
  usage
  exit 2
fi

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EVMBENCH_DIR="$(cd "${EVMBENCH_DIR}" && pwd)"

if [[ ! -d "${EVMBENCH_DIR}/evmbench" ]]; then
  echo "not a frontier-evals EVMbench checkout: ${EVMBENCH_DIR}" >&2
  echo "expected directory: ${EVMBENCH_DIR}/evmbench" >&2
  exit 2
fi

mkdir -p "${EVMBENCH_DIR}/evmbench/agents/agent-audit-codex"
rsync -a --delete \
  "${ROOT_DIR}/EVMbench/agents/agent-audit-codex/" \
  "${EVMBENCH_DIR}/evmbench/agents/agent-audit-codex/"

if [[ "${PATCH_DOCKERFILE}" -eq 1 ]]; then
  if [[ -z "${DOCKERFILE_PATH}" ]]; then
    if [[ -f "${EVMBENCH_DIR}/base/Dockerfile" ]]; then
      DOCKERFILE_PATH="${EVMBENCH_DIR}/base/Dockerfile"
    elif [[ -f "${EVMBENCH_DIR}/evmbench/Dockerfile" ]]; then
      DOCKERFILE_PATH="${EVMBENCH_DIR}/evmbench/Dockerfile"
    elif [[ -f "${EVMBENCH_DIR}/Dockerfile" ]]; then
      DOCKERFILE_PATH="${EVMBENCH_DIR}/Dockerfile"
    else
      echo "could not auto-detect EVMbench Dockerfile" >&2
      echo "rerun with --dockerfile <path> or --no-patch-dockerfile" >&2
      exit 2
    fi
  elif [[ "${DOCKERFILE_PATH}" != /* ]]; then
    DOCKERFILE_PATH="${EVMBENCH_DIR}/${DOCKERFILE_PATH}"
  fi

  if [[ ! -f "${DOCKERFILE_PATH}" ]]; then
    echo "Dockerfile not found: ${DOCKERFILE_PATH}" >&2
    exit 2
  fi

  DOCKER_CONTEXT_DIR="$(cd "$(dirname "${DOCKERFILE_PATH}")" && pwd)"
else
  DOCKER_CONTEXT_DIR="${EVMBENCH_DIR}"
fi

OVERLAY_DIR="${DOCKER_CONTEXT_DIR}/agent-audit-overlay"
rm -rf "${OVERLAY_DIR}"
mkdir -p \
  "${OVERLAY_DIR}/eval" \
  "${OVERLAY_DIR}/codex-runner" \
  "${OVERLAY_DIR}/.codex"

cp "${ROOT_DIR}/eval_docker/start.sh" "${OVERLAY_DIR}/eval/start.sh"
cp "${ROOT_DIR}/eval_docker/AGENTS.md" "${OVERLAY_DIR}/AGENTS.md"
cp "${ROOT_DIR}/eval_docker/agent-audit-guard.sh" "${OVERLAY_DIR}/agent-audit-guard.sh"
cp "${ROOT_DIR}/eval_docker/codex-runner/agent-audit-run.mjs" \
  "${OVERLAY_DIR}/codex-runner/agent-audit-run.mjs"
rsync -a --delete "${ROOT_DIR}/eval_docker/.codex/" "${OVERLAY_DIR}/.codex/"

cat > "${OVERLAY_DIR}/Dockerfile.fragment" <<'EOF'
# BEGIN agent-audit-codex overlay
# agent-audit-codex EVMbench overlay.
#
# Apply these instructions to an EVMbench audit image after its base tooling is
# installed. The build context must include the generated agent-audit-overlay/
# directory next to this Dockerfile.

USER root

ARG NODE_MAJOR=22
ENV CODEX_HOME=/root/.codex
ENV CODEX_RUNNER_DIR=/opt/agent-audit/codex-runner
ENV AGENT_AUDIT_PROJECT_ROOT=/opt/agent-audit
ENV AGENT_AUDIT_EVAL_ENTRYPOINT=/opt/agent-audit/eval/start.sh

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        bash \
        ca-certificates \
        curl \
        git \
        gnupg \
        rsync \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

RUN if ! command -v node >/dev/null 2>&1; then \
      mkdir -p /etc/apt/keyrings \
      && curl -fsSL https://deb.nodesource.com/gpgkey/nodesource-repo.gpg.key \
        | gpg --dearmor -o /etc/apt/keyrings/nodesource.gpg \
      && echo "deb [signed-by=/etc/apt/keyrings/nodesource.gpg] https://deb.nodesource.com/node_${NODE_MAJOR}.x nodistro main" \
        > /etc/apt/sources.list.d/nodesource.list \
      && apt-get update \
      && apt-get install -y --no-install-recommends nodejs \
      && apt-get purge -y --auto-remove gnupg \
      && apt-get clean \
      && rm -rf /var/lib/apt/lists/*; \
    fi

RUN mkdir -p "${CODEX_RUNNER_DIR}" \
    && cd "${CODEX_RUNNER_DIR}" \
    && npm init -y \
    && npm install --omit=dev @openai/codex @openai/codex-sdk \
    && npm cache clean --force

ENV PATH=${CODEX_RUNNER_DIR}/node_modules/.bin:${PATH}

RUN mkdir -p /opt/agent-audit/eval /opt/agent-audit/bin /root/.codex /home/agent /home/logs

COPY agent-audit-overlay/AGENTS.md /opt/agent-audit/AGENTS.md
COPY agent-audit-overlay/.codex /opt/agent-audit/.codex
COPY agent-audit-overlay/eval/start.sh /opt/agent-audit/eval/start.sh
COPY agent-audit-overlay/codex-runner/agent-audit-run.mjs ${CODEX_RUNNER_DIR}/agent-audit-run.mjs
COPY agent-audit-overlay/agent-audit-guard.sh /usr/local/bin/agent-audit

RUN chmod +x /opt/agent-audit/eval/start.sh \
    && chmod +x ${CODEX_RUNNER_DIR}/agent-audit-run.mjs \
    && chmod +x /usr/local/bin/agent-audit \
    && cp /opt/agent-audit/.codex/config.toml /root/.codex/config.toml \
    && test -f /opt/agent-audit/.codex/skills/workspace/SKILL.md \
    && node /opt/agent-audit/codex-runner/agent-audit-run.mjs --help >/dev/null \
    && /opt/agent-audit/eval/start.sh --help >/dev/null \
    && agent-audit --help >/dev/null
# END agent-audit-codex overlay
EOF

cat > "${OVERLAY_DIR}/README.md" <<'EOF'
# agent-audit-codex Overlay

This directory was generated by `agent-audit/EVMbench/overlay.sh`.

It contains:

- `AGENTS.md`: eval-specific Codex instructions.
- `.codex/`: eval-specific Codex config and skills.
- `eval/start.sh`: EVMbench Detect entrypoint.
- `codex-runner/agent-audit-run.mjs`: Codex SDK runner.
- `agent-audit-guard.sh`: guard wrapper for production `agent-audit` commands.
- `Dockerfile.fragment`: Docker instructions to add the runtime to an EVMbench
  audit image.

`Dockerfile.fragment` can be appended to the Dockerfile used to build EVMbench
audit images, after the base audit tooling is installed. The generated
`agent-audit-overlay/` directory must be inside the Docker build context, next
to the patched Dockerfile.
EOF

if [[ "${PATCH_DOCKERFILE}" -eq 1 ]]; then
  BACKUP_PATH="${DOCKERFILE_PATH}.agent-audit-overlay.bak"
  if [[ ! -f "${BACKUP_PATH}" ]]; then
    cp "${DOCKERFILE_PATH}" "${BACKUP_PATH}"
  fi

  TMP_PATH="$(mktemp)"
  awk '
    /^# BEGIN agent-audit-codex overlay$/ { skip = 1; next }
    /^# END agent-audit-codex overlay$/ { skip = 0; next }
    skip != 1 { print }
  ' "${DOCKERFILE_PATH}" > "${TMP_PATH}"
  cat "${TMP_PATH}" > "${DOCKERFILE_PATH}"
  rm -f "${TMP_PATH}"
  {
    printf '\n'
    cat "${OVERLAY_DIR}/Dockerfile.fragment"
  } >> "${DOCKERFILE_PATH}"
fi

cat <<EOF
Overlay written to:
  ${EVMBENCH_DIR}/evmbench/agents/agent-audit-codex
  ${OVERLAY_DIR}

Next steps:
  1. Build the EVMbench audit images from ${EVMBENCH_DIR}.
  2. Run Detect with agent_id=agent-audit-codex.
EOF

if [[ "${PATCH_DOCKERFILE}" -eq 1 ]]; then
  cat <<EOF

Patched Dockerfile:
  ${DOCKERFILE_PATH}

Backup:
  ${BACKUP_PATH}
EOF
else
  cat <<EOF

Dockerfile was not patched. Append this fragment manually:
  ${OVERLAY_DIR}/Dockerfile.fragment
EOF
fi
