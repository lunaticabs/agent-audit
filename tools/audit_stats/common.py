from __future__ import annotations

import json
import math
import os
import re
import tempfile
from pathlib import Path
from typing import Any, Dict, Iterable, Iterator, List, Optional, Sequence, Tuple

# --- vocabularies -----------------------------------------------------------

SEVERITY_ORDER = ("critical", "high", "medium", "low", "informational", "unknown")
SEVERITY_RANK = {name: index for index, name in enumerate(SEVERITY_ORDER)}
HIGH_PLUS = {"critical", "high"}

SEVERITY_ALIASES = {
    "critical": "critical",
    "crit": "critical",
    "very high": "critical",
    "veryhigh": "critical",
    "severe": "critical",
    "high": "high",
    "medium": "medium",
    "med": "medium",
    "moderate": "medium",
    "low": "low",
    "informational": "informational",
    "informative": "informational",
    "info": "informational",
    "note": "informational",
}

CONFIDENCE_ORDER = ("confirmed", "likely", "unlikely", "unknown")

CONFIDENCE_ALIASES = {
    "confirmed": "confirmed",
    "certain": "confirmed",
    "high": "confirmed",
    "verified": "confirmed",
    "likely": "likely",
    "probable": "likely",
    "medium": "likely",
    "possible": "likely",
    "unlikely": "unlikely",
    "low": "unlikely",
    "speculative": "unlikely",
}

REPORT_STATUSES = {"completed", "incomplete", "blocked"}

#: Required top-level fields of the report contract in
#: docker/dsh/skills/evm-audit/references/report.md.
REPORT_REQUIRED_FIELDS = (
    "run_id",
    "target",
    "status",
    "summary",
    "findings",
    "hypotheses",
    "limitations",
    "coverage",
)

MAX_TEXT_BYTES = 8 * 1024 * 1024
MAX_EVIDENCE_SAMPLES = 8


