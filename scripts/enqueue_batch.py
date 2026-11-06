#!/usr/bin/env python3
"""Start a batch: open the Redis tunnel, resolve the pinned image, enqueue.

This is the short entry point. It replaces the three-step flow of

    k3s kubectl -n agent-audit port-forward svc/agent-audit-redis 6380:6379 &
    python3 scripts/enqueue_redis.py ... --host 127.0.0.1 --port 6380 \
      --image ghcr.io/...@sha256:...

with

    ./scripts/enqueue_batch.py

It picks a free local port, starts `kubectl port-forward` itself, waits for it,
reads the runner image from k3s/runner-scaledjob.yaml (and refuses to enqueue if
it differs from AGENT_AUDIT_IMAGE), enqueues, writes a manifest under runs/,
and tears the tunnel down again.
"""

from __future__ import annotations

import argparse
import os
import shlex
import shutil
import socket
import subprocess
import sys
import time
from pathlib import Path
from typing import Optional, Tuple

SCRIPT_DIR = Path(__file__).resolve().parent
REPO_ROOT = SCRIPT_DIR.parent
sys.path.insert(0, str(SCRIPT_DIR))

import enqueue_redis as enq  # noqa: E402

DEFAULT_NAMESPACE = "agent-audit"
DEFAULT_SERVICE = "agent-audit-redis"
DEFAULT_REMOTE_PORT = 6379
DEFAULT_SCALEDJOB = REPO_ROOT / "k3s" / "runner-scaledjob.yaml"
DEFAULT_ADDRESS_FILE = REPO_ROOT / "scripts" / "addresses" / "addrs.txt"
DEFAULT_RUNS_DIR = REPO_ROOT / "runs"


def default_kubectl() -> str:
    configured = os.environ.get("KUBECTL", "").strip()
    if configured:
        return configured
    if shutil.which("k3s"):
        return "k3s kubectl"
    return "kubectl"


def find_free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def start_port_forward(
    kubectl: str,
    namespace: str,
    service: str,
    remote_port: int,
    timeout: float = 20.0,
) -> Tuple[subprocess.Popen, int]:
    local_port = find_free_port()
    command = shlex.split(kubectl) + [
        "-n",
        namespace,
        "port-forward",
        "svc/%s" % service,
        "%d:%d" % (local_port, remote_port),
    ]
    process = subprocess.Popen(
        command,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    deadline = time.time() + timeout
    while time.time() < deadline:
        if process.poll() is not None:
            _, error = process.communicate()
            raise RuntimeError(
                "port-forward exited: %s" % (error or "no output").strip()
            )
        try:
            with socket.create_connection(("127.0.0.1", local_port), timeout=0.5):
                return process, local_port
        except OSError:
            time.sleep(0.2)

    process.terminate()
    raise RuntimeError("port-forward did not become ready in %.0fs" % timeout)


def stop_port_forward(process: Optional[subprocess.Popen]) -> None:
    if process is None or process.poll() is not None:
        return
    process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=5)


def read_scaledjob_image(path: Path) -> Tuple[Optional[str], Optional[str]]:
    """Return ``(container image, AGENT_AUDIT_IMAGE value)`` from the manifest."""

    image: Optional[str] = None
    agent_image: Optional[str] = None
    in_agent_env = False

    for line in path.read_text(encoding="utf-8").splitlines():
        stripped = line.strip()
        if stripped.startswith("- name: AGENT_AUDIT_IMAGE"):
            in_agent_env = True
            continue
        if in_agent_env and stripped.startswith("value:"):
            agent_image = stripped.split(":", 1)[1].strip()
            in_agent_env = False
            continue
        if image is None and stripped.startswith("image:"):
            image = stripped.split(":", 1)[1].strip()

    return image, agent_image


def resolve_requested_image(
    explicit: str,
    scaledjob: Path,
    registry_user: Optional[str],
    registry_token: Optional[str],
) -> Tuple[str, str, Optional[str]]:
    """Return ``(pinned, tag, digest)`` and guard against pod/manifest drift."""

    manifest_image, agent_image = read_scaledjob_image(scaledjob)
    requested = explicit.strip()

    if requested:
        source = "override"
        reference = requested
    elif manifest_image:
        source = str(scaledjob)
        reference = manifest_image
    else:
        raise RuntimeError(
            "no --image given and no image found in %s" % scaledjob
        )

    pinned, digest = enq.resolve_image_digest(
        reference,
        username=registry_user,
        password=registry_token,
    )

    if agent_image and pinned != agent_image:
        raise RuntimeError(
            "image mismatch: requested %s but the ScaledJob's AGENT_AUDIT_IMAGE is %s. "
            "Run scripts/set_runner_image.sh <image> --apply first." % (pinned, agent_image)
        )

    tag = "" if pinned == reference else reference
    print("[IMAGE] source=%s pinned=%s" % (source, pinned))
    return pinned, tag, digest


