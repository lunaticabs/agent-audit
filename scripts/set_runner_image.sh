#!/usr/bin/env bash
# Point the runner ScaledJob at an exact image digest.
#
#   ./scripts/set_runner_image.sh ghcr.io/lunaticabs/agent-audit:rebuild
#   ./scripts/set_runner_image.sh ghcr.io/lunaticabs/agent-audit@sha256:... --apply
#
# A tag is resolved to a digest through the registry before it is written, so
# the manifest never carries a mutable tag. Both the container image and
# AGENT_AUDIT_IMAGE are rewritten: the runner compares the queued message image
# with AGENT_AUDIT_IMAGE and refuses to run a task enqueued for another digest.
#
# Private registries: export REGISTRY_USER and REGISTRY_TOKEN.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IMAGE="${1:-}"
APPLY="${2:-}"
FILE="${RUNNER_SCALEDJOB:-k3s/runner-scaledjob.yaml}"

if [[ -z "$IMAGE" ]]; then
  printf 'usage: %s <image:tag|image@sha256:...> [--apply]\n' "$0" >&2
  exit 2
fi
if [[ ! -f "$FILE" ]]; then
  printf 'error: %s not found\n' "$FILE" >&2
  exit 2
fi

if [[ "$IMAGE" != *"@sha256:"* ]]; then
  RESOLVED="$(PYTHONPATH="$SCRIPT_DIR" python3 - "$IMAGE" <<'PY'
import os
import sys

from enqueue_redis import resolve_image_digest

pinned, _ = resolve_image_digest(
    sys.argv[1],
    username=os.environ.get("REGISTRY_USER") or None,
    password=os.environ.get("REGISTRY_TOKEN") or None,
)
print(pinned)
PY
)" || {
    printf 'error: could not resolve a digest for %s\n' "$IMAGE" >&2
    exit 1
  }
  printf 'resolved %s -> %s\n' "$IMAGE" "$RESOLVED"
  IMAGE="$RESOLVED"
fi

python3 - "$FILE" "$IMAGE" <<'PY'
import sys

path, image = sys.argv[1], sys.argv[2]
with open(path, "r", encoding="utf-8") as handle:
    lines = handle.readlines()

out = []
changed_image = False
changed_env = False
in_aut_env = False

for line in lines:
    stripped = line.strip()
    if stripped.startswith("- name: AGENT_AUDIT_IMAGE"):
        in_aut_env = True
        out.append(line)
        continue
    if in_aut_env and stripped.startswith("value:"):
        indent = line[: len(line) - len(line.lstrip())]
        out.append("%svalue: %s\n" % (indent, image))
        in_aut_env = False
        changed_env = True
        continue
    if not changed_image and stripped.startswith("image:") and "agent-audit" in line:
        indent = line[: len(line) - len(line.lstrip())]
        out.append("%simage: %s\n" % (indent, image))
        changed_image = True
        continue
    out.append(line)

if not changed_image or not changed_env:
    sys.stderr.write(
        "error: expected one container image and one AGENT_AUDIT_IMAGE value in %s\n" % path
    )
    sys.exit(1)

with open(path, "w", encoding="utf-8") as handle:
    handle.writelines(out)
print("pinned %s" % path)
PY

printf 'runner image -> %s\n' "$IMAGE"

if [[ "$APPLY" == "--apply" ]]; then
  KUBECTL="${KUBECTL:-k3s kubectl}"
  # shellcheck disable=SC2086
  ${KUBECTL} apply -f "$FILE"
  printf 'applied. Running Jobs keep their image; new Jobs use the digest.\n'
fi
