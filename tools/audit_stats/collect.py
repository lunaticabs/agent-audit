from __future__ import annotations

import hashlib
import json
import os
import re
import time
from pathlib import Path
from typing import Any, Dict, List, Optional, Sequence, Tuple

from . import STATS_SCHEMA_VERSION, __version__
from .common import (
    CONFIDENCE_ORDER,
    HIGH_PLUS,
    MAX_EVIDENCE_SAMPLES,
    MAX_TEXT_BYTES,
    REPORT_REQUIRED_FIELDS,
    REPORT_STATUSES,
    SEVERITY_ORDER,
    as_list,
    as_text,
    atomic_write_json,
    coerce_bool,
    dir_size,
    normalize_confidence,
    normalize_severity,
    read_json,
    read_text_capped,
    resolve_evidence,
    utc_now_iso,
)
from .forge import (
    TEST_NAME_RE,
    fork_log_signals,
    fork_signal_sites,
    iter_test_functions,
    merge_forge_logs,
    parse_forge_log,
)
from .replay import load_replay, read_pinned_block, replay_index

#: Directories that belong to dependency/tooling trees, not to the audit
#: harness. `lib/forge-std` alone ships dozens of `.t.sol` files.
HARNESS_EXCLUDE_DIRS = {
    "lib",
    "out",
    "cache",
    "build",
    "broadcast",
    "node_modules",
    "vendor",
}

SLITHER_RESULT_RE = re.compile(r"(\d+)\s+result\(s\)\s+found")
SLITHER_IMPACT_RE = re.compile(r"Number of\s+(\w+)\s+issues:\s*(\d+)")
ADDRESS_RE = re.compile(r"0x[a-fA-F0-9]{40}")
# `├─ [1234] 0xC02a...::withdraw(1 ether)` or `WETH9::withdraw(...)`
TRACE_CALL_RE = re.compile(
    r"\b(0x[a-fA-F0-9]{40}|[A-Za-z_][A-Za-z0-9_]*)::([A-Za-z_][A-Za-z0-9_]*)"
    r"\s*(?:\{[^{}\n]*\})?\s*\("
)
HARNESS_CALL_RE = re.compile(
    r"\b([A-Za-z_][A-Za-z0-9_]*)\.([A-Za-z_][A-Za-z0-9_]*)"
    r"\s*(?:\{[^{}\n]*\})?\s*\("
)


# --- harness and forge ------------------------------------------------------


def _iter_harness_files(run_dir: Path) -> List[Path]:
    files: List[Path] = []
    for root, dirnames, filenames in os.walk(str(run_dir)):
        dirnames[:] = sorted(
            name
            for name in dirnames
            if name not in HARNESS_EXCLUDE_DIRS and not name.startswith(".")
        )
        for name in sorted(filenames):
            if name.endswith(".t.sol"):
                files.append(Path(root) / name)
    return files


def audit_forge(run_dir: Path, harness_tests: Sequence[str]) -> Dict[str, Any]:
    artifacts = run_dir / "artifacts"
    logs: List[Path] = []
    if artifacts.is_dir():
        for root, dirnames, filenames in os.walk(str(artifacts)):
            dirnames[:] = sorted(dirnames)
            for name in sorted(filenames):
                if name.endswith(".log") or name.endswith(".txt"):
                    logs.append(Path(root) / name)

    parsed_logs: List[str] = []
    parsed_texts: List[str] = []
    fork_signals: Dict[str, List[str]] = {}

    for path in logs:
        text = read_text_capped(path, MAX_TEXT_BYTES)
        if not text:
            continue
        if "Suite result:" not in text and "[PASS]" not in text and "[FAIL" not in text:
            continue
        parsed = parse_forge_log(text)
        if not parsed["tests"] and not parsed["suites"]:
            continue
        relative = path.relative_to(run_dir).as_posix()
        parsed_logs.append(relative)
        parsed_texts.append(text)
        signals = fork_log_signals(text)
        if signals:
            fork_signals[relative] = signals

    merged_all = merge_forge_logs(parsed_texts)
    merged: Dict[str, str] = merged_all["tests"]
    fuzz_runs = merged_all["fuzz_runs"]
    suites = merged_all["suites"]

    passed = sum(1 for value in merged.values() if value == "PASS")
    failed = sum(1 for value in merged.values() if value == "FAIL")
    skipped = sum(1 for value in merged.values() if value == "SKIP")

    return {
        "logs": parsed_logs,
        "tests_observed": len(merged),
        "passed": passed,
        "failed": failed,
        "skipped": skipped,
        "suite_ok": all(suite["status"] in {"ok", "OK"} for suite in suites) if suites else None,
        "fuzz_runs": max(fuzz_runs.values()) if fuzz_runs else 0,
        "fork_log_signals": fork_signals,
        "tests_in_harness_not_in_log": sorted(set(harness_tests) - set(merged)),
        "tests_in_log_not_in_harness": sorted(set(merged) - set(harness_tests)),
        "status_by_test": merged,
    }


def _pinned_block(run_dir: Path) -> Optional[int]:
    """The fork block the agent claims, shared with the replay module."""
    return read_pinned_block(run_dir)