def utc_now_iso() -> str:
    from datetime import datetime, timezone

    return datetime.now(timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")


# --- normalization ----------------------------------------------------------


def _normalize_token(raw: Any) -> str:
    if not isinstance(raw, str):
        return ""
    token = raw.strip().lower()
    token = token.replace("_", " ").replace("-", " ")
    token = re.sub(r"\s+", " ", token)
    return token


def normalize_severity(raw: Any) -> Tuple[str, bool]:
    """Map a report severity onto the canonical vocabulary.

    Returns (canonical, recognized). Unrecognized and missing values become
    ``unknown`` so that the aggregate never silently drops a finding.
    """
    token = _normalize_token(raw)
    if not token:
        return "unknown", False
    canonical = SEVERITY_ALIASES.get(token)
    if canonical is not None:
        return canonical, True
    # "very-high" / "Very High" already collapse above; check a prefix as a
    # last resort for values like "high (confirmed)".
    for alias, mapped in SEVERITY_ALIASES.items():
        if token.startswith(alias + " "):
            return mapped, True
    return "unknown", False


def normalize_confidence(raw: Any) -> Tuple[str, bool]:
    token = _normalize_token(raw)
    if not token:
        return "unknown", False
    canonical = CONFIDENCE_ALIASES.get(token)
    if canonical is not None:
        return canonical, True
    for alias, mapped in CONFIDENCE_ALIASES.items():
        if token.startswith(alias + " "):
            return mapped, True
    return "unknown", False


def coerce_bool(raw: Any) -> Tuple[Optional[bool], bool]:
    """Return (value, valid). Missing/None is (None, True); junk is (None, False)."""
    if raw is None:
        return None, True
    if isinstance(raw, bool):
        return raw, True
    if isinstance(raw, str):
        token = raw.strip().lower()
        if token in {"true", "yes", "1"}:
            return True, True
        if token in {"false", "no", "0"}:
            return False, True
    if isinstance(raw, (int, float)) and raw in (0, 1):
        return bool(raw), True
    return None, False


def as_list(value: Any) -> List[Any]:
    if isinstance(value, list):
        return value
    if value is None:
        return []
    return [value]


def as_text(value: Any) -> str:
    if isinstance(value, str):
        return value
    if isinstance(value, (int, float)):
        return str(value)
    return ""


# --- filesystem -------------------------------------------------------------


def iter_run_dirs(runs_root: Path) -> Iterator[Path]:
    """Yield candidate run directories.

    Names starting with ``_`` or ``.`` are reserved for derived data (for
    example ``runs/_stats``) and are skipped.
    """
    if not runs_root.is_dir():
        return
    for child in sorted(runs_root.iterdir()):
        if not child.is_dir():
            continue
        if child.name.startswith("_") or child.name.startswith("."):
            continue
        yield child


def _strip_ref_annotation(text: str) -> str:
    """Drop line/column and prose annotations from an evidence reference.

    Reports cite evidence as `path:12`, `path:12-34`, `path#L12-L30`, or
    `path (lines 12-34)`. Only the path is a filesystem object.
    """
    stripped = text.strip()
    stripped = re.sub(r"\s+\([^()]*\)\s*$", "", stripped)
    stripped = re.sub(r"#L\d+(?:-L?\d+)?$", "", stripped)
    stripped = re.sub(r":\d+(?:-\d+)?(?:,\s*\d+(?:-\d+)?)*$", "", stripped)
    stripped = re.sub(
        r"\s+lines?\s+\d+(?:\s*[-\u2013]\s*\d+)?$", "", stripped, flags=re.IGNORECASE
    )
    return stripped.strip()


def _check_evidence_path(run_dir: Path, text: str) -> Tuple[str, Optional[Path]]:
    if not text or "\x00" in text:
        return "invalid", None
    if os.path.isabs(text) or re.match(r"^[A-Za-z]:[\\/]", text):
        return "absolute", None

    normalized = os.path.normpath(text.replace("\\", "/"))
    if normalized == ".." or normalized.startswith("../"):
        return "escape", None

    target = run_dir / normalized
    try:
        if not target.exists():
            return "missing", target
        real_run = os.path.realpath(str(run_dir))
        real_target = os.path.realpath(str(target))
        if os.path.commonpath([real_run, real_target]) != real_run:
            return "escape", target
    except (OSError, ValueError):
        return "invalid", target
    return "ok", target


def resolve_evidence(run_dir: Path, ref: Any) -> Tuple[str, Optional[Path]]:
    """Resolve an evidence reference inside a run directory.

    Returns ``(status, path)`` where status is one of ``ok``, ``missing``,
    ``escape``, ``absolute``, ``invalid``.

    The agent controls the report, so a reference is never followed outside
    the run directory: absolute paths, ``..`` traversal, and symlinks that
    point out of the tree are all rejected. Line suffixes and trailing
    parenthetical notes are stripped before the check.
    """
    if not isinstance(ref, str):
        return "invalid", None
    text = ref.strip()
    if not text:
        return "invalid", None

    stripped = _strip_ref_annotation(text)
    candidates = [stripped] if stripped == text or not stripped else [stripped, text]
    first_status: Optional[Tuple[str, Optional[Path]]] = None
    for candidate in candidates:
        status, target = _check_evidence_path(run_dir, candidate)
        if status == "ok":
            return status, target
        if first_status is None:
            first_status = (status, target)
    return first_status if first_status is not None else ("invalid", None)


def read_text_capped(path: Path, limit: int = MAX_TEXT_BYTES) -> Optional[str]:
    try:
        with path.open("rb") as handle:
            data = handle.read(limit)
    except OSError:
        return None
    return data.decode("utf-8", errors="replace")


def read_json(path: Path) -> Tuple[Optional[Any], Optional[str]]:
    """Read a JSON file. Returns (value, error)."""
    if not path.exists():
        return None, "missing"
    try:
        with path.open("r", encoding="utf-8", errors="replace") as handle:
            return json.load(handle), None
    except json.JSONDecodeError as error:
        return None, "invalid_json: {}".format(error)
    except OSError as error:
        return None, "unreadable: {}".format(error)


def atomic_write_json(path: Path, payload: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp_fd, tmp_name = tempfile.mkstemp(prefix=".metrics-", suffix=".tmp", dir=str(path.parent))
    try:
        with os.fdopen(tmp_fd, "w", encoding="utf-8") as handle:
            json.dump(payload, handle, indent=2, sort_keys=False, ensure_ascii=False)
            handle.write("\n")
            handle.flush()
            os.fsync(handle.fileno())
        # mkstemp creates 0600; match the rest of the run directory so a
        # different collector user can read the derived metrics.
        os.chmod(tmp_name, 0o644)
        os.replace(tmp_name, str(path))
    except BaseException:
        try:
            os.unlink(tmp_name)
        except OSError:
            pass
        raise


def dir_size(path: Path) -> Tuple[int, int]:
    """Return (file_count, byte_count) for a directory, without following dir symlinks."""
    files = 0
    size = 0
    if not path.is_dir():
        return 0, 0
    for root, dirnames, filenames in os.walk(str(path)):
        dirnames[:] = [name for name in dirnames if not os.path.islink(os.path.join(root, name))]
        for filename in filenames:
            full = os.path.join(root, filename)
            try:
                stat = os.lstat(full)
            except OSError:
                continue
            files += 1
            size += stat.st_size
    return files, size


def strip_ansi(text: str) -> str:
    return re.sub(r"\x1b\[[0-9;]*[A-Za-z]", "", text)


# --- numerics ---------------------------------------------------------------


def percentile(values: Sequence[float], fraction: float) -> Optional[float]:
    """Nearest-rank percentile. Returns None for an empty sample."""
    if not values:
        return None
    ordered = sorted(values)
    if fraction <= 0:
        return ordered[0]
    if fraction >= 1:
        return ordered[-1]
    index = math.ceil(fraction * len(ordered)) - 1
    index = max(0, min(len(ordered) - 1, index))
    return ordered[index]


def ratio(numerator: int, denominator: int) -> Optional[float]:
    if denominator <= 0:
        return None
    return numerator / denominator


def format_pct(value: Optional[float]) -> str:
    if value is None:
        return "n/a"
    return "{:.1f}%".format(value * 100.0)


def format_seconds(value: Optional[float]) -> str:
    if value is None:
        return "n/a"
    value = float(value)
    if value < 60:
        return "{:.0f}s".format(value)
    minutes = value / 60.0
    if minutes < 60:
        return "{:.1f}m".format(minutes)
    return "{:.1f}h".format(minutes / 60.0)
