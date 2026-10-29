"""Solidity source and `forge test` output parsing shared by collect and replay.

Two properties matter here and both are tested:

* A fork marker only counts when it sits inside a test function body. A comment
  or an unused import must not turn a run into a "fork-backed" one.
* `forge test` output is the only execution signal the collector has, so it is
  parsed once, in one place, and reused by the post-exit replay.
"""

from __future__ import annotations

import hashlib
import re
from pathlib import Path
from typing import Any, Dict, Iterator, List, Optional, Sequence, Tuple

#: Fork markers that count only when they appear inside a test body.
FORK_MARKERS = (
    "createSelectFork",
    "createFork",
    "forkBlockNumber",
    'envString("RPC_URL")',
    "envString('RPC_URL')",
    "fork-url",
)

#: Runtime signals that a log really came from a forked run. The replay writes
#: the header itself; an agent log can only satisfy these by echoing a command
#: or printing the block it forked at.
FORK_LOG_PATTERNS = (
    re.compile(r"--fork(-block-number|-url)?\b"),
    re.compile(r"\[agent-audit replay\]", re.IGNORECASE),
    re.compile(r"\bfork[_ -]?block(?:[_ -]?number)?\s*[=:]\s*\d+", re.IGNORECASE),
    re.compile(r"\bblock\.number\s*[=:]\s*\d+", re.IGNORECASE),
    re.compile(r"\bcreateSelectFork\b"),
)

TEST_NAME_RE = re.compile(r"\b(test[A-Za-z0-9_]+)")
_FUNCTION_HEAD_RE = re.compile(
    r"\b(?:function|constructor|modifier)\s+([A-Za-z_][A-Za-z0-9_]*)?\s*\("
)
FORGE_TEST_LINE_RE = re.compile(r"^\[(PASS|FAIL|SKIP)\b[^\]]*\]\s+([A-Za-z0-9_]+)")
FORGE_SUITE_RE = re.compile(
    r"^Suite result:\s*(\w+)\.\s*(\d+) passed;\s*(\d+) failed;\s*(\d+) skipped"
)
FORGE_RUNS_RE = re.compile(r"\bruns:\s*(\d+)")


def sha256_file(path: Path, limit: int = 16 * 1024 * 1024) -> Optional[str]:
    """Hash a file's bytes. Returns None when it cannot be read."""
    digest = hashlib.sha256()
    try:
        with path.open("rb") as handle:
            remaining = limit
            while remaining > 0:
                chunk = handle.read(min(65536, remaining))
                if not chunk:
                    break
                digest.update(chunk)
                remaining -= len(chunk)
    except OSError:
        return None
    return digest.hexdigest()


# --- comment stripping ------------------------------------------------------


def strip_solidity_comments(text: str) -> str:
    """Remove ``//`` and ``/* */`` comments while keeping string literals.

    Newlines are preserved so that a marker inside a comment leaves an empty
    line rather than gluing two identifiers together.
    """
    out: List[str] = []
    index = 0
    length = len(text)

    while index < length:
        char = text[index]
        if char == "/" and index + 1 < length and text[index + 1] == "/":
            newline = text.find("\n", index)
            if newline < 0:
                break
            out.append("\n")
            index = newline + 1
        elif char == "/" and index + 1 < length and text[index + 1] == "*":
            close = text.find("*/", index + 2)
            if close < 0:
                break
            out.append("\n" * text.count("\n", index, close + 2))
            index = close + 2
        elif char in "\"'":
            quote = char
            cursor = index + 1
            while cursor < length:
                if text[cursor] == "\\":
                    cursor += 2
                    continue
                if text[cursor] == quote:
                    cursor += 1
                    break
                cursor += 1
            out.append(text[index:cursor])
            index = cursor
        else:
            out.append(char)
            index += 1

    return "".join(out)


# --- test function bodies ---------------------------------------------------


def _skip_string(text: str, index: int) -> int:
    quote = text[index]
    cursor = index + 1
    while cursor < len(text):
        if text[cursor] == "\\":
            cursor += 2
            continue
        if text[cursor] == quote:
            return cursor + 1
        cursor += 1
    return cursor


def _body_start(text: str, index: int) -> int:
    """Index of a function's opening brace, or -1 for a declaration."""
    depth = 0
    cursor = index
    while cursor < len(text):
        char = text[cursor]
        if char in "\"'":
            cursor = _skip_string(text, cursor)
            continue
        if char == "(":
            depth += 1
        elif char == ")":
            depth = max(0, depth - 1)
        elif char == ";" and depth == 0:
            return -1
        elif char == "{" and depth == 0:
            return cursor
        cursor += 1
    return -1


def _matching_brace(text: str, open_index: int) -> int:
    depth = 0
    cursor = open_index
    while cursor < len(text):
        char = text[cursor]
        if char in "\"'":
            cursor = _skip_string(text, cursor)
            continue
        if char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return cursor
        cursor += 1
    return -1