def audit_harness(run_dir: Path) -> Dict[str, Any]:
    """Scan `.t.sol` files for tests and for fork markers inside function bodies.

    Only a marker inside a function body (`testXxx`, `setUp`, or a helper) makes
    a run fork-backed. The previous implementation matched the marker anywhere
    in the file, so a comment or a string was enough. Rejected sites are still
    reported so the aggregate can show how often that happened.
    """
    harness_files = _iter_harness_files(run_dir)
    harness_bytes = 0
    tests: Dict[str, str] = {}
    marker_sites: List[Dict[str, str]] = []
    outside_functions: set = set()
    comment_only: set = set()

    for path in harness_files:
        text = read_text_capped(path, MAX_TEXT_BYTES)
        if not text:
            continue
        relative = path.relative_to(run_dir).as_posix()
        try:
            harness_bytes += path.stat().st_size
        except OSError:
            pass
        for name, _body, _index in iter_test_functions(text):
            tests[name] = relative

        signals = fork_signal_sites(text)
        for item in signals["in_functions"]:
            marker_sites.append(
                {"marker": item["marker"], "function": item["function"], "file": relative}
            )
        outside_functions.update(signals["outside_functions"])
        comment_only.update(signals["comment_only"])

    markers = sorted({item["marker"] for item in marker_sites})
    pinned_block_number = _pinned_block(run_dir)
    pinned_block_file = (run_dir / "artifacts" / "pinned_block.json").exists()
    if pinned_block_file:
        markers.append("artifacts/pinned_block.json")

    fuzz_tests = [name for name in tests if "fuzz" in name.lower()]

    return {
        "files": [path.relative_to(run_dir).as_posix() for path in harness_files],
        "bytes": harness_bytes,
        "tests": sorted(tests),
        "tests_fuzz": sorted(fuzz_tests),
        "test_files": dict(sorted(tests.items())),
        "fork_backed": bool(markers),
        "fork_signals": markers,
        "fork_marker_sites": marker_sites,
        "fork_markers_outside_functions": sorted(outside_functions),
        "fork_markers_comment_only": sorted(comment_only),
        "pinned_block": pinned_block_number,
    }


# --- report and findings ----------------------------------------------------


def validate_report(obj: Any) -> Dict[str, Any]:
    result: Dict[str, Any] = {
        "grade": "invalid",
        "missing": [],
        "type_errors": [],
        "status_valid": False,
    }
    if not isinstance(obj, dict):
        return result

    for field in REPORT_REQUIRED_FIELDS:
        if field not in obj:
            result["missing"].append(field)

    expected_types = {
        "run_id": str,
        "target": dict,
        "status": str,
        "summary": str,
        "findings": list,
        "hypotheses": list,
        "limitations": list,
        "coverage": dict,
    }
    for field, expected in expected_types.items():
        if field in obj and not isinstance(obj[field], expected):
            result["type_errors"].append(
                "{} should be {}".format(field, expected.__name__)
            )

    status = obj.get("status")
    if isinstance(status, str) and status in REPORT_STATUSES:
        result["status_valid"] = True

    if not result["missing"] and not result["type_errors"]:
        result["grade"] = "current"
        return result

    legacy = (
        isinstance(obj.get("findings"), list)
        and (
            "status" in result["missing"]
            or "summary" in result["type_errors"]
            or "coverage" in result["missing"]
        )
    )
    result["grade"] = "legacy" if legacy else "partial"
    return result


def _extract_findings(report: Dict[str, Any]) -> Tuple[List[Dict[str, Any]], Optional[str], int]:
    key = None
    raw: List[Any] = []
    for candidate in ("findings", "confirmed_findings"):
        value = report.get(candidate)
        if isinstance(value, list):
            key = candidate
            raw = value
            break

    findings: List[Dict[str, Any]] = []
    malformed = 0
    for item in raw:
        if isinstance(item, dict):
            findings.append(item)
        else:
            malformed += 1
    return findings, key, malformed


def _audit_evidence_items(run_dir: Path, items: Sequence[Dict[str, Any]]) -> Dict[str, Any]:
    total = 0
    resolved = 0
    missing = 0
    outside = 0
    invalid = 0
    missing_samples: List[str] = []
    outside_samples: List[str] = []

    for item in items:
        for ref in as_list(item.get("evidence")):
            total += 1
            status, _ = resolve_evidence(run_dir, ref)
            if status == "ok":
                resolved += 1
            elif status == "missing":
                missing += 1
                if len(missing_samples) < MAX_EVIDENCE_SAMPLES:
                    missing_samples.append(as_text(ref)[:200])
            elif status in {"escape", "absolute"}:
                outside += 1
                if len(outside_samples) < MAX_EVIDENCE_SAMPLES:
                    outside_samples.append(as_text(ref)[:200])
            else:
                invalid += 1

    return {
        "refs_total": total,
        "refs_resolved": resolved,
        "refs_missing": missing,
        "refs_outside_run": outside,
        "refs_invalid": invalid,
        "missing_samples": missing_samples,
        "outside_samples": outside_samples,
    }


def _replay_row(replay_tests: Dict[str, Dict[str, Any]], name: str) -> Optional[Dict[str, Any]]:
    row = replay_tests.get(name)
    if not isinstance(row, dict):
        return None
    if row.get("status") not in {"PASS", "FAIL", "SKIP"}:
        return None
    return row


