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
    strip_ansi,
    utc_now_iso,
)

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

FORK_MARKERS = (
    "createSelectFork",
    "createFork",
    "forkBlockNumber",
    'envString("RPC_URL")',
    "envString('RPC_URL')",
    "fork-url",
)

TEST_NAME_RE = re.compile(r"\b(test[A-Za-z0-9_]+)")
TEST_FUNC_RE = re.compile(r"function\s+(test[A-Za-z0-9_]*)\s*\(")
FORGE_TEST_LINE_RE = re.compile(r"^\[(PASS|FAIL|SKIP)\b[^\]]*\]\s+([A-Za-z0-9_]+)")
FORGE_SUITE_RE = re.compile(r"^Suite result:\s*(\w+)\.\s*(\d+) passed;\s*(\d+) failed;\s*(\d+) skipped")
FORGE_RUNS_RE = re.compile(r"\bruns:\s*(\d+)")
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


def _merge_test_status(current: Optional[str], incoming: str) -> str:
    rank = {"PASS": 0, "SKIP": 1, "FAIL": 2}
    if current is None:
        return incoming
    return incoming if rank.get(incoming, 0) >= rank.get(current, 0) else current


def _parse_forge_log(text: str) -> Dict[str, Any]:
    status: Dict[str, str] = {}
    fuzz_runs: Dict[str, int] = {}
    suites: List[Dict[str, Any]] = []

    for raw_line in strip_ansi(text).splitlines():
        line = raw_line.strip()
        match = FORGE_TEST_LINE_RE.match(line)
        if match:
            name = match.group(2)
            status[name] = _merge_test_status(status.get(name), match.group(1))
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


def audit_forge(run_dir: Path, harness_tests: Sequence[str]) -> Dict[str, Any]:
    artifacts = run_dir / "artifacts"
    logs: List[Path] = []
    if artifacts.is_dir():
        for root, dirnames, filenames in os.walk(str(artifacts)):
            dirnames[:] = sorted(dirnames)
            for name in sorted(filenames):
                if name.endswith(".log") or name.endswith(".txt"):
                    logs.append(Path(root) / name)

    merged: Dict[str, str] = {}
    fuzz_runs: Dict[str, int] = {}
    suites: List[Dict[str, Any]] = []
    parsed_logs: List[str] = []

    for path in logs:
        text = read_text_capped(path, MAX_TEXT_BYTES)
        if not text:
            continue
        if "Suite result:" not in text and "[PASS]" not in text and "[FAIL" not in text:
            continue
        parsed = _parse_forge_log(text)
        if not parsed["tests"] and not parsed["suites"]:
            continue
        parsed_logs.append(path.relative_to(run_dir).as_posix())
        for name, value in parsed["tests"].items():
            merged[name] = _merge_test_status(merged.get(name), value)
        for name, value in parsed["fuzz_runs"].items():
            fuzz_runs[name] = max(fuzz_runs.get(name, 0), value)
        suites.extend(parsed["suites"])

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
        "tests_in_harness_not_in_log": sorted(set(harness_tests) - set(merged)),
        "tests_in_log_not_in_harness": sorted(set(merged) - set(harness_tests)),
        "status_by_test": merged,
    }