def iter_functions(text: str) -> Iterator[Tuple[str, str, int]]:
    """Yield ``(name, body, body_start)`` for every function-like body.

    ``setUp()`` and internal helpers are included: forking inside `setUp` is the
    most common Foundry pattern, so restricting this to `test*` would reject
    real harnesses. A marker in a comment, or at file scope outside any body,
    still does not appear here.

    Comments are stripped first, and that is load-bearing: an apostrophe in a
    comment ("Alice's allowance") would otherwise open a string literal and
    swallow the rest of the file, silently dropping every later function.
    """
    text = strip_solidity_comments(text)
    for match in _FUNCTION_HEAD_RE.finditer(text):
        name = match.group(1) or "(anonymous)"
        brace = _body_start(text, match.end())
        if brace < 0:
            continue
        close = _matching_brace(text, brace)
        if close < 0:
            continue
        yield name, text[brace + 1 : close], brace


def iter_test_functions(text: str) -> Iterator[Tuple[str, str, int]]:
    """Yield only the ``function testXxx(...)`` bodies."""
    for name, body, start in iter_functions(text):
        if name.startswith("test"):
            yield name, body, start


def test_names(text: str) -> List[str]:
    return sorted({name for name, _, _ in iter_test_functions(text)})


# --- fork signal classification --------------------------------------------


def fork_signal_sites(raw_text: str) -> Dict[str, Any]:
    """Classify fork markers by where they appear.

    ``in_functions`` is the only list that may make a run fork-backed. The other
    two exist so the collector can say *why* a marker was rejected instead of
    silently ignoring it.
    """
    stripped = strip_solidity_comments(raw_text)

    in_functions: List[Dict[str, str]] = []
    seen: set = set()
    for name, body, _ in iter_functions(stripped):
        for marker in FORK_MARKERS:
            key = (marker, name)
            if key in seen:
                continue
            if marker in body:
                seen.add(key)
                in_functions.append({"marker": marker, "function": name})

    outside_functions: List[str] = []
    comment_only: List[str] = []
    for marker in FORK_MARKERS:
        if marker not in raw_text:
            continue
        if marker in stripped:
            if not any(item["marker"] == marker for item in in_functions):
                outside_functions.append(marker)
        else:
            comment_only.append(marker)

    return {
        "in_functions": in_functions,
        "outside_functions": outside_functions,
        "comment_only": comment_only,
    }


def fork_log_signals(text: str) -> List[str]:
    """Return the fork-related patterns a log actually contains."""
    signals: List[str] = []
    for pattern in FORK_LOG_PATTERNS:
        if pattern.search(text):
            signals.append(pattern.pattern)
    return signals


# --- forge output -----------------------------------------------------------


def merge_test_status(current: Optional[str], incoming: str) -> str:
    rank = {"PASS": 0, "SKIP": 1, "FAIL": 2}
    if current is None:
        return incoming
    return incoming if rank.get(incoming, 0) >= rank.get(current, 0) else current


def parse_forge_log(text: str) -> Dict[str, Any]:
    """Parse `forge test` stdout into per-test statuses and suite totals."""
    from .common import strip_ansi

    status: Dict[str, str] = {}
    fuzz_runs: Dict[str, int] = {}
    suites: List[Dict[str, Any]] = []

    for raw_line in strip_ansi(text).splitlines():
        line = raw_line.strip()
        match = FORGE_TEST_LINE_RE.match(line)
        if match:
            name = match.group(2)
            status[name] = merge_test_status(status.get(name), match.group(1))
            runs = FORGE_RUNS_RE.search(line)
            if runs:
                fuzz_runs[name] = max(fuzz_runs.get(name, 0), int(runs.group(1)))
            continue
        suite = FORGE_SUITE_RE.match(line)
        if suite:
            suites.append(
                {
                    "status": suite.group(1),
                    "passed": int(suite.group(2)),
                    "failed": int(suite.group(3)),
                    "skipped": int(suite.group(4)),
                }
            )

    return {"tests": status, "fuzz_runs": fuzz_runs, "suites": suites}


def merge_forge_logs(texts: Sequence[str]) -> Dict[str, Any]:
    """Merge several forge logs; a later FAIL is not hidden by an earlier PASS."""
    merged: Dict[str, str] = {}
    fuzz_runs: Dict[str, int] = {}
    suites: List[Dict[str, Any]] = []
    for text in texts:
        parsed = parse_forge_log(text)
        for name, value in parsed["tests"].items():
            merged[name] = merge_test_status(merged.get(name), value)
        for name, value in parsed["fuzz_runs"].items():
            fuzz_runs[name] = max(fuzz_runs.get(name, 0), value)
        suites.extend(parsed["suites"])
    return {"tests": merged, "fuzz_runs": fuzz_runs, "suites": suites}