def _collect_findings(
    run_dir: Path,
    raw_findings: Sequence[Dict[str, Any]],
    harness_tests: Sequence[str],
    forge_status: Dict[str, str],
    replay_tests: Optional[Dict[str, Dict[str, Any]]] = None,
) -> Tuple[List[Dict[str, Any]], Dict[str, Any]]:
    harness_set = set(harness_tests)
    replay_tests = replay_tests or {}
    rows: List[Dict[str, Any]] = []

    severity_counts = {name: 0 for name in SEVERITY_ORDER}
    confidence_counts = {name: 0 for name in CONFIDENCE_ORDER}
    seen_ids: Dict[str, int] = {}
    evidence_totals = {"total": 0, "resolved": 0, "missing": 0, "outside": 0, "invalid": 0}
    missing_samples: List[str] = []
    outside_samples: List[str] = []
    anomalies: List[Dict[str, str]] = []

    for index, finding in enumerate(raw_findings):
        finding_id = as_text(finding.get("id")).strip() or "finding-{}".format(index + 1)
        seen_ids[finding_id] = seen_ids.get(finding_id, 0) + 1

        severity, severity_ok = normalize_severity(finding.get("severity"))
        confidence, confidence_ok = normalize_confidence(finding.get("confidence"))
        severity_counts[severity] += 1
        confidence_counts[confidence] += 1

        evidence_total = 0
        evidence_resolved = 0
        evidence_missing = 0
        for ref in as_list(finding.get("evidence")):
            evidence_total += 1
            evidence_totals["total"] += 1
            status, _ = resolve_evidence(run_dir, ref)
            if status == "ok":
                evidence_resolved += 1
                evidence_totals["resolved"] += 1
            elif status == "missing":
                evidence_missing += 1
                evidence_totals["missing"] += 1
                if len(missing_samples) < MAX_EVIDENCE_SAMPLES:
                    missing_samples.append("{}: {}".format(finding_id, as_text(ref)[:200]))
            elif status in {"escape", "absolute"}:
                evidence_totals["outside"] += 1
                if len(outside_samples) < MAX_EVIDENCE_SAMPLES:
                    outside_samples.append("{}: {}".format(finding_id, as_text(ref)[:200]))
            else:
                evidence_totals["invalid"] += 1

        reproduction = finding.get("reproduction", finding.get("repro", ""))
        reproduction_text = " ".join(as_text(item) for item in as_list(reproduction)).strip()
        repro_tests = sorted(set(TEST_NAME_RE.findall(reproduction_text)))
        repro_linked = [name for name in repro_tests if name in harness_set]
        repro_missing = [name for name in repro_tests if name not in harness_set]

        # A replay row wins over the agent's own log: it was produced after the
        # agent exited, with the command and block recorded next to the output.
        repro_passed: List[str] = []
        replays: List[Dict[str, Any]] = []
        for name in repro_linked:
            row = _replay_row(replay_tests, name)
            if row is not None:
                replays.append(
                    {
                        "test": name,
                        "status": row.get("status"),
                        "delta_status": row.get("delta_status"),
                        "state_sensitive": row.get("state_sensitive"),
                        "sha256": row.get("sha256"),
                    }
                )
                if row.get("status") == "PASS":
                    repro_passed.append(name)
            elif forge_status.get(name) == "PASS":
                repro_passed.append(name)

        attested = [name for name in repro_passed if _replay_row(replay_tests, name) is not None]
        repro_attested = bool(repro_passed) and len(attested) == len(repro_passed)
        repro_source = None
        if repro_passed:
            repro_source = "replay" if repro_attested else "agent_log"
        repro_state_sensitive = any(item.get("state_sensitive") is True for item in replays)

        if not severity_ok:
            anomalies.append(
                {"code": "finding_severity_unknown", "detail": "{}: {!r}".format(finding_id, finding.get("severity"))}
            )
        if not confidence_ok:
            anomalies.append(
                {
                    "code": "finding_confidence_unknown",
                    "detail": "{}: {!r}".format(finding_id, finding.get("confidence")),
                }
            )
        if evidence_total == 0:
            anomalies.append({"code": "finding_without_evidence", "detail": finding_id})

        rows.append(
            {
                "id": finding_id,
                "title": as_text(finding.get("title"))[:200],
                "severity": severity,
                "confidence": confidence,
                "severity_ok": severity_ok,
                "confidence_ok": confidence_ok,
                "contract": as_text(finding.get("contract")),
                "function": as_text(finding.get("function"))[:200],
                "evidence_total": evidence_total,
                "evidence_resolved": evidence_resolved,
                "evidence_missing": evidence_missing,
                "has_reproduction": bool(reproduction_text),
                "repro_tests": repro_tests,
                "repro_linked": repro_linked,
                "repro_passed": repro_passed,
                "repro_test_missing": repro_missing,
                "repro_source": repro_source,
                "repro_attested": repro_attested,
                "repro_state_sensitive": repro_state_sensitive,
                "repro_replays": replays,
            }
        )

    duplicate_ids = sorted(name for name, count in seen_ids.items() if count > 1)
    if duplicate_ids:
        anomalies.append({"code": "finding_id_duplicate", "detail": ", ".join(duplicate_ids)})

    max_severity = "unknown"
    for name in SEVERITY_ORDER:
        if severity_counts[name]:
            max_severity = name
            break

    summary = {
        "severity": severity_counts,
        "confidence": confidence_counts,
        "max_severity": max_severity,
        "high_plus": sum(severity_counts[name] for name in HIGH_PLUS),
        "critical": severity_counts["critical"],
        "evidence": {
            "refs_total": evidence_totals["total"],
            "refs_resolved": evidence_totals["resolved"],
            "refs_missing": evidence_totals["missing"],
            "refs_outside_run": evidence_totals["outside"],
            "refs_invalid": evidence_totals["invalid"],
            "missing_samples": missing_samples,
            "outside_samples": outside_samples,
        },
        "anomalies": anomalies,
    }
    return rows, summary


# --- tooling ----------------------------------------------------------------


def _tool_files(run_dir: Path, subdir: str) -> List[Path]:
    base = run_dir / subdir
    if not base.is_dir():
        return []
    files: List[Path] = []
    for root, dirnames, filenames in os.walk(str(base)):
        dirnames[:] = sorted(dirnames)
        for name in sorted(filenames):
            files.append(Path(root) / name)
    return files


