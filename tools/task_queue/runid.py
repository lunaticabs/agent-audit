"""Run-directory naming for queued tasks.

This is a byte-for-byte port of ``run_id_for_task`` in
``dispatcher/src/main.rs`` so a queue message produces the same run directory
name it produced under the old push-based dispatcher. Keeping the two in sync
matters for old batches: their run directories are already on disk under these
names.
"""

from __future__ import annotations

import hashlib

MAX_RUN_ID_LEN = 80
JOB_NAME_PREFIX = "agent-audit-"


def sanitize_task_id(task_id: str) -> str:
    out = []
    last_dash = False
    for char in task_id:
        code = ord(char)
        if 48 <= code <= 57 or 97 <= code <= 122:  # 0-9 a-z
            out.append(char)
            last_dash = False
        elif 65 <= code <= 90:  # A-Z -> a-z (ASCII only, like to_ascii_lowercase)
            out.append(chr(code + 32))
            last_dash = False
        elif not last_dash:
            out.append("-")
            last_dash = True
    sanitized = "".join(out).strip("-")
    return sanitized or "task"


def short_hash(value: str) -> str:
    return hashlib.sha256(value.encode("utf-8")).hexdigest()[:8]


def run_id_for_task(task_id: str) -> str:
    sanitized = sanitize_task_id(task_id)
    if len(sanitized) <= MAX_RUN_ID_LEN:
        return sanitized

    suffix = short_hash(task_id)
    body_limit = max(MAX_RUN_ID_LEN - (len(suffix) + 1), 1)
    truncated = sanitized[:body_limit].rstrip("-")
    return "%s-%s" % (truncated or "task", suffix)
