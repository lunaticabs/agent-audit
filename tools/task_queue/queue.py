"""Claim / acknowledge / inspect tasks on the Redis Stream.

Delivery semantics, deliberately:

* A Job claims **one** message. The first attempt reads ``>`` (new work); a
  retry reads ``0`` (this consumer's pending list) first, so the second pod of
  the same Kubernetes Job re-runs the *same* task instead of stealing a new
  one. The consumer name is therefore the Job name, which is stable across pod
  retries inside a Job.
* The message is acknowledged only after the audit finished successfully. A
  failed or timed-out attempt leaves it pending for the retry.
* Malformed messages are acknowledged and skipped, otherwise a poison pill
  would be retried forever.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from typing import Any, Dict, List, Optional, Tuple
from urllib.parse import unquote, urlparse

from .resp import RedisClient, RedisError, as_text, split_stream_reply
from .runid import run_id_for_task

DEFAULT_STREAM = "agent-audit:tasks"
DEFAULT_GROUP = "agent-audit-runners"
DEFAULT_BLOCK_MS = 5000


class NoTask(Exception):
    """The queue has nothing to hand out right now."""


class InvalidTask(Exception):
    """A stream entry is missing its required fields."""


@dataclass
class Task:
    message_id: str
    task_id: str
    full_prompt: str
    image: Optional[str]
    image_tag: Optional[str]
    attempt: int
    run_id: str
    stream: str
    group: str
    consumer: str

    def as_dict(self) -> Dict[str, Any]:
        return {
            "message_id": self.message_id,
            "task_id": self.task_id,
            "full_prompt": self.full_prompt,
            "image": self.image,
            "image_tag": self.image_tag,
            "attempt": self.attempt,
            "run_id": self.run_id,
            "stream": self.stream,
            "group": self.group,
            "consumer": self.consumer,
        }


def parse_redis_url(url: str) -> Tuple[str, int, int, Optional[str]]:
    """Parse ``redis://[:password@]host:port/db`` without any dependency."""

    parsed = urlparse(url)
    if parsed.scheme not in {"redis", ""}:
        raise ValueError(
            "unsupported redis url scheme %r (expected redis://)" % parsed.scheme
        )
    if not parsed.hostname:
        raise ValueError("redis url is missing a host: %r" % url)
    return (
        parsed.hostname,
        parsed.port or 6379,
        int((parsed.path or "/0").lstrip("/") or 0),
        unquote(parsed.password) if parsed.password else None,
    )


def connect(url: str, timeout: Optional[float] = None) -> RedisClient:
    host, port, db, password = parse_redis_url(url)
    return RedisClient(host=host, port=port, db=db, password=password, timeout=timeout or 10.0)


def entries_added(client: RedisClient, stream: str) -> Optional[int]:
    """Total entries ever added to the stream (``XINFO STREAM``)."""

    reply = client.execute("XINFO", "STREAM", stream)
    if isinstance(reply, list):
        for index in range(0, len(reply) - 1, 2):
            if as_text(reply[index]) == "entries-added":
                try:
                    return int(reply[index + 1])
                except (TypeError, ValueError):
                    return None
    length = client.execute("XLEN", stream)
    return int(length) if isinstance(length, int) else None


def ensure_group(
    client: RedisClient,
    stream: str,
    group: str,
    start_id: str = "$",
) -> bool:
    """Create the consumer group if it is missing. Returns True when created.

    ``XGROUP CREATE ... $`` leaves ``entries-read`` unset. Redis can still
    compute lag on an untrimmed stream, but after a trim it reports ``lag nil``
    and KEDA (which scales on lag) never activates. Pinning ``entries-read`` to
    the stream's ``entries-added`` makes lag deterministic from the first poll.
    """

    try:
        client.execute("XGROUP", "CREATE", stream, group, start_id, "MKSTREAM")
        created = True
    except RedisError as error:
        if "BUSYGROUP" in error.message:
            return False
        raise

    if created and start_id == "$":
        added = entries_added(client, stream)
        if added is not None:
            client.execute("XGROUP", "SETID", stream, group, "$", "ENTRIESREAD", str(added))
    return True



def claim(
    client: RedisClient,
    stream: str = DEFAULT_STREAM,
    group: str = DEFAULT_GROUP,
    consumer: str = "runner",
    block_ms: int = DEFAULT_BLOCK_MS,
    max_skipped: int = 20,
) -> Optional[Task]:
    block = max(int(block_ms), 0)
    for _ in range(max_skipped):
        entry, attempt = _read_one(client, stream, group, consumer, block)
        if entry is None:
            return None
        message_id, fields = entry
        try:
            return _build_task(message_id, fields, attempt, stream, group, consumer)
        except InvalidTask as error:
            # Poison pill: drop it, report it, keep the runner alive.
            ack(client, stream, group, [message_id])
            print(
                "task_queue: dropped invalid message id=%s consumer=%s error=%s"
                % (message_id, consumer, error),
                flush=True,
            )
            block = 0
    return None