def audit_tooling(run_dir: Path, forge_logs: Sequence[str] = ()) -> Dict[str, Any]:
    artifact_files = _tool_files(run_dir, "artifacts")
    rel_names = [path.relative_to(run_dir).as_posix() for path in artifact_files]

    # Slither: JSON result plus the human/stdout text that the agent often
    # cites instead. The two can disagree; record both and flag it.
    slither_json = [path for path in artifact_files if path.name.startswith("slither") and path.suffix == ".json"]
    slither_text = [
        path
        for path in artifact_files
        if path.name.startswith("slither") and path.suffix in {".log", ".txt"}
    ]

    slither_json_results: Optional[int] = None
    slither_success: Optional[bool] = None
    for path in slither_json[:1]:
        payload, error = read_json(path)
        if error or not isinstance(payload, dict):
            continue
        slither_success = bool(payload.get("success"))
        results = payload.get("results")
        if isinstance(results, dict):
            detectors = results.get("detectors")
            if isinstance(detectors, list):
                slither_json_results = len(detectors)
            elif isinstance(results.get("results"), list):
                slither_json_results = len(results["results"])
        elif isinstance(results, list):
            slither_json_results = len(results)

    slither_stdout_results: Optional[int] = None
    impact_counts: Dict[str, int] = {}
    for path in slither_text:
        text = read_text_capped(path, MAX_TEXT_BYTES) or ""
        match = SLITHER_RESULT_RE.search(text)
        if match:
            found = int(match.group(1))
            slither_stdout_results = max(slither_stdout_results or 0, found)
        for impact, count in SLITHER_IMPACT_RE.findall(text):
            impact_counts[impact] = max(impact_counts.get(impact, 0), int(count))

    slither_consistent: Optional[bool] = None
    if slither_json_results is not None and slither_stdout_results is not None:
        slither_consistent = slither_json_results == slither_stdout_results

    heimdall_files = [name for name in rel_names if name.startswith("artifacts/heimdall/")]
    echidna_json = [name for name in rel_names if "echidna" in name.lower()]

    bytecode_match: Optional[bool] = None
    payload, error = read_json(run_dir / "artifacts" / "bytecode_match.json")
    if not error and isinstance(payload, dict):
        bytecode_match = bool(payload.get("only_metadata_hash_differs"))

    return {
        "slither": {
            "present": bool(slither_json or slither_text),
            "json_files": [path.relative_to(run_dir).as_posix() for path in slither_json],
            "text_files": [path.relative_to(run_dir).as_posix() for path in slither_text],
            "json_results": slither_json_results,
            "stdout_results": slither_stdout_results,
            "consistent": slither_consistent,
            "success": slither_success,
            "impact_counts": impact_counts,
        },
        "forge": {
            "present": bool(forge_logs),
            "logs": list(forge_logs),
            "inferred_from": "parsed_log" if forge_logs else None,
        },
        "echidna": {
            "present": bool(echidna_json) or (run_dir / "echidna_project").is_dir(),
            "files": echidna_json,
        },
        "heimdall": {
            "present": bool(heimdall_files),
            "files": len(heimdall_files),
            "disassembly": any(name.endswith(".asm") for name in heimdall_files),
        },
        "cast": {
            "traces": sum(1 for name in rel_names if re.search(r"/trace_[^/]*\.log$", name)),
            "code_reads": sum(1 for name in rel_names if re.search(r"/code_at_[^/]*\.hex$", name)),
        },
        "source_api": {
            "getsourcecode": (run_dir / "artifacts" / "etherscan_getsourcecode.json").exists(),
            "getabi": (run_dir / "artifacts" / "etherscan_getabi.json").exists(),
        },
        "bytecode_match": bytecode_match,
    }


# --- coverage claims --------------------------------------------------------


def coverage_claims(
    report: Dict[str, Any],
    tooling: Dict[str, Any],
    fork: Dict[str, Any],
    volume: Dict[str, Any],
) -> Dict[str, Any]:
    declared_raw = report.get("coverage") if isinstance(report.get("coverage"), dict) else {}
    declared: Dict[str, Optional[bool]] = {}
    invalid: List[str] = []
    for key in ("source_review", "static_analysis", "fork_testing", "fuzzing"):
        value, valid = coerce_bool(declared_raw.get(key))
        declared[key] = value
        if not valid:
            invalid.append(key)

    observed = {
        "source_review": volume["sources_files"] > 0 or tooling["source_api"]["getsourcecode"],
        "static_analysis": tooling["slither"]["present"] or tooling["heimdall"]["present"],
        "fork_testing": bool(fork["files"]) and (fork["fork_backed"] or fork["forge"]["tests_observed"] > 0),
        "fuzzing": bool(fork["tests_fuzz"]) or fork["forge"]["fuzz_runs"] > 0 or tooling["echidna"]["present"],
    }

    overclaims = [key for key in observed if declared.get(key) is True and not observed[key]]
    underclaims = [key for key in observed if declared.get(key) is False and observed[key]]

    return {
        "declared": declared,
        "observed": observed,
        "invalid_values": invalid,
        "overclaims": overclaims,
        "underclaims": underclaims,
    }


# --- function coverage ------------------------------------------------------


_ABI_ITEM_TYPES = {"function", "event", "error", "constructor", "fallback", "receive"}


def _looks_like_abi_entry(entry: Any) -> bool:
    if not isinstance(entry, dict):
        return False
    kind = entry.get("type")
    if isinstance(kind, str) and kind in _ABI_ITEM_TYPES:
        return True
    # Older ABI JSON omits `type` but still has these fields.
    if isinstance(entry.get("inputs"), list) and any(
        key in entry for key in ("outputs", "constant", "payable", "stateMutability")
    ):
        return True
    return False


def _abi_entries(payload: Any, depth: int = 0) -> List[Dict[str, Any]]:
    """Find an ABI list in common Etherscan/Blockscout/Sourcify shapes.

    Agents save ABIs in different wrappers: `{"abi": [...]}`, Etherscan's
    `{"result": [{"ABI": "..."}]}`, or Sourcify's `{"output": {"abi": [...]}}`.
    A bare JSON list is only treated as an ABI when its entries look like ABI
    items: GitHub tag lists and other metadata also happen to be lists of
    objects with a `name`.
    """
    if depth > 3:
        return []

    if isinstance(payload, list):
        entries = [entry for entry in payload if isinstance(entry, dict)]
        valid = [entry for entry in entries if _looks_like_abi_entry(entry)]
        if valid:
            return valid
        for item in entries:
            nested = _abi_entries(item, depth + 1)
            if nested:
                return nested
        return []

    if isinstance(payload, dict):
        for key in ("abi", "ABI"):
            value = payload.get(key)
            if isinstance(value, list):
                entries = [entry for entry in value if _looks_like_abi_entry(entry)]
                if entries:
                    return entries
            elif isinstance(value, str) and value.strip():
                try:
                    decoded = json.loads(value)
                except json.JSONDecodeError:
                    continue
                if isinstance(decoded, list):
                    entries = [entry for entry in decoded if _looks_like_abi_entry(entry)]
                    if entries:
                        return entries
        for key in ("result", "output", "metadata", "contract"):
            if key in payload:
                nested = _abi_entries(payload[key], depth + 1)
                if nested:
                    return nested
    return []