def audit_harness(run_dir: Path) -> Dict[str, Any]:
    harness_files = _iter_harness_files(run_dir)
    harness_text: List[str] = []
    harness_bytes = 0
    tests: Dict[str, str] = {}

    for path in harness_files:
        text = read_text_capped(path, MAX_TEXT_BYTES)
        if not text:
            continue
        harness_text.append(text)
        try:
            harness_bytes += path.stat().st_size
        except OSError:
            pass
        for match in TEST_FUNC_RE.finditer(text):
            tests[match.group(1)] = path.relative_to(run_dir).as_posix()

    combined = "\n".join(harness_text)
    markers = sorted({marker for marker in FORK_MARKERS if marker in combined})

    pinned_block = (run_dir / "artifacts" / "pinned_block.json").exists()
    if pinned_block:
        markers.append("artifacts/pinned_block.json")

    fuzz_tests = [name for name in tests if "fuzz" in name.lower()]

    return {
        "files": [path.relative_to(run_dir).as_posix() for path in harness_files],
        "bytes": harness_bytes,
        "tests": sorted(tests),
        "tests_fuzz": sorted(fuzz_tests),
        "fork_backed": bool(markers),
        "fork_signals": markers,
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


def _collect_findings(
    run_dir: Path,
    raw_findings: Sequence[Dict[str, Any]],
    harness_tests: Sequence[str],
    forge_status: Dict[str, str],
) -> Tuple[List[Dict[str, Any]], Dict[str, Any]]:
    harness_set = set(harness_tests)
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
        repro_passed = [name for name in repro_linked if forge_status.get(name) == "PASS"]
        repro_missing = [name for name in repro_tests if name not in harness_set]

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


def _abi_entries(payload: Any, depth: int = 0) -> List[Dict[str, Any]]:
    """Find an ABI list in common Etherscan/Blockscout/Sourcify shapes.

    Agents save ABIs in different wrappers: `{"abi": [...]}`, Etherscan's
    `{"result": [{"ABI": "..."}]}`, or Sourcify's `{"output": {"abi": [...]}}`.
    Recursion is bounded and only follows known keys.
    """
    if depth > 3:
        return []

    if isinstance(payload, list):
        entries = [entry for entry in payload if isinstance(entry, dict)]
        if not entries:
            return []
        if any(entry.get("type") or entry.get("name") for entry in entries):
            return entries
        for item in entries:
            nested = _abi_entries(item, depth + 1)
            if nested:
                return nested
        return []

    if isinstance(payload, dict):
        for key in ("abi", "ABI"):
            value = payload.get(key)
            if isinstance(value, list):
                entries = [entry for entry in value if isinstance(entry, dict)]
                if entries:
                    return entries
            elif isinstance(value, str) and value.strip():
                try:
                    decoded = json.loads(value)
                except json.JSONDecodeError:
                    continue
                if isinstance(decoded, list):
                    entries = [entry for entry in decoded if isinstance(entry, dict)]
                    if entries:
                        return entries
        for key in ("result", "output", "metadata", "contract"):
            if key in payload:
                nested = _abi_entries(payload[key], depth + 1)
                if nested:
                    return nested
    return []


def _load_target_abi(run_dir: Path) -> Dict[str, Any]:
    """Find the target ABI in `sources/` or `artifacts/`.

    Agents do not always save `*.abi.json`; a Blockscout contract response, an
    Etherscan `getsourcecode` payload, or a Sourcify response can carry the same
    ABI. Scan top-level JSON files in both directories and keep the candidate
    with the most functions.
    """
    candidates: List[Tuple[str, List[Dict[str, Any]]]] = []
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
            if entries:
                candidates.append((path.relative_to(run_dir).as_posix(), entries))

    best: Optional[Tuple[str, Dict[str, set], Dict[str, set]]] = None
    for source, entries in candidates:
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
        if best is None or len(functions) > len(best[1]):
            best = (source, functions, mutability)

    if best is None:
        return {"source": None, "functions": {}, "state_changing": set(), "overloaded": []}

    source, functions, mutability = best
    state_changing = {
        name
        for name, modes in mutability.items()
        if any(mode in {"nonpayable", "payable"} for mode in modes)
    }
    overloaded = sorted(name for name, signatures in functions.items() if len(signatures) > 1)
    return {
        "source": source,
        "functions": functions,
        "state_changing": state_changing,
        "overloaded": overloaded,
    }


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
    run_dir: Path, harness_files: Sequence[str], abi_names: set
) -> Tuple[set, bool, int]:
    """Static fallback: methods called on any variable whose name is in the ABI."""
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
            method = match.group(2)
            if method in abi_names:
                names.add(method)
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
    """
    abi = _load_target_abi(run_dir)
    abi_names: set = set(abi["functions"])
    trace = _trace_calls(run_dir, target_address, target_name)
    harness_names, harness_fallback, harness_files_read = _harness_calls(
        run_dir, harness_files, abi_names
    )

    if abi_names:
        trace_names = trace["functions"] & abi_names
        unexpected = sorted(trace["functions"] - abi_names)
    else:
        trace_names = set(trace["functions"])
        unexpected = []

    covered = trace_names | harness_names
    basis = "none"
    if trace["files"] and covered:
        basis = "trace+harness" if harness_names else "trace"
    elif harness_names:
        basis = "harness"

    def rate(numerator: int) -> Optional[float]:
        if not abi_names:
            return None
        return numerator / len(abi_names)

    state_changing = set(abi["state_changing"])
    return {
        "abi_available": bool(abi_names),
        "abi_source": abi["source"],
        "abi_functions": len(abi_names),
        "abi_state_changing": len(state_changing),
        "overloaded": abi["overloaded"],
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
        "trace_only": sorted(trace_names - harness_names),
        "harness_only": sorted(harness_names - trace_names),
        "unexpected_in_trace": unexpected[:20],
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
    fork = dict(harness)
    fork["forge"] = {key: value for key, value in forge.items() if key != "status_by_test"}
    fork["forge"]["passed_tests"] = sorted(
        name for name, value in forge["status_by_test"].items() if value == "PASS"
    )
    fork["forge"]["failed_tests"] = sorted(
        name for name, value in forge["status_by_test"].items() if value == "FAIL"
    )
    function_coverage = audit_function_coverage(
        run_path, harness["files"], target_address, target_name
    )
    finding_rows, finding_summary = _collect_findings(
        run_path, raw_findings, harness["tests"], forge["status_by_test"]
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
    findings_total = len(finding_rows)
    repro_rate = (repro_passed / findings_total) if findings_total else None

    fork["findings_repro"] = {
        "linked": repro_linked,
        "passed": repro_passed,
        "rate": repro_rate,
        "fork_backed": bool(harness["fork_backed"]),
        "fork_rate": repro_rate if harness["fork_backed"] else None,
    }

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