def _read_one(
    client: RedisClient,
    stream: str,
    group: str,
    consumer: str,
    block_ms: int,
) -> Tuple[Optional[Tuple[str, List[Any]]], int]:
    # 1. This consumer's own pending entries: the retry path.
    own = split_stream_reply(
        client.execute(
            "XREADGROUP", "GROUP", group, consumer, "COUNT", "1", "STREAMS", stream, "0"
        )
    )
    if own and own[0][1]:
        return own[0][1][0], 2

    # 2. New work.
    if block_ms > 0:
        client.set_timeout(block_ms / 1000.0 + 15.0)
        reply = client.execute(
            "XREADGROUP",
            "GROUP",
            group,
            consumer,
            "COUNT",
            "1",
            "BLOCK",
            str(block_ms),
            "STREAMS",
            stream,
            ">",
        )
    else:
        reply = client.execute(
            "XREADGROUP", "GROUP", group, consumer, "COUNT", "1", "STREAMS", stream, ">"
        )
    new = split_stream_reply(reply)
    if new and new[0][1]:
        return new[0][1][0], 1
    return None, 0


def _build_task(
    message_id: str,
    fields: List[Any],
    attempt: int,
    stream: str,
    group: str,
    consumer: str,
) -> Task:
    if len(fields) % 2 != 0:
        raise InvalidTask("odd number of field/value pairs")
    data: Dict[str, str] = {}
    for index in range(0, len(fields), 2):
        key = as_text(fields[index])
        if key is None:
            continue
        value = as_text(fields[index + 1])
        data[key] = value if value is not None else ""

    task_id = data.get("task_id", "").strip()
    if not task_id:
        raise InvalidTask("task_id is required")
    full_prompt = data.get("full_prompt", "")
    if not full_prompt.strip():
        raise InvalidTask("full_prompt must not be blank")

    image = data.get("image", "").strip() or None
    image_tag = data.get("image_tag", "").strip() or None
    return Task(
        message_id=message_id,
        task_id=task_id,
        full_prompt=full_prompt,
        image=image,
        image_tag=image_tag,
        attempt=attempt,
        run_id=run_id_for_task(task_id),
        stream=stream,
        group=group,
        consumer=consumer,
    )


def ack(
    client: RedisClient,
    stream: str,
    group: str,
    message_ids: List[str],
) -> int:
    if not message_ids:
        return 0
    reply = client.execute("XACK", stream, group, *message_ids)
    return int(reply) if isinstance(reply, int) else 0


def pending_summary(client: RedisClient, stream: str, group: str) -> Dict[str, Any]:
    reply = client.execute("XPENDING", stream, group)
    if not isinstance(reply, list) or len(reply) < 4:
        return {"count": 0, "min_id": None, "max_id": None, "consumers": {}}
    consumers: Dict[str, int] = {}
    for item in reply[3] or []:
        if isinstance(item, list) and len(item) == 2:
            name = as_text(item[0]) or ""
            consumers[name] = int(item[1])
    return {
        "count": int(reply[0] or 0),
        "min_id": as_text(reply[1]),
        "max_id": as_text(reply[2]),
        "consumers": consumers,
    }


def pending_details(
    client: RedisClient, stream: str, group: str, count: int = 100
) -> List[Dict[str, Any]]:
    reply = client.execute("XPENDING", stream, group, "-", "+", str(count))
    details: List[Dict[str, Any]] = []
    for item in reply or []:
        if not isinstance(item, list) or len(item) < 4:
            continue
        details.append(
            {
                "message_id": as_text(item[0]),
                "consumer": as_text(item[1]),
                "idle_ms": int(item[2]),
                "deliveries": int(item[3]),
            }
        )
    return details


def requeue(
    client: RedisClient,
    stream: str,
    group: str,
    message_id: str,
) -> str:
    """Copy a stranded pending message back onto the stream, then ack the old one.

    This is the manual escape hatch for a task whose Job failed twice. It
    intentionally creates a new message id; the old entry is acknowledged so it
    does not show up as pending forever.
    """

    reply = client.execute("XRANGE", stream, message_id, message_id)
    entries = reply or []
    if not entries:
        raise RedisError(
            "message %s is no longer in %s (trimmed?); cannot requeue" % (message_id, stream)
        )
    _, fields = entries[0]
    if len(fields) % 2 != 0:
        raise RedisError("message %s has malformed fields" % message_id)
    parts = ["XADD", stream, "*"]
    for index in range(0, len(fields), 2):
        key = as_text(fields[index]) or ""
        value = as_text(fields[index + 1]) or ""
        parts.extend([key, value])
    new_id = client.execute(*parts)
    ack(client, stream, group, [message_id])
    return as_text(new_id) or ""


def dumps(task: Optional[Task]) -> str:
    return json.dumps(task.as_dict() if task else None, ensure_ascii=False)