def _load_target_abi_candidates(run_dir: Path) -> List[Dict[str, Any]]:
    """Build ABI candidates from top-level JSON in `sources/` and `artifacts/`."""
    candidates: List[Dict[str, Any]] = []
    seen: set = set()
    for base in (run_dir / "sources", run_dir / "artifacts"):
        if not base.is_dir():
            continue
        for path in sorted(base.glob("*.json")):
            key = str(path)
            if key in seen:
                continue
            seen.add(key)
            try:
                if path.stat().st_size > 5 * 1024 * 1024:
                    continue
            except OSError:
                continue
            payload, error = read_json(path)
            if error:
                continue
            entries = _abi_entries(payload)
            if not entries:
                continue

            functions: Dict[str, set] = {}
            mutability: Dict[str, set] = {}
            for entry in entries:
                if entry.get("type") not in (None, "function"):
                    continue
                name = entry.get("name")
                if not isinstance(name, str) or not name:
                    continue
                inputs = ",".join(
                    str(item.get("type", ""))
                    for item in entry.get("inputs", [])
                    if isinstance(item, dict)
                )
                mode = entry.get("stateMutability")
                if not isinstance(mode, str):
                    mode = "view" if entry.get("constant") else "nonpayable"
                functions.setdefault(name, set()).add(inputs)
                mutability.setdefault(name, set()).add(mode)
            if not functions:
                continue

            state_changing = {
                name
                for name, modes in mutability.items()
                if any(mode in {"nonpayable", "payable"} for mode in modes)
            }
            candidates.append(
                {
                    "source": path.relative_to(run_dir).as_posix(),
                    "functions": functions,
                    "state_changing": state_changing,
                    "overloaded": sorted(
                        name for name, signatures in functions.items() if len(signatures) > 1
                    ),
                }
            )
    return candidates


def _trace_calls(run_dir: Path, address: str, contract_name: str) -> Dict[str, Any]:
    """Collect target calls from saved `forge -vvvv` / `cast run` logs."""
    artifacts = run_dir / "artifacts"
    if not artifacts.is_dir():
        return {"functions": set(), "calls": 0, "files": [], "fallback_calls": 0}

    candidates: List[Path] = []
    for root, dirnames, filenames in os.walk(str(artifacts)):
        dirnames[:] = sorted(dirnames)
        for name in sorted(filenames):
            if name.endswith((".log", ".txt")):
                candidates.append(Path(root) / name)

    names: set = set()
    calls = 0
    fallback_calls = 0
    files: List[str] = []
    address_lower = address.lower()
    named_target = bool(contract_name) and len(contract_name) >= 3

    for path in candidates[:50]:
        text = read_text_capped(path, MAX_TEXT_BYTES)
        if not text or "::" not in text:
            continue
        matched = False
        for match in TRACE_CALL_RE.finditer(text):
            who, function = match.group(1), match.group(2)
            is_target = bool(address_lower) and who.lower() == address_lower
            if not is_target and named_target and who == contract_name:
                is_target = True
            if not is_target:
                continue
            calls += 1
            matched = True
            if function in {"receive", "fallback"}:
                fallback_calls += 1
            else:
                names.add(function)
        if matched:
            files.append(path.relative_to(run_dir).as_posix())

    return {
        "functions": names,
        "calls": calls,
        "files": files,
        "fallback_calls": fallback_calls,
    }


def _harness_calls(
    run_dir: Path, harness_files: Sequence[str]
) -> Tuple[set, bool, int]:
    """Collect every method name referenced by the harness.

    The caller intersects this with the chosen ABI. Keeping the raw set lets
    ABI selection score candidates against what the harness actually calls,
    including when a wrong candidate would otherwise win on function count.
    """
    names: set = set()
    fallback = False
    read_files = 0
    for relative in harness_files:
        text = read_text_capped(run_dir / relative, MAX_TEXT_BYTES)
        if not text:
            continue
        read_files += 1
        if re.search(r"\.call\s*\{\s*value", text):
            fallback = True
        for match in HARNESS_CALL_RE.finditer(text):
            names.add(match.group(2))
    return names, fallback, read_files


def audit_function_coverage(
    run_dir: Path,
    harness_files: Sequence[str],
    target_address: str,
    target_name: str,
) -> Dict[str, Any]:
    """Coverage of the target's ABI surface by saved traces and the harness.

    `coverage` is the union of trace-observed and harness-referenced functions.
    It is an upper bound when a referenced call sits in a branch that never ran.
    `trace_coverage` is the conservative lower bound, but it only sees traces the
    agent actually saved.

    ABI selection is scored rather than "largest wins": the agent often saves
    several ABIs (target, owner, vault, upstream tags), and the largest one may
    belong to a different contract. Candidates are ranked by overlap with
    observed calls, then by an explicit `sources/abi.json`, target address/name
    hints, and finally size.
    """
    candidates = _load_target_abi_candidates(run_dir)
    trace = _trace_calls(run_dir, target_address, target_name)
    harness_names, harness_fallback, harness_files_read = _harness_calls(
        run_dir, harness_files
    )
    observed = set(trace["functions"]) | harness_names

    target_address_lower = target_address.strip().lower()
    target_slug = re.sub(r"[^a-z0-9]", "", target_name.lower())

    def candidate_score(candidate: Dict[str, Any]) -> Tuple[int, int, int, int, int]:
        functions = set(candidate["functions"])
        overlap = len(functions & observed)
        source = candidate["source"].lower()
        abi_hint = 1 if source.endswith("/abi.json") else 0
        address_hint = 1 if target_address_lower and target_address_lower in source else 0
        source_slug = re.sub(r"[^a-z0-9]", "", source)
        name_hint = 1 if target_slug and target_slug in source_slug else 0
        size = len(functions)
        # When nothing overlaps, prefer the smaller plausible ABI so an
        # unrelated 100-function artifact cannot dominate the denominator.
        return (overlap, abi_hint, address_hint, name_hint, size if overlap else -size)

    best: Optional[Dict[str, Any]] = None
    if candidates:
        best = max(candidates, key=candidate_score)

    if best is None:
        functions: Dict[str, set] = {}
        abi_source: Optional[str] = None
        state_changing: set = set()
        overloaded: List[str] = []
    else:
        functions = best["functions"]
        abi_source = best["source"]
        state_changing = best["state_changing"]
        overloaded = best["overloaded"]

    abi_names: set = set(functions)
    trace_names = trace["functions"] & abi_names
    harness_funcs = harness_names & abi_names
    covered = trace_names | harness_funcs

    basis = "none"
    if trace["files"] and covered:
        basis = "trace+harness" if harness_funcs else "trace"
    elif harness_funcs:
        basis = "harness"

    def rate(numerator: int) -> Optional[float]:
        if not abi_names:
            return None
        return numerator / len(abi_names)

    return {
        "abi_available": bool(abi_names),
        "abi_source": abi_source,
        "abi_candidates": len(candidates),
        "abi_functions": len(abi_names),
        "abi_state_changing": len(state_changing),
        "overloaded": overloaded,
        "basis": basis,
        "covered": len(covered),
        "coverage": rate(len(covered)),
        "trace_covered": len(trace_names),
        "trace_coverage": rate(len(trace_names)) if trace["files"] else None,
        "state_changing_covered": len(covered & state_changing),
        "state_changing_coverage": (
            len(covered & state_changing) / len(state_changing) if state_changing else None
        ),
        "uncovered": sorted(abi_names - covered),
        "trace_only": sorted(trace_names - harness_funcs),
        "harness_only": sorted(harness_funcs - trace_names),
        "unexpected_in_trace": sorted(trace["functions"] - abi_names)[:20] if abi_names else [],
        "fallback_exercised": bool(harness_fallback or trace["fallback_calls"]),
        "fallback_calls": trace["fallback_calls"],
        "trace_files": trace["files"],
        "calls_observed": trace["calls"],
        "harness_files_read": harness_files_read,
    }


