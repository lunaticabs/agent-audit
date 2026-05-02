#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DOCKER_DIR="${ROOT_DIR}/docker"
CONTEXT_DIR="${DOCKER_DIR}/context"

rm -rf "${CONTEXT_DIR}"
mkdir -p "${CONTEXT_DIR}"

cp "${ROOT_DIR}/Cargo.toml" "${CONTEXT_DIR}/Cargo.toml"
cp "${ROOT_DIR}/Cargo.lock" "${CONTEXT_DIR}/Cargo.lock"
cp "${ROOT_DIR}/AGENTS.md" "${CONTEXT_DIR}/AGENTS.md"

cp -R "${ROOT_DIR}/src" "${CONTEXT_DIR}/src"
cp -R "${ROOT_DIR}/xtask" "${CONTEXT_DIR}/xtask"
cp -R "${ROOT_DIR}/.codex" "${CONTEXT_DIR}/.codex"

echo "synced docker context to ${CONTEXT_DIR}"
