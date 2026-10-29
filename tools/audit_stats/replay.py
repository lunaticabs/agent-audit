"""Post-exit fork replay for one audit run.

The agent writes its own `forge` output into `artifacts/`. Those files live in
the same directory tree the agent can write, so a passing log is evidence that
*something* ran, not evidence that the run reproduced the finding now. This
module rebuilds that evidence after the agent process has exited:

1. discover the Foundry project(s) the agent built inside the run directory;
2. re-run only the tests each finding claims, against `RPC_URL`, at the pinned
   fork block (or latest), writing our own log under `artifacts/replay/`;
3. repeat at `block - delta` and record whether the same test fails, which is a
   diagnostic for whether the assertion actually reads chain state;
4. record the sha256 of each harness file so the result is bound to a version.

The output is `artifacts/replay/replay.json`. It is deliberately written outside
`stats/`: `stats/` is disposable derived data, while the replay record is the
attestation the collector trusts. Everything here is best-effort — a replay
failure must never change the audit's exit status.
"""

from __future__ import annotations

import os
import re
import shutil
import subprocess
import time
from pathlib import Path
from typing import Any, Dict, List, Optional, Sequence, Tuple
from urllib.parse import urlsplit, urlunsplit

from .common import atomic_write_json, read_json, read_text_capped, utc_now_iso
from .forge import TEST_NAME_RE, merge_forge_logs, sha256_file

REPLAY_SCHEMA_VERSION = 1
REPLAY_DIR_RELATIVE = Path("artifacts") / "replay"
REPLAY_JSON_RELATIVE = REPLAY_DIR_RELATIVE / "replay.json"

#: The header every replay log starts with. `collect` also accepts it as a
#: runtime fork signal, so a log without it cannot claim to be a replay.
LOG_HEADER = "[agent-audit replay]"

DEFAULT_BUDGET_MS = 900_000
DEFAULT_DELTA_BLOCKS = 7_200
MIN_DELTA_BUDGET_MS = 20_000

EXCLUDED_DIRS = {
    "lib",
    "out",
    "cache",
    "build",
    "broadcast",
    "node_modules",
    "vendor",
    "vendor-built",
    ".git",
}


def _env_int(name: str, default: int) -> int:
    raw = os.environ.get(name)
    if not raw:
        return default
    try:
        return int(str(raw).strip())
    except ValueError:
        return default


def budget_ms() -> int:
    return max(30_000, _env_int("AGENT_AUDIT_REPLAY_TIMEOUT_MS", DEFAULT_BUDGET_MS))


def default_delta() -> int:
    return max(0, _env_int("AGENT_AUDIT_REPLAY_BLOCK_DELTA", DEFAULT_DELTA_BLOCKS))


# --- discovery --------------------------------------------------------------


def discover_foundry_roots(run_dir: Path, limit: int = 8) -> List[Path]:
    """Return directories inside the run that hold a `foundry.toml`."""
    roots: List[Path] = []
    for root, dirnames, filenames in os.walk(str(run_dir)):
        dirnames[:] = sorted(
            name for name in dirnames if name not in EXCLUDED_DIRS and not name.startswith(".")
        )
        if "foundry.toml" in filenames:
            roots.append(Path(root))
            if len(roots) >= limit:
                break
    return roots


def harness_tests_by_root(run_dir: Path, roots: Sequence[Path]) -> Dict[str, List[str]]:
    """Map each Foundry root to the test names declared under it."""
    from .forge import iter_test_functions

    by_root: Dict[str, List[str]] = {}
    for root in roots:
        names: List[str] = []
        for dirpath, dirnames, filenames in os.walk(str(root)):
            dirnames[:] = sorted(
                name for name in dirnames if name not in EXCLUDED_DIRS and not name.startswith(".")
            )
            for filename in sorted(filenames):
                if not filename.endswith(".t.sol"):
                    continue
                text = read_text_capped(Path(dirpath) / filename)
                if not text:
                    continue
                names.extend(name for name, _body, _index in iter_test_functions(text))
        by_root[str(root)] = sorted(set(names))
    return by_root


def linked_tests(report: Dict[str, Any]) -> List[str]:
    """Test names the report claims to reproduce findings with."""
    names: set = set()
    for finding in report.get("findings") or []:
        if not isinstance(finding, dict):
            continue
        raw = finding.get("reproduction", finding.get("repro", ""))
        values = raw if isinstance(raw, list) else [raw]
        text = " ".join(item for item in values if isinstance(item, str))
        names.update(TEST_NAME_RE.findall(text))
    return sorted(names)


BLOCK_KEYS = ("block", "block_number", "blockNumber", "number", "fork_block", "forkBlockNumber")