# --- main collector ---------------------------------------------------------


def _duration_seconds(meta: Dict[str, Any]) -> Tuple[Optional[float], Optional[float]]:
    declared = meta.get("duration_seconds")
    declared_value: Optional[float] = None
    if isinstance(declared, (int, float)) and not isinstance(declared, bool):
        declared_value = float(declared)

    computed: Optional[float] = None
    from datetime import datetime

    def parse(value: Any) -> Optional[datetime]:
        if not isinstance(value, str) or not value:
            return None
        try:
            return datetime.fromisoformat(value.replace("Z", "+00:00"))
        except ValueError:
            return None

    started = parse(meta.get("started_at"))
    finished = parse(meta.get("finished_at"))
    if started and finished:
        computed = (finished - started).total_seconds()
    return declared_value, computed


def _replay_summary(
    payload: Optional[Dict[str, Any]], tests: Dict[str, Dict[str, Any]]
) -> Dict[str, Any]:
    """Describe the post-exit replay for the metrics record.

    `replayed_tests` counts tests we re-ran ourselves; `state_sensitive_tests`
    counts those that passed at the pinned block and failed at the earlier one.
    """
    if not isinstance(payload, dict):
        return {
            "present": False,
            "status": None,
            "reason": "no replay record",
            "block": None,
            "block_source": None,
            "delta": None,
            "delta_block": None,
            "tests": 0,
            "replayed_tests": 0,
            "state_sensitive_tests": 0,
            "duration_ms": None,
            "errors": [],
        }

    replayed = sum(1 for row in tests.values() if row.get("status") in {"PASS", "FAIL", "SKIP"})
    state_sensitive = sum(1 for row in tests.values() if row.get("state_sensitive") is True)
    return {
        "present": True,
        "schema_version": payload.get("schema_version"),
        "status": payload.get("status"),
        "reason": payload.get("reason"),
        "block": payload.get("block"),
        "block_source": payload.get("block_source"),
        "delta": payload.get("delta"),
        "delta_block": payload.get("delta_block"),
        "tests": len(tests),
        "replayed_tests": replayed,
        "state_sensitive_tests": state_sensitive,
        "duration_ms": payload.get("duration_ms"),
        "errors": [as_text(item)[:200] for item in (payload.get("errors") or [])][:8],
    }


