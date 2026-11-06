#!/usr/bin/env python3
"""Read-only live view of the agent-audit KEDA batch.

KEDA has no built-in console. This is the smallest thing that answers the
questions that matter during a batch: how deep is the queue, how many Jobs are
running, which ones failed, and how long they have been going.

    # on the k3s node
    python3 scripts/watch_batch.py --kubectl "k3s kubectl"
    python3 scripts/watch_batch.py --kubectl "k3s kubectl" --once

It only reads: `kubectl get` for the ScaledJob/Jobs/Pods and `redis-cli` for
the stream's consumer-group lag and pending count.
"""

from __future__ import annotations

import argparse
import json
import os
import shlex
import subprocess
import sys
import time
from datetime import datetime, timezone
from typing import Any, Dict, List, Optional, Sequence

RESET = "\033[0m"
BOLD = "\033[1m"
DIM = "\033[2m"
RED = "\033[31m"
GREEN = "\033[32m"
YELLOW = "\033[33m"


def run(argv: Sequence[str], timeout: float = 10.0) -> subprocess.CompletedProcess:
    return subprocess.run(
        list(argv),
        capture_output=True,
        text=True,
        timeout=timeout,
    )


def parse_time(value: Optional[str]) -> Optional[datetime]:
    if not value:
        return None
    text = value.replace("Z", "+00:00")
    try:
        return datetime.fromisoformat(text)
    except ValueError:
        return None