def _coerce_block(value: Any) -> Optional[int]:
    if isinstance(value, bool):
        return None
    if isinstance(value, int):
        return value if value > 0 else None
    if isinstance(value, str):
        text = value.strip()
        if not text:
            return None
        try:
            number = int(text, 16) if text.lower().startswith("0x") else int(text)
        except ValueError:
            return None
        return number if number > 0 else None
    return None


def read_pinned_block(run_dir: Path) -> Optional[int]:
    """Read the fork block the agent pinned.

    Two shapes are in the wild: a compact `{"block": 21000000}` and the raw
    `eth_getBlockByNumber` response an agent saved (`{"data": {"number":
    "0x1910921"}}`). Both are accepted, including hex numbers.
    """
    payload, error = read_json(run_dir / "artifacts" / "pinned_block.json")
    if error or not isinstance(payload, dict):
        return None

    sources = [payload]
    for key in ("data", "result"):
        nested = payload.get(key)
        if isinstance(nested, dict):
            sources.append(nested)

    for source in sources:
        for key in BLOCK_KEYS:
            number = _coerce_block(source.get(key))
            if number is not None:
                return number
    return None


def resolve_block(cast_bin: Optional[str], rpc_url: str, run_dir: Path) -> Tuple[Optional[int], Optional[str]]:
    """Pinned block file first, then the chain head. Returns (block, source)."""
    pinned = read_pinned_block(run_dir)
    if pinned is not None:
        return pinned, "pinned_block_file"

    binary = cast_bin or shutil.which("cast")
    if not binary:
        return None, None
    try:
        completed = subprocess.run(
            [binary, "block-number", "--rpc-url", rpc_url],
            capture_output=True,
            text=True,
            timeout=30,
        )
    except (OSError, subprocess.SubprocessError):
        return None, None
    if completed.returncode != 0:
        return None, None
    text = (completed.stdout or "").strip().splitlines()
    if not text:
        return None, None
    try:
        return int(text[-1].strip()), "rpc_latest"
    except ValueError:
        return None, None


def _redact_rpc(rpc_url: str) -> str:
    try:
        parts = urlsplit(rpc_url)
    except ValueError:
        return "<unparseable>"
    return urlunsplit((parts.scheme, parts.netloc, "/redacted", "", ""))


def _test_file_index(run_dir: Path, roots: Sequence[Path]) -> Dict[str, str]:
    """Map a test name to the harness file that declares it."""
    from .forge import iter_test_functions

    index: Dict[str, str] = {}
    for root in roots:
        for dirpath, dirnames, filenames in os.walk(str(root)):
            dirnames[:] = sorted(
                name for name in dirnames if name not in EXCLUDED_DIRS and not name.startswith(".")
            )
            for filename in sorted(filenames):
                if not filename.endswith(".t.sol"):
                    continue
                path = Path(dirpath) / filename
                text = read_text_capped(path)
                if not text:
                    continue
                for name, _body, _index in iter_test_functions(text):
                    index.setdefault(name, path.relative_to(run_dir).as_posix())
    return index


# --- execution --------------------------------------------------------------


def _forge_command(
    forge_bin: str,
    rpc_url: str,
    tests: Sequence[str],
    block: Optional[int],
) -> List[str]:
    pattern = "^(?:{})$".format("|".join(re.escape(name) for name in tests))
    command = [forge_bin, "test", "--fork-url", rpc_url]
    if block is not None:
        command.extend(["--fork-block-number", str(block)])
    command.extend(["--match-test", pattern, "-vvv"])
    return command


def _child_env(rpc_url: str) -> Dict[str, str]:
    env = dict(os.environ)
    env["RPC_URL"] = rpc_url
    env["FOUNDRY_DISABLE_NIGHTLY_WARNING"] = "1"
    # The replay needs no provider credentials; do not hand them to a child.
    env.pop("DEEPSEEK_API_KEY", None)
    return env