def collect_run(run_dir: Any, now: Optional[str] = None) -> Dict[str, Any]:
    """Derive a metrics record for one run directory."""
    started_at = time.time()
    run_path = Path(run_dir).resolve()
    run_id = run_path.name
    anomalies: List[Dict[str, str]] = []

    meta, meta_error = read_json(run_path / "input" / "run_meta.json")
    if meta_error or not isinstance(meta, dict):
        meta = {}
        anomalies.append({"code": "run_meta_missing", "detail": meta_error or "not an object"})

    request, _ = read_json(run_path / "input" / "request.json")
    if not isinstance(request, dict):
        request = {}

    report, report_error = read_json(run_path / "reports" / "final_report.json")
    report_present = report_error != "missing"
    report_parsed = isinstance(report, dict)
    report_is_object = isinstance(report, dict)

    if report_error == "missing":
        anomalies.append({"code": "report_missing", "detail": "reports/final_report.json"})
    elif report_error:
        anomalies.append({"code": "report_unreadable", "detail": report_error})
    elif not report_is_object:
        anomalies.append({"code": "report_not_object", "detail": type(report).__name__})

    schema = validate_report(report if report_is_object else None)
    if schema["grade"] == "legacy":
        anomalies.append({"code": "schema_legacy", "detail": ",".join(schema["missing"] + schema["type_errors"])})
    elif schema["grade"] == "partial":
        anomalies.append({"code": "schema_partial", "detail": ",".join(schema["missing"] + schema["type_errors"])})

    report_dict: Dict[str, Any] = report if report_is_object else {}

    declared_status = as_text(report_dict.get("status"))
    if declared_status and not schema["status_valid"]:
        anomalies.append({"code": "report_status_invalid", "detail": declared_status})

    if declared_status and report_dict.get("run_id") and report_dict.get("run_id") != run_id:
        anomalies.append(
            {"code": "run_id_mismatch", "detail": "{} != {}".format(report_dict.get("run_id"), run_id)}
        )

    target = report_dict.get("target") if isinstance(report_dict.get("target"), dict) else {}
    verified, verified_valid = coerce_bool(target.get("verified"))
    if not target:
        anomalies.append({"code": "target_missing", "detail": "reports/final_report.json target"})
    elif not verified_valid:
        anomalies.append({"code": "target_verified_invalid", "detail": repr(target.get("verified"))})

    prompt_text = read_text_capped(run_path / "input" / "prompt.txt") or ""
    prompt_addresses = {value.lower() for value in ADDRESS_RE.findall(prompt_text)}
    target_address = as_text(target.get("address")).strip()
    target_name = as_text(target.get("name")).strip()
    if prompt_addresses and target_address and target_address.lower() not in prompt_addresses:
        anomalies.append(
            {
                "code": "target_address_mismatch",
                "detail": "prompt={} report={}".format(
                    ",".join(sorted(prompt_addresses)), target_address
                ),
            }
        )

    raw_findings, findings_key, malformed_findings = _extract_findings(report_dict)
    if report_parsed and findings_key is None:
        anomalies.append({"code": "findings_not_list", "detail": "no findings list"})
    if malformed_findings:
        anomalies.append(
            {"code": "finding_not_object", "detail": "{} entr(y/ies)".format(malformed_findings)}
        )

    harness = audit_harness(run_path)
    forge = audit_forge(run_path, harness["tests"])
    replay_payload = load_replay(run_path)
    replay_tests = replay_index(replay_payload)
    fork = dict(harness)
    fork["forge"] = {key: value for key, value in forge.items() if key != "status_by_test"}
    fork["forge"]["passed_tests"] = sorted(
        name for name, value in forge["status_by_test"].items() if value == "PASS"
    )
    fork["forge"]["failed_tests"] = sorted(
        name for name, value in forge["status_by_test"].items() if value == "FAIL"
    )
    fork["replay"] = _replay_summary(replay_payload, replay_tests)
    # Count only the agent's own logs here: our replay always writes a header,
    # so a compile failure would otherwise look like runtime corroboration.
    agent_log_signals = {
        path: signals
        for path, signals in forge["fork_log_signals"].items()
        if not path.startswith("artifacts/replay/")
    }
    fork["forge"]["agent_fork_log_signals"] = agent_log_signals
    fork["fork_verified"] = bool(fork["fork_backed"]) and bool(
        fork.get("pinned_block") is not None
        or agent_log_signals
        or fork["replay"]["replayed_tests"] > 0
    )
    function_coverage = audit_function_coverage(
        run_path, harness["files"], target_address, target_name
    )
    finding_rows, finding_summary = _collect_findings(
        run_path, raw_findings, harness["tests"], forge["status_by_test"], replay_tests
    )
    anomalies.extend(finding_summary["anomalies"])

    hypotheses = [item for item in as_list(report_dict.get("hypotheses")) if isinstance(item, dict)]
    hypotheses_evidence = _audit_evidence_items(run_path, hypotheses)
    if hypotheses_evidence["refs_missing"] or hypotheses_evidence["refs_outside_run"]:
        anomalies.append(
            {
                "code": "hypothesis_evidence_invalid",
                "detail": "missing={} outside={}".format(
                    hypotheses_evidence["refs_missing"], hypotheses_evidence["refs_outside_run"]
                ),
            }
        )

    evidence = finding_summary["evidence"]
    if evidence["refs_missing"]:
        anomalies.append(
            {"code": "evidence_missing", "detail": "{} ref(s)".format(evidence["refs_missing"])}
        )
    if evidence["refs_outside_run"]:
        anomalies.append(
            {
                "code": "evidence_outside_run",
                "detail": "{} ref(s)".format(evidence["refs_outside_run"]),
            }
        )

    artifacts_files, artifacts_bytes = dir_size(run_path / "artifacts")
    sources_files, sources_bytes = dir_size(run_path / "sources")
    volume = {
        "artifacts_files": artifacts_files,
        "artifacts_bytes": artifacts_bytes,
        "sources_files": sources_files,
        "sources_bytes": sources_bytes,
        "harness_files": len(harness["files"]),
        "harness_bytes": harness["bytes"],
    }

    tooling = audit_tooling(run_path, forge_logs=forge["logs"])
    if not tooling["forge"]["present"] and fork["tests"] and (run_path / "out").is_dir():
        # A compiled Foundry project with a harness is evidence forge ran, even
        # if the agent did not keep the test output.
        tooling["forge"]["present"] = True
        tooling["forge"]["inferred_from"] = "out+harness"
    if tooling["slither"]["consistent"] is False:
        anomalies.append(
            {
                "code": "slither_result_mismatch",
                "detail": "json={} stdout={}".format(
                    tooling["slither"]["json_results"], tooling["slither"]["stdout_results"]
                ),
            }
        )

    # A PASS line for a test that exists nowhere in the harness cannot have come
    # from `forge test`. Only PASS is flagged: a FAIL for a test the agent later
    # deleted is unremarkable, a PASS for one that never existed is not.
    unknown_pass = sorted(
        name
        for name in forge["tests_in_log_not_in_harness"]
        if forge["status_by_test"].get(name) == "PASS"
    )
    if unknown_pass:
        anomalies.append(
            {
                "code": "forge_log_test_unknown",
                "detail": "{} PASS line(s) for tests not in the harness: {}".format(
                    len(unknown_pass), ", ".join(unknown_pass[:5])
                ),
            }
        )

    coverage = coverage_claims(report_dict, tooling, fork, volume)
    for key in coverage["overclaims"]:
        anomalies.append({"code": "declared_coverage_overclaim", "detail": key})
    for key in coverage["invalid_values"]:
        anomalies.append({"code": "declared_coverage_invalid_value", "detail": key})

    declared_duration, computed_duration = _duration_seconds(meta)
    duration = declared_duration if declared_duration is not None else computed_duration
    if (
        declared_duration is not None
        and computed_duration is not None
        and abs(declared_duration - computed_duration) > 5
    ):
        anomalies.append(
            {
                "code": "duration_inconsistent",
                "detail": "declared={} computed={}".format(declared_duration, computed_duration),
            }
        )

    run_status = as_text(meta.get("status")) or "unknown"
    if run_status in {"failed", "error"}:
        anomalies.append({"code": "run_failed", "detail": "exit={}".format(meta.get("exit_code"))})
    if run_status == "running":
        anomalies.append({"code": "run_not_finished", "detail": "run_meta.status=running"})

    if run_status == "completed" and (meta.get("exit_code") not in (0, None)):
        anomalies.append(
            {"code": "status_mismatch", "detail": "run_meta=completed exit={}".format(meta.get("exit_code"))}
        )

    final_answer = run_path / "reports" / "final_answer.md"
    if run_status == "completed" and (not final_answer.exists() or final_answer.stat().st_size == 0):
        anomalies.append({"code": "final_answer_missing", "detail": "reports/final_answer.md"})

    repro_linked = sum(1 for row in finding_rows if row["repro_linked"])
    repro_passed = sum(1 for row in finding_rows if row["repro_passed"])
    repro_attested = sum(
        1 for row in finding_rows if row["repro_passed"] and row["repro_attested"]
    )
    repro_state_sensitive = sum(
        1 for row in finding_rows if row["repro_state_sensitive"] and row["repro_passed"]
    )
    findings_total = len(finding_rows)
    repro_rate = (repro_passed / findings_total) if findings_total else None

    fork["findings_repro"] = {
        "linked": repro_linked,
        "passed": repro_passed,
        "rate": repro_rate,
        "attested": repro_attested,
        "attested_rate": (repro_attested / findings_total) if findings_total else None,
        "state_sensitive": repro_state_sensitive,
        "state_sensitive_of_passed": (
            repro_state_sensitive / repro_passed if repro_passed else None
        ),
        "fork_backed": bool(harness["fork_backed"]),
        "fork_rate": repro_rate if harness["fork_backed"] else None,
    }

    if replay_payload is not None:
        replay_status = as_text(replay_payload.get("status"))
        if replay_status == "error":
            anomalies.append(
                {
                    "code": "fork_replay_failed",
                    "detail": as_text(replay_payload.get("reason"))[:200] or "replay error",
                }
            )
        elif replay_status == "skipped":
            anomalies.append(
                {
                    "code": "fork_replay_skipped",
                    "detail": as_text(replay_payload.get("reason"))[:200] or "replay skipped",
                }
            )
        else:
            unverified = [
                row["id"] for row in finding_rows if row["repro_passed"] and not row["repro_attested"]
            ]
            if unverified:
                anomalies.append(
                    {
                        "code": "repro_unattested",
                        "detail": "{} finding(s) not re-run: {}".format(
                            len(unverified), ", ".join(unverified[:5])
                        ),
                    }
                )
            contradicted = [
                row["id"]
                for row in finding_rows
                if row["repro_replays"]
                and not row["repro_passed"]
                and any(item.get("status") == "FAIL" for item in row["repro_replays"])
            ]
            if contradicted:
                anomalies.append(
                    {
                        "code": "repro_contradicted_by_replay",
                        "detail": "{} finding(s): {}".format(
                            len(contradicted), ", ".join(contradicted[:5])
                        ),
                    }
                )

    if fork["fork_markers_comment_only"]:
        anomalies.append(
            {
                "code": "fork_marker_not_in_test_body",
                "detail": ",".join(fork["fork_markers_comment_only"]),
            }
        )

    return {
        "stats_schema_version": STATS_SCHEMA_VERSION,
        "collector": {
            "version": __version__,
            "collected_at": now or utc_now_iso(),
            "duration_ms": int((time.time() - started_at) * 1000),
        },
        "run_id": run_id,
        "run": {
            "status": run_status,
            "exit_code": meta.get("exit_code"),
            "task_id": meta.get("task_id"),
            "batch_id": meta.get("batch_id") or os.environ.get("BATCH_ID"),
            "model": meta.get("model"),
            "reasoning_effort": meta.get("reasoning_effort"),
            "image": meta.get("image") or os.environ.get("AGENT_AUDIT_IMAGE"),
            "started_at": meta.get("started_at"),
            "finished_at": meta.get("finished_at"),
            "duration_seconds": duration,
            "prompt_sha256": (
                hashlib.sha256(prompt_text.encode("utf-8")).hexdigest() if prompt_text else None
            ),
        },
        "target": {
            "chain": as_text(target.get("chain")),
            "address": target_address,
            "name": as_text(target.get("name")),
            "verified": verified,
            "prompt_addresses": sorted(prompt_addresses),
        },
        "report": {
            "present": report_present,
            "parsed": report_parsed,
            "grade": schema["grade"],
            "errors": schema["missing"] + schema["type_errors"],
            "declared_status": declared_status,
            "findings_key": findings_key,
            "findings_total": findings_total,
            "malformed_findings": malformed_findings,
            "by_severity": finding_summary["severity"],
            "by_confidence": finding_summary["confidence"],
            "max_severity": finding_summary["max_severity"],
            "high_plus": finding_summary["high_plus"],
            "critical": finding_summary["critical"],
            "hypotheses": len(hypotheses),
            "limitations": len(as_list(report_dict.get("limitations"))),
            "declared_coverage": coverage["declared"],
        },
        "evidence": {
            "refs_total": evidence["refs_total"],
            "refs_resolved": evidence["refs_resolved"],
            "refs_missing": evidence["refs_missing"],
            "refs_outside_run": evidence["refs_outside_run"],
            "refs_invalid": evidence["refs_invalid"],
            "missing_samples": evidence["missing_samples"],
            "outside_samples": evidence["outside_samples"],
            "findings_with_evidence": sum(1 for row in finding_rows if row["evidence_total"] > 0),
            "findings_fully_resolved": sum(
                1 for row in finding_rows if row["evidence_total"] > 0 and row["evidence_missing"] == 0
            ),
            "hypotheses_refs_total": hypotheses_evidence["refs_total"],
            "hypotheses_refs_resolved": hypotheses_evidence["refs_resolved"],
        },
        "fork": fork,
        "function_coverage": function_coverage,
        "coverage": {
            "declared": coverage["declared"],
            "observed": coverage["observed"],
            "overclaims": coverage["overclaims"],
            "underclaims": coverage["underclaims"],
            "invalid_values": coverage["invalid_values"],
        },
        "tooling": tooling,
        "volume": volume,
        "findings": finding_rows,
        "anomalies": anomalies,
    }


METRICS_RELATIVE_PATH = Path("stats") / "metrics.json"


def metrics_path(run_dir: Any) -> Path:
    return Path(run_dir) / METRICS_RELATIVE_PATH


def write_run_metrics(run_dir: Any, payload: Dict[str, Any]) -> Path:
    path = metrics_path(run_dir)
    atomic_write_json(path, payload)
    return path
