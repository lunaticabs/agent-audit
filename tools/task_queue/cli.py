"""Command line for the runner-side task queue."""

from __future__ import annotations

import argparse
import json
import os
import sys
from typing import List, Optional

from . import queue as task_queue
from .resp import RedisError

EXIT_NO_TASK = 3


def _default_consumer() -> str:
    configured = os.environ.get("AGENT_AUDIT_QUEUE_CONSUMER", "").strip()
    if configured:
        return configured
    job = os.environ.get("JOB_NAME", "").strip()
    if job:
        return job
    if os.environ.get("KUBERNETES_SERVICE_HOST"):
        print(
            "task_queue: JOB_NAME is empty; falling back to the pod name. "
            "A Job retry will then claim a new task instead of the failed one.",
            file=sys.stderr,
            flush=True,
        )
    return os.environ.get("HOSTNAME", "").strip() or "runner"


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="python3 -m task_queue",
        description="Claim and acknowledge agent-audit tasks on a Redis Stream.",
    )
    parser.add_argument(
        "--redis-url",
        default=os.environ.get("AGENT_AUDIT_REDIS_URL", ""),
        help="redis://host:port/db. Default: $AGENT_AUDIT_REDIS_URL",
    )
    parser.add_argument(
        "--stream",
        default=os.environ.get("AGENT_AUDIT_QUEUE_STREAM", task_queue.DEFAULT_STREAM),
        help="Stream name. Default: %(default)s",
    )
    parser.add_argument(
        "--group",
        default=os.environ.get("AGENT_AUDIT_QUEUE_GROUP", task_queue.DEFAULT_GROUP),
        help="Consumer group. Default: %(default)s",
    )
    parser.add_argument(
        "--consumer",
        default=_default_consumer(),
        help="Consumer name. Default: $JOB_NAME or $HOSTNAME",
    )

    sub = parser.add_subparsers(dest="command", required=True)

    claim = sub.add_parser("claim", help="Claim one task and print it as JSON.")
    claim.add_argument(
        "--block-ms",
        type=int,
        default=int(os.environ.get("AGENT_AUDIT_QUEUE_BLOCK_MS", "5000")),
        help="How long to wait for a new task. Default: %(default)s",
    )

    ack = sub.add_parser("ack", help="Acknowledge one or more messages.")
    ack.add_argument("--message-id", action="append", required=True)

    pending = sub.add_parser("pending", help="Show unacknowledged messages.")
    pending.add_argument("--details", action="store_true", help="List each pending message.")
    pending.add_argument("--json", action="store_true", help="Machine-readable output.")

    requeue = sub.add_parser(
        "requeue",
        help="Copy a stranded pending message back onto the stream and ack the old id.",
    )
    requeue.add_argument("--message-id", required=True)

    return parser


def main(argv: Optional[List[str]] = None) -> int:
    args = build_parser().parse_args(argv)
    if not args.redis_url:
        print(
            "task_queue: AGENT_AUDIT_REDIS_URL is not set (or pass --redis-url)",
            file=sys.stderr,
        )
        return 2

    try:
        with task_queue.connect(args.redis_url) as client:
            if args.command == "claim":
                task_queue.ensure_group(client, args.stream, args.group, start_id="0")
                task = task_queue.claim(
                    client,
                    stream=args.stream,
                    group=args.group,
                    consumer=args.consumer,
                    block_ms=args.block_ms,
                )
                if task is None:
                    print(
                        "task_queue: no task available stream=%s group=%s consumer=%s"
                        % (args.stream, args.group, args.consumer),
                        file=sys.stderr,
                    )
                    return EXIT_NO_TASK
                print(task_queue.dumps(task))
                return 0

            if args.command == "ack":
                count = task_queue.ack(client, args.stream, args.group, args.message_id)
                print("acked=%d" % count)
                return 0

            if args.command == "pending":
                summary = task_queue.pending_summary(client, args.stream, args.group)
                details = (
                    task_queue.pending_details(client, args.stream, args.group)
                    if args.details
                    else []
                )
                if args.json:
                    print(json.dumps({"summary": summary, "messages": details}, ensure_ascii=False))
                else:
                    print(
                        "pending count=%d min=%s max=%s consumers=%s"
                        % (
                            summary["count"],
                            summary["min_id"],
                            summary["max_id"],
                            ",".join(
                                "%s:%d" % item for item in sorted(summary["consumers"].items())
                            )
                            or "-",
                        )
                    )
                    for item in details:
                        print(
                            "  %s consumer=%s idle_ms=%d deliveries=%d"
                            % (
                                item["message_id"],
                                item["consumer"],
                                item["idle_ms"],
                                item["deliveries"],
                            )
                        )
                return 0

            if args.command == "requeue":
                new_id = task_queue.requeue(client, args.stream, args.group, args.message_id)
                print("requeued %s -> %s" % (args.message_id, new_id))
                return 0
    except RedisError as error:
        print("task_queue: redis error: %s" % error, file=sys.stderr)
        return 1
    except ValueError as error:
        print("task_queue: %s" % error, file=sys.stderr)
        return 2

    return 2