def _write_log(path: Path, header: Sequence[str], stdout: str, stderr: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as handle:
        for line in header:
            handle.write("{}\n".format(line))
        handle.write(stdout or "")
        if stderr:
            if stdout and not stdout.endswith("\n"):
                handle.write("\n")
            handle.write(stderr)


def _run_once(
    run_dir: Path,
    root: Path,
    forge_bin: str,
    rpc_url: str,
    tests: Sequence[str],
    block: Optional[int],
    timeout_s: float,
) -> Dict[str, Any]:
    command = _forge_command(forge_bin, rpc_url, tests, block)
    started = time.time()
    # The RPC URL often carries an API key in its path, so the header shows a
    # redacted copy rather than argv verbatim.
    shown = [_redact_rpc(part) if part == rpc_url else part for part in command]
    header = [
        "{} command: {}".format(LOG_HEADER, " ".join(shown)),
        "{} root: {}".format(LOG_HEADER, root.relative_to(run_dir).as_posix() or "."),
        "{} block: {}".format(LOG_HEADER, block if block is not None else "latest"),
        "{} rpc: {}".format(LOG_HEADER, _redact_rpc(rpc_url)),
        "{} started: {}".format(LOG_HEADER, utc_now_iso()),
    ]

    stdout = ""
    stderr = ""
    timed_out = False
    exit_code: Optional[int] = None
    try:
        completed = subprocess.run(
            command,
            cwd=str(root),
            env=_child_env(rpc_url),
            capture_output=True,
            text=True,
            timeout=timeout_s,
        )
        stdout = completed.stdout or ""
        stderr = completed.stderr or ""
        exit_code = completed.returncode
    except subprocess.TimeoutExpired as expired:
        timed_out = True
        stdout = _as_text(expired.stdout)
        stderr = _as_text(expired.stderr) + "\nreplay timed out after {:.0f}s".format(timeout_s)
    except OSError as error:
        stderr = "replay failed to start forge: {}".format(error)

    duration_ms = int((time.time() - started) * 1000)
    label = "block-{}".format(block if block is not None else "latest")
    log_path = REPLAY_DIR_RELATIVE / _slug(root.relative_to(run_dir).as_posix() or "root") / "{}.log".format(label)
    _write_log(run_dir / log_path, header, stdout, stderr)

    parsed = merge_forge_logs([stdout])
    return {
        "root": root.relative_to(run_dir).as_posix() or ".",
        "block": block,
        "log": log_path.as_posix(),
        "exit_code": exit_code,
        "timed_out": timed_out,
        "duration_ms": duration_ms,
        "status_by_test": parsed["tests"],
        "tests_observed": len(parsed["tests"]),
    }


def _slug(value: str) -> str:
    # A Foundry project at the run root resolves to ".", which would otherwise
    # produce `artifacts/replay/./block-N.log`.
    if value in {"", "."}:
        return "root"
    slug = re.sub(r"[^A-Za-z0-9._-]+", "-", value.replace("/", "-")).strip(".-")
    return slug or "root"


def _as_text(value: Any) -> str:
    if isinstance(value, bytes):
        return value.decode("utf-8", errors="replace")
    return value if isinstance(value, str) else ""


# --- entry point ------------------------------------------------------------


def run_replay(
    run_dir: Path,
    rpc_url: Optional[str] = None,
    block: Optional[int] = None,
    delta: Optional[int] = None,
    budget: Optional[int] = None,
    forge_bin: Optional[str] = None,
    cast_bin: Optional[str] = None,
    now: Optional[str] = None,
) -> Dict[str, Any]:
    """Replay the claimed tests and return the attestation record.

    Never raises for an expected failure: missing RPC, missing forge, no
    Foundry project, or a compile error all come back as a status with a
    reason, because the caller is the container's exit path.
    """
    run_path = Path(run_dir)
    started = time.time()
    total_budget_ms = max(1_000, budget if budget is not None else budget_ms())
    remaining_ms = total_budget_ms
    errors: List[str] = []

    payload: Dict[str, Any] = {
        "schema_version": REPLAY_SCHEMA_VERSION,
        "status": "skipped",
        "reason": None,
        "created_at": now or utc_now_iso(),
        "rpc": {"configured": False, "host": None},
        "block": None,
        "block_source": None,
        "delta": delta if delta is not None else default_delta(),
        "delta_block": None,
        "budget_ms": total_budget_ms,
        "roots": [],
        "tests": {},
        "not_replayed": [],
        "errors": errors,
        "duration_ms": 0,
    }

    resolved_rpc = rpc_url or os.environ.get("RPC_URL", "")
    if not resolved_rpc.strip():
        payload["reason"] = "RPC_URL is not configured"
        return payload
    payload["rpc"] = {"configured": True, "host": _redact_rpc(resolved_rpc)}

    binary = forge_bin or shutil.which("forge")
    if not binary:
        payload["reason"] = "forge is not installed"
        return payload

    report, report_error = read_json(run_path / "reports" / "final_report.json")
    if report_error or not isinstance(report, dict):
        payload["reason"] = "final_report.json is missing or unreadable"
        return payload

    claimed = linked_tests(report)
    if not claimed:
        payload["reason"] = "no finding names a test to replay"
        return payload

    roots = discover_foundry_roots(run_path)
    if not roots:
        payload["reason"] = "no foundry.toml found in the run directory"
        return payload

    by_root = harness_tests_by_root(run_path, roots)
    planned: List[Tuple[Path, List[str]]] = []
    for root in roots:
        available = set(by_root.get(str(root), []))
        selected = [name for name in claimed if name in available]
        if selected:
            planned.append((root, selected))
    if not planned:
        payload["reason"] = "claimed tests are not declared under any foundry project"
        return payload

    if block is not None:
        payload["block"], payload["block_source"] = block, "override"
    else:
        payload["block"], payload["block_source"] = resolve_block(cast_bin, resolved_rpc, run_path)
    payload["delta_block"] = (
        payload["block"] - payload["delta"]
        if payload["block"] is not None and payload["delta"]
        else None
    )

    per_run_ms = max(30_000, total_budget_ms // max(1, len(planned) * 2))
    index = _test_file_index(run_path, roots)
    seen_tests: List[str] = []
    ok_runs = 0

    for root, selected in planned:
        root_entry: Dict[str, Any] = {
            "path": root.relative_to(run_path).as_posix() or ".",
            "tests": selected,
            "runs": [],
            "error": None,
        }
        payload["roots"].append(root_entry)

        primary_ms = min(per_run_ms, remaining_ms)
        if primary_ms < 1_000:
            root_entry["error"] = "replay budget exhausted"
            errors.append("{}: replay budget exhausted".format(root_entry["path"]))
            continue

        primary = _run_once(
            run_path, root, binary, resolved_rpc, selected, payload["block"], primary_ms / 1000.0
        )
        root_entry["runs"].append(primary)
        remaining_ms -= primary["duration_ms"]

        delta_run: Optional[Dict[str, Any]] = None
        if (
            primary["status_by_test"]
            and payload["delta_block"] is not None
            and remaining_ms >= MIN_DELTA_BUDGET_MS
        ):
            delta_ms = min(per_run_ms, remaining_ms)
            delta_run = _run_once(
                run_path,
                root,
                binary,
                resolved_rpc,
                selected,
                payload["delta_block"],
                delta_ms / 1000.0,
            )
            root_entry["runs"].append(delta_run)
            remaining_ms -= delta_run["duration_ms"]

        if primary["timed_out"]:
            root_entry["error"] = "replay timed out"
            errors.append("{}: replay timed out".format(root_entry["path"]))
            continue
        # `forge test` exits non-zero when a test fails, which is a valid
        # replay result. A non-zero exit with no parsed tests is a failure.
        if not primary["status_by_test"] and primary["exit_code"] not in (0, None):
            root_entry["error"] = "forge exited {} without test results".format(primary["exit_code"])
            errors.append(
                "{}: forge exited {} without test results".format(
                    root_entry["path"], primary["exit_code"]
                )
            )
        if primary["status_by_test"]:
            ok_runs += 1

        for name in selected:
            status = primary["status_by_test"].get(name)
            delta_status = (delta_run or {}).get("status_by_test", {}).get(name)
            if status is None:
                continue
            seen_tests.append(name)
            # `None` distinguishes "the second run never happened" from
            # "the test passed at both blocks".
            if delta_status is None:
                state_sensitive: Optional[bool] = None
            else:
                state_sensitive = status == "PASS" and delta_status == "FAIL"
            payload["tests"][name] = {
                "root": root_entry["path"],
                "file": index.get(name),
                "sha256": sha256_file(run_path / index[name]) if index.get(name) else None,
                "status": status,
                "block": primary["block"],
                "log": primary["log"],
                "delta_status": delta_status,
                "delta_block": (delta_run or {}).get("block"),
                "state_sensitive": state_sensitive,
            }

    payload["not_replayed"] = sorted(set(claimed) - set(seen_tests))
    payload["duration_ms"] = int((time.time() - started) * 1000)
    if not payload["tests"]:
        payload["status"] = "error"
        payload["reason"] = "replay produced no test results"
        if not errors:
            errors.append(payload["reason"])
    else:
        payload["status"] = "ok"
        payload["reason"] = None
        if ok_runs == 0:
            payload["status"] = "error"
            payload["reason"] = "every replay run failed"
    return payload


def replay_index(payload: Optional[Dict[str, Any]]) -> Dict[str, Dict[str, Any]]:
    """Return the per-test map of a replay payload, if it is usable."""
    if not isinstance(payload, dict):
        return {}
    if payload.get("status") not in {"ok", "error"}:
        return {}
    tests = payload.get("tests")
    if not isinstance(tests, dict):
        return {}
    return {name: row for name, row in tests.items() if isinstance(row, dict)}


def load_replay(run_dir: Path) -> Optional[Dict[str, Any]]:
    payload, error = read_json(Path(run_dir) / REPLAY_JSON_RELATIVE)
    if error:
        return None
    return payload if isinstance(payload, dict) else None


def write_replay(run_dir: Path, payload: Dict[str, Any]) -> Path:
    path = Path(run_dir) / REPLAY_JSON_RELATIVE
    atomic_write_json(path, payload)
    return path
