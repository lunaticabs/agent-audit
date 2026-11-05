#!/usr/bin/env bash
# Install the pinned KEDA release that provides the ScaledJob CRD.
#
#   ./scripts/install_keda.sh
#   KEDA_VERSION=2.21.0 ./scripts/install_keda.sh
#
# k3s ships its own kubectl; override with KUBECTL=kubectl elsewhere.
set -euo pipefail

KEDA_VERSION="${KEDA_VERSION:-2.21.0}"
KUBECTL="${KUBECTL:-k3s kubectl}"
MANIFEST_URL="https://github.com/kedacore/keda/releases/download/v${KEDA_VERSION}/keda-${KEDA_VERSION}.yaml"

printf 'installing KEDA %s from %s\n' "$KEDA_VERSION" "$MANIFEST_URL"
# shellcheck disable=SC2086
${KUBECTL} apply -f "$MANIFEST_URL"

# The ScaledJob CRD must be established before k3s/kustomization.yaml applies
# the runner. Wait for the operator so `apply -k` does not race the CRD.
# shellcheck disable=SC2086
${KUBECTL} -n keda rollout status deploy/keda-operator --timeout=180s
# shellcheck disable=SC2086
${KUBECTL} -n keda rollout status deploy/keda-metrics-apiserver --timeout=180s

printf 'KEDA %s ready; apply k3s/ next\n' "$KEDA_VERSION"