def parse_args(argv=None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Open the Redis port-forward, resolve the pinned runner image, and enqueue a batch.",
    )
    parser.add_argument("--chain", default="eth", help="Chain label for the prompt. Default: eth")
    parser.add_argument(
        "--address-file",
        default=str(DEFAULT_ADDRESS_FILE),
        help="Address file. Default: scripts/addresses/addrs.txt",
    )
    parser.add_argument("--max-count", type=int, default=0, help="Only the first N addresses")
    parser.add_argument(
        "--image",
        default="",
        help="Override the runner image (default: read from k3s/runner-scaledjob.yaml)",
    )
    parser.add_argument("--prompt-template", default=enq.DEFAULT_PROMPT_TEMPLATE)
    parser.add_argument("--task-prefix", default=enq.DEFAULT_TASK_PREFIX)
    parser.add_argument(
        "--manifest-out",
        default="",
        help="Default: runs/batch-<stamp>/manifest.json",
    )
    parser.add_argument("--dry-run", action="store_true", help="Print tasks; no tunnel, no enqueue")
    parser.add_argument("--namespace", default=DEFAULT_NAMESPACE)
    parser.add_argument("--service", default=DEFAULT_SERVICE)
    parser.add_argument("--remote-port", type=int, default=DEFAULT_REMOTE_PORT)
    parser.add_argument("--kubectl", default=default_kubectl(), help="Default: $KUBECTL, k3s kubectl, or kubectl")
    parser.add_argument(
        "--host",
        default="",
        help="Use an existing tunnel instead of starting port-forward",
    )
    parser.add_argument("--port", type=int, default=0, help="Local Redis port (with --host)")
    parser.add_argument(
        "--registry-user",
        default=os.environ.get("REGISTRY_USER", ""),
        help="Registry user for private images",
    )
    parser.add_argument(
        "--registry-token",
        default=os.environ.get("REGISTRY_TOKEN", ""),
        help="Registry token for private images",
    )
    return parser.parse_args(argv)


def main(argv=None) -> int:
    args = parse_args(argv)
    address_file = Path(args.address_file).expanduser()
    try:
        addresses = enq.load_addresses(address_file, args.max_count)
        items, batch_stamp = enq.build_items(
            addresses=addresses,
            chain=args.chain,
            prompt_template=args.prompt_template,
            task_prefix=args.task_prefix,
        )
    except (OSError, RuntimeError, ValueError) as error:
        print("[FAIL ] %s" % error, file=sys.stderr)
        return 2

    try:
        image, image_tag, digest = resolve_requested_image(
            args.image,
            DEFAULT_SCALEDJOB,
            args.registry_user or None,
            args.registry_token or None,
        )
    except (RuntimeError, ValueError, OSError) as error:
        print("[FAIL ] %s" % error, file=sys.stderr)
        return 2

    print(
        "[LOAD ] file=%s unique_addresses=%d stream=%s group=%s"
        % (address_file, len(addresses), enq.DEFAULT_STREAM, enq.DEFAULT_GROUP)
    )

    if args.dry_run:
        enq.print_dry_run(
            items,
            enq.DEFAULT_STREAM,
            enq.DEFAULT_GROUP,
            args.host or "127.0.0.1",
            args.port or 0,
            image,
            image_tag,
        )
        return 0

    tunnel: Optional[subprocess.Popen] = None
    try:
        if args.host and args.port:
            host, port = args.host, args.port
        else:
            tunnel, port = start_port_forward(
                args.kubectl, args.namespace, args.service, args.remote_port
            )
            host = "127.0.0.1"
            print("[TUNNEL] %s -> %s:%d" % (args.service, host, port))

        enq.enqueue_items(
            items=items,
            stream=enq.DEFAULT_STREAM,
            group=enq.DEFAULT_GROUP,
            host=host,
            port=port,
            image=image,
            image_tag=image_tag,
            timeout_sec=10.0,
        )

        manifest_path = Path(
            args.manifest_out
            or (DEFAULT_RUNS_DIR / ("batch-%s" % batch_stamp) / "manifest.json")
        ).expanduser()
        enq.write_manifest(
            manifest_path,
            items,
            batch_stamp,
            enq.DEFAULT_STREAM,
            enq.DEFAULT_GROUP,
            image,
            image_tag,
            digest,
            args.chain,
        )
        print("[MANIF] %s" % manifest_path)
        print(
            "[DONE ] enqueued=%d. Watch with: ./scripts/watch_batch.py --kubectl '%s'"
            % (len(items), args.kubectl)
        )
    except KeyboardInterrupt:
        print("[STOP ] interrupted", file=sys.stderr)
        return 130
    except (RuntimeError, OSError) as error:
        print("[FAIL ] %s" % error, file=sys.stderr)
        return 1
    finally:
        stop_port_forward(tunnel)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