def fmt_duration(seconds: float) -> str:
    seconds = max(0, int(seconds))
    if seconds < 60:
        return "%ds" % seconds
    if seconds < 3600:
        return "%dm" % (seconds // 60)
    if seconds < 86400:
        return "%dh%02dm" % (seconds // 3600, (seconds % 3600) // 60)
    return "%dd%02dh" % (seconds // 86400, (seconds % 86400) // 3600)


def color(text: str, code: str, enabled: bool) -> str:
    return "%s%s%s" % (code, text, RESET) if enabled else text


class Kubectl:
    def __init__(self, binary: str) -> None:
        self.base = shlex.split(binary)

    def json(self, *args: str) -> Any:
        result = run(self.base + list(args))
        if result.returncode != 0:
            raise RuntimeError(result.stderr.strip() or "kubectl failed")
        payload = result.stdout.strip()
        return json.loads(payload) if payload else None

    def redis_json(self, namespace: str, deployment: str, *args: str) -> Any:
        result = run(
            self.base
            + [
                "-n",
                namespace,
                "exec",
                "deploy/%s" % deployment,
                "--",
                "redis-cli",
                "--json",
            ]
            + list(args)
        )
        if result.returncode != 0:
            raise RuntimeError(result.stderr.strip() or "redis-cli failed")
        return json.loads(result.stdout.strip())


def job_status(job: Dict[str, Any]) -> str:
    conditions = job.get("status", {}).get("conditions") or []
    for condition in conditions:
        if condition.get("status") != "True":
            continue
        if condition.get("type") == "Complete":
            return "Complete"
        if condition.get("type") == "Failed":
            return "Failed"
    status = job.get("status", {})
    if status.get("active"):
        return "Running"
    if status.get("failed"):
        return "Failed"
    return "Pending"


PHASE_LABELS = {
    "Running": "run",
    "Succeeded": "done",
    "Failed": "err",
    "Error": "err",
    "Pending": "pend",
    "ContainerCreating": "init",
}


def job_pods(job_name: str, pods_by_job: Dict[str, List[Dict[str, Any]]]) -> str:
    pods = pods_by_job.get(job_name) or []
    if not pods:
        return "-"
    latest = max(pods, key=lambda pod: pod.get("metadata", {}).get("creationTimestamp", ""))
    phase = latest.get("status", {}).get("phase") or "?"
    label = PHASE_LABELS.get(phase, phase[:4].lower())
    return "%s x%d" % (label, len(pods))


def collect(
    kube: Kubectl,
    namespace: str,
    stream: str,
    group: str,
    scaledjob: str,
    redis_deployment: str,
) -> Dict[str, Any]:
    now = datetime.now(timezone.utc)
    data: Dict[str, Any] = {"now": now, "errors": []}

    try:
        jobs = (kube.json("-n", namespace, "get", "jobs", "-o", "json") or {}).get("items", [])
        pods = (kube.json("-n", namespace, "get", "pods", "-o", "json") or {}).get("items", [])
    except (RuntimeError, ValueError) as error:
        data["errors"].append("kubectl: %s" % error)
        jobs, pods = [], []

    pods_by_job: Dict[str, List[Dict[str, Any]]] = {}
    for pod in pods:
        labels = pod.get("metadata", {}).get("labels") or {}
        name = labels.get("job-name") or labels.get("batch.kubernetes.io/job-name")
        if name:
            pods_by_job.setdefault(name, []).append(pod)

    rows = []
    counts = {"Running": 0, "Pending": 0, "Complete": 0, "Failed": 0}
    for job in jobs:
        name = job.get("metadata", {}).get("name", "?")
        status = job_status(job)
        counts[status] = counts.get(status, 0) + 1
        started = parse_time(job.get("status", {}).get("startTime"))
        finished = parse_time(job.get("status", {}).get("completionTime"))
        end = finished or now
        age = (now - started).total_seconds() if started else 0.0
        duration = (end - started).total_seconds() if started else 0.0
        rows.append(
            {
                "name": name,
                "status": status,
                "age": age,
                "duration": duration,
                "pods": job_pods(name, pods_by_job),
                "started": started or datetime.min.replace(tzinfo=timezone.utc),
            }
        )

    order = {"Running": 0, "Pending": 1, "Failed": 2, "Complete": 3}
    rows.sort(key=lambda row: (order.get(row["status"], 9), -row["started"].timestamp()))

    data["jobs"] = rows
    data["counts"] = counts

    try:
        groups = kube.redis_json(namespace, redis_deployment, "XINFO", "GROUPS", stream) or []
        current = next((item for item in groups if item.get("name") == group), None)
        if current is None:
            data["queue"] = {"error": "group %s not found" % group}
        else:
            data["queue"] = {
                "lag": current.get("lag"),
                "pending": current.get("pending"),
                "consumers": current.get("consumers"),
            }
        data["stream_length"] = kube.redis_json(namespace, redis_deployment, "XLEN", stream)
    except (RuntimeError, ValueError) as error:
        data["queue"] = {"error": str(error)}
        data["stream_length"] = None

    try:
        sj = kube.json("-n", namespace, "get", "scaledjob", scaledjob, "-o", "json") or {}
        conditions = {
            item.get("type"): item.get("status")
            for item in (sj.get("status", {}).get("conditions") or [])
        }
        spec = sj.get("spec", {})
        data["scaledjob"] = {
            "ready": conditions.get("Ready"),
            "active": conditions.get("Active"),
            "paused": conditions.get("Paused"),
            "max": spec.get("maxReplicaCount"),
            "image": (
                spec.get("jobTargetRef", {})
                .get("template", {})
                .get("spec", {})
                .get("containers", [{}])[0]
                .get("image", "")
            ),
        }
    except (RuntimeError, ValueError, IndexError) as error:
        data["errors"].append("scaledjob: %s" % error)
        data["scaledjob"] = {}

    return data


def render(data: Dict[str, Any], limit: int, colors: bool, group: str) -> str:
    now = data["now"]
    lines: List[str] = []
    header = "agent-audit · %sZ" % now.strftime("%Y-%m-%d %H:%M:%S")
    lines.append(color(header, BOLD, colors))

    queue = data.get("queue") or {}
    if "error" in queue:
        queue_line = "queue    %s" % color(queue["error"], RED, colors)
    else:
        queue_line = "queue    lag %-4s pending %-4s stream %-6s group %s" % (
            queue.get("lag"),
            queue.get("pending"),
            data.get("stream_length"),
            group,
        )
    lines.append(queue_line)

    sj = data.get("scaledjob") or {}
    if sj:
        active = sj.get("active")
        lines.append(
            "scaled   ready=%-5s active=%-5s paused=%-5s max=%s"
            % (sj.get("ready"), active, sj.get("paused"), sj.get("max"))
        )

    counts = data.get("counts") or {}
    summary = "jobs     running %d  pending %d  complete %d  failed %d" % (
        counts.get("Running", 0),
        counts.get("Pending", 0),
        counts.get("Complete", 0),
        counts.get("Failed", 0),
    )
    if counts.get("Failed"):
        summary += color("   ! failures", RED, colors)
    lines.append(summary)
    lines.append("")

    rows = data.get("jobs") or []
    if rows:
        lines.append(
            color("%-38s %-9s %-7s %-7s %-10s" % ("NAME", "STATUS", "AGE", "DUR", "POD/ATT"), BOLD, colors)
        )
        for row in rows[:limit]:
            status = row["status"]
            code = {"Running": YELLOW, "Failed": RED, "Complete": GREEN}.get(status, DIM)
            lines.append(
                "%-38s %-9s %-7s %-7s %-10s"
                % (
                    row["name"][:38],
                    color(status, code, colors),
                    fmt_duration(row["age"]),
                    fmt_duration(row["duration"]),
                    row["pods"],
                )
            )
        if len(rows) > limit:
            lines.append(color("... %d more (--limit)" % (len(rows) - limit), DIM, colors))
    else:
        lines.append(color("no Jobs yet — waiting for queue lag", DIM, colors))

    for error in data.get("errors") or []:
        lines.append(color("error: %s" % error, RED, colors))

    if sj.get("image"):
        lines.append("")
        lines.append(color("image %s" % sj["image"], DIM, colors))

    return "\n".join(lines)


def main(argv: Optional[List[str]] = None) -> int:
    parser = argparse.ArgumentParser(description="Live, read-only view of the agent-audit KEDA batch.")
    parser.add_argument("--namespace", default="agent-audit")
    parser.add_argument("--stream", default="agent-audit:tasks")
    parser.add_argument("--group", default="agent-audit-runners")
    parser.add_argument("--scaledjob", default="agent-audit-runner")
    parser.add_argument("--redis-deployment", default="agent-audit-redis")
    parser.add_argument("--kubectl", default=os.environ.get("KUBECTL", "kubectl"),
                        help='kubectl binary, e.g. "k3s kubectl". Default: $KUBECTL or kubectl')
    parser.add_argument("--interval", type=float, default=5.0, help="refresh seconds. Default: 5")
    parser.add_argument("--limit", type=int, default=15, help="max Job rows. Default: 15")
    parser.add_argument("--once", action="store_true", help="print one snapshot and exit")
    args = parser.parse_args(argv)

    colors = sys.stdout.isatty() and os.environ.get("NO_COLOR") is None
    kube = Kubectl(args.kubectl)

    while True:
        try:
            data = collect(
                kube,
                args.namespace,
                args.stream,
                args.group,
                args.scaledjob,
                args.redis_deployment,
            )
        except Exception as error:  # keep the watch alive on transient API errors
            data = {
                "now": datetime.now(timezone.utc),
                "errors": [str(error)],
                "jobs": [],
                "counts": {},
                "queue": {"error": "unavailable"},
                "scaledjob": {},
            }

        text = render(data, args.limit, colors, args.group)
        if args.once:
            print(text)
            return 0

        if colors:
            sys.stdout.write("\033[H\033[2J")
        print(text, flush=True)
        time.sleep(max(args.interval, 1.0))


if __name__ == "__main__":
    raise SystemExit(main())
