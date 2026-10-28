from __future__ import annotations

import csv
import json
from pathlib import Path
from typing import Any, Dict, List, Optional, Sequence

from . import STATS_SCHEMA_VERSION, __version__
from .collect import METRICS_RELATIVE_PATH, collect_run
from .common import (
    HIGH_PLUS,
    SEVERITY_ORDER,
    format_pct,
    format_seconds,
    iter_run_dirs,
    percentile,
    ratio,
    utc_now_iso,
)
from .stability import stability_summary

ANOMALY_TOP_N = 12
MISSING_SAMPLE = 20


# --- loading ----------------------------------------------------------------


def load_metrics(
    runs_root: Path,
    since: Optional[str] = None,
    until: Optional[str] = None,
    backfill: bool = False,
    now: Optional[str] = None,
) -> Dict[str, Any]:
    runs_root = Path(runs_root).resolve()
    metrics: List[Dict[str, Any]] = []
    missing: List[str] = []
    stale: List[str] = []
    errors: List[Dict[str, str]] = []

    for run_dir in iter_run_dirs(runs_root):
        path = run_dir / METRICS_RELATIVE_PATH
        payload: Optional[Dict[str, Any]] = None
        if path.exists():
            try:
                with path.open("r", encoding="utf-8") as handle:
                    candidate = json.load(handle)
                if isinstance(candidate, dict):
                    payload = candidate
                else:
                    errors.append({"run_id": run_dir.name, "error": "metrics is not an object"})
            except (OSError, json.JSONDecodeError) as error:
                errors.append({"run_id": run_dir.name, "error": str(error)})

        if payload is None and backfill:
            payload = collect_run(run_dir, now=now)

        if payload is None:
            missing.append(run_dir.name)
            continue

        if payload.get("stats_schema_version") != STATS_SCHEMA_VERSION:
            stale.append(run_dir.name)

        started = (payload.get("run") or {}).get("started_at")
        if since and isinstance(started, str) and started < since:
            continue
        if until and isinstance(started, str) and started > until:
            continue
        metrics.append(payload)

    metrics.sort(key=lambda item: str(item.get("run_id")))
    return {
        "runs_root": str(runs_root),
        "metrics": metrics,
        "missing": missing,
        "stale": stale,
        "errors": errors,
    }


# --- aggregation ------------------------------------------------------------


def _evaluable(metrics: Dict[str, Any]) -> bool:
    report = metrics.get("report") or {}
    run = metrics.get("run") or {}
    if run.get("status") in {"failed", "error"}:
        return False
    if not report.get("parsed"):
        return False
    if report.get("grade") not in {"current", "legacy"}:
        return False
    if report.get("declared_status") == "blocked":
        return False
    return True


def _empty_severity() -> Dict[str, int]:
    return {name: 0 for name in SEVERITY_ORDER}


def _sum_severity(items: Sequence[Dict[str, Any]]) -> Dict[str, int]:
    totals = _empty_severity()
    for item in items:
        for key, value in (item.get("report", {}).get("by_severity") or {}).items():
            if key in totals:
                totals[key] += int(value or 0)
    return totals


def _duration_stats(values: Sequence[float]) -> Dict[str, Any]:
    clean = [float(value) for value in values if isinstance(value, (int, float)) and value >= 0]
    if not clean:
        return {"n": 0, "p50": None, "p90": None, "max": None, "total_seconds": 0.0}
    return {
        "n": len(clean),
        "p50": percentile(clean, 0.5),
        "p90": percentile(clean, 0.9),
        "max": max(clean),
        "total_seconds": sum(clean),
    }


def _age(values: Sequence[Any]) -> List[float]:
    return [value for value in values if isinstance(value, (int, float))]


def _strata(metrics: Sequence[Dict[str, Any]], selector) -> List[Dict[str, Any]]:
    groups: Dict[str, List[Dict[str, Any]]] = {}
    for item in metrics:
        key = selector(item)
        groups.setdefault(key, []).append(item)

    rows: List[Dict[str, Any]] = []
    for key in sorted(groups):
        items = groups[key]
        evaluable = [item for item in items if _evaluable(item)]
        findings = sum(int(item["report"]["findings_total"]) for item in evaluable)
        high_plus = sum(int(item["report"]["high_plus"]) for item in evaluable)
        runs_high_plus = sum(1 for item in evaluable if int(item["report"]["high_plus"]) > 0)
        repro_passed = sum(
            int((item.get("fork", {}).get("findings_repro") or {}).get("passed") or 0)
            for item in evaluable
        )
        rows.append(
            {
                "key": key,
                "runs": len(items),
                "evaluable": len(evaluable),
                "findings": findings,
                "high_plus": high_plus,
                "runs_high_plus": runs_high_plus,
                "high_plus_rate": ratio(runs_high_plus, len(evaluable)),
                "repro_passed": repro_passed,
                "repro_rate": ratio(repro_passed, findings),
                "p50_duration": percentile(
                    _age([(item.get("run") or {}).get("duration_seconds") for item in items]), 0.5
                ),
            }
        )
    return rows


def function_coverage_summary(metrics: Sequence[Dict[str, Any]]) -> Dict[str, Any]:
    runs = [
        item
        for item in metrics
        if (item.get("function_coverage") or {}).get("abi_available")
    ]
    trace_runs = [
        item for item in runs if (item.get("function_coverage") or {}).get("trace_coverage") is not None
    ]

    abi_total = sum(int(item["function_coverage"].get("abi_functions") or 0) for item in runs)
    covered_total = sum(int(item["function_coverage"].get("covered") or 0) for item in runs)
    state_total = sum(int(item["function_coverage"].get("abi_state_changing") or 0) for item in runs)
    state_covered = sum(
        int(item["function_coverage"].get("state_changing_covered") or 0) for item in runs
    )
    trace_abi = sum(int(item["function_coverage"].get("abi_functions") or 0) for item in trace_runs)
    trace_covered = sum(int(item["function_coverage"].get("trace_covered") or 0) for item in trace_runs)

    per_run = [
        float(item["function_coverage"]["coverage"])
        for item in runs
        if isinstance(item["function_coverage"].get("coverage"), (int, float))
    ]

    basis: Dict[str, int] = {}
    fallback_runs = 0
    uncovered_counter: Dict[str, int] = {}
    for item in runs:
        block = item["function_coverage"]
        key = str(block.get("basis") or "none")
        basis[key] = basis.get(key, 0) + 1
        if block.get("fallback_exercised"):
            fallback_runs += 1
        for name in block.get("uncovered") or []:
            uncovered_counter[name] = uncovered_counter.get(name, 0) + 1

    return {
        "runs_with_abi": len(runs),
        "runs_with_trace": len(trace_runs),
        "runs_with_fallback": fallback_runs,
        "basis": dict(sorted(basis.items())),
        "abi_functions_total": abi_total,
        "covered_total": covered_total,
        "coverage": ratio(covered_total, abi_total),
        "mean_run_coverage": (sum(per_run) / len(per_run)) if per_run else None,
        "runs_fully_covered": sum(1 for value in per_run if value >= 1.0),
        "state_changing_total": state_total,
        "state_changing_covered": state_covered,
        "state_changing_coverage": ratio(state_covered, state_total),
        "trace_abi_total": trace_abi,
        "trace_covered_total": trace_covered,
        "trace_coverage": ratio(trace_covered, trace_abi),
        "uncovered_functions": [
            {"function": name, "runs": count}
            for name, count in sorted(uncovered_counter.items(), key=lambda item: (-item[1], item[0]))[:10]
        ],
    }


def aggregate(
    metrics: Sequence[Dict[str, Any]],
    expected: Optional[int] = None,
    generated_at: Optional[str] = None,
    runs_root: Optional[str] = None,
    missing: Sequence[str] = (),
    stale: Sequence[str] = (),
    errors: Sequence[Dict[str, str]] = (),
) -> Dict[str, Any]:
    evaluable = [item for item in metrics if _evaluable(item)]
    findings_total = sum(int(item["report"]["findings_total"]) for item in evaluable)
    severity = _sum_severity(evaluable)

    runs_any_finding = sum(1 for item in evaluable if int(item["report"]["findings_total"]) > 0)
    runs_high_plus = sum(1 for item in evaluable if int(item["report"]["high_plus"]) > 0)
    runs_low_only = sum(
        1
        for item in evaluable
        if int(item["report"]["findings_total"]) > 0 and int(item["report"]["high_plus"]) == 0
    )
    max_severity_counts = _empty_severity()
    for item in evaluable:
        name = item["report"].get("max_severity")
        if name in max_severity_counts:
            max_severity_counts[name] += 1

    confidence: Dict[str, int] = {}
    for item in evaluable:
        for key, value in (item["report"].get("by_confidence") or {}).items():
            confidence[key] = confidence.get(key, 0) + int(value or 0)

    # Fork reproduction. The denominator is findings from evaluable runs; the
    # fork-specific view restricts it to runs that actually used a fork.
    findings_with_repro = sum(
        1
        for item in evaluable
        for row in item.get("findings") or []
        if row.get("has_reproduction")
    )
    repro_linked = sum(
        int((item.get("fork", {}).get("findings_repro") or {}).get("linked") or 0) for item in evaluable
    )
    repro_passed = sum(
        int((item.get("fork", {}).get("findings_repro") or {}).get("passed") or 0) for item in evaluable
    )
    fork_runs = [item for item in evaluable if (item.get("fork") or {}).get("fork_backed")]
    fork_findings = sum(int(item["report"]["findings_total"]) for item in fork_runs)
    fork_repro_passed = sum(
        int((item.get("fork", {}).get("findings_repro") or {}).get("passed") or 0) for item in fork_runs
    )
    harness_runs = [item for item in evaluable if (item.get("fork") or {}).get("tests")]
    tests_total = sum(len((item.get("fork") or {}).get("tests") or []) for item in metrics)
    fuzz_tests = sum(len((item.get("fork") or {}).get("tests_fuzz") or []) for item in metrics)
    fuzz_runs = sum(
        int(((item.get("fork") or {}).get("forge") or {}).get("fuzz_runs") or 0) for item in metrics
    )
    forge_passed = sum(
        int(((item.get("fork") or {}).get("forge") or {}).get("passed") or 0) for item in metrics
    )
    forge_failed = sum(
        int(((item.get("fork") or {}).get("forge") or {}).get("failed") or 0) for item in metrics
    )

    # Evidence integrity.
    evidence_refs = sum(int((item.get("evidence") or {}).get("refs_total") or 0) for item in metrics)
    evidence_resolved = sum(int((item.get("evidence") or {}).get("refs_resolved") or 0) for item in metrics)
    evidence_missing = sum(int((item.get("evidence") or {}).get("refs_missing") or 0) for item in metrics)
    evidence_outside = sum(int((item.get("evidence") or {}).get("refs_outside_run") or 0) for item in metrics)
    evidence_invalid = sum(int((item.get("evidence") or {}).get("refs_invalid") or 0) for item in metrics)
    findings_without_evidence = sum(
        1 for item in metrics for row in item.get("findings") or [] if int(row.get("evidence_total") or 0) == 0
    )
    findings_fully_resolved = sum(
        1
        for item in metrics
        for row in item.get("findings") or []
        if int(row.get("evidence_total") or 0) > 0 and int(row.get("evidence_missing") or 0) == 0
    )
    runs_evidence_clean = sum(
        1
        for item in metrics
        if int((item.get("evidence") or {}).get("refs_missing") or 0) == 0
        and int((item.get("evidence") or {}).get("refs_outside_run") or 0) == 0
    )

    # Declared vs observed coverage.
    declared_true = {key: 0 for key in ("source_review", "static_analysis", "fork_testing", "fuzzing")}
    observed_true = {key: 0 for key in declared_true}
    for item in metrics:
        declared = (item.get("coverage") or {}).get("declared") or {}
        observed = (item.get("coverage") or {}).get("observed") or {}
        for key in declared_true:
            if declared.get(key) is True:
                declared_true[key] += 1
            if observed.get(key) is True:
                observed_true[key] += 1
    overclaims = {"total": 0, "by_key": {}}
    underclaims = {"total": 0, "by_key": {}}
    for item in metrics:
        for key in (item.get("coverage") or {}).get("overclaims") or []:
            overclaims["total"] += 1
            overclaims["by_key"][key] = overclaims["by_key"].get(key, 0) + 1
        for key in (item.get("coverage") or {}).get("underclaims") or []:
            underclaims["total"] += 1
            underclaims["by_key"][key] = underclaims["by_key"].get(key, 0) + 1

    tooling = {
        "slither": sum(1 for item in metrics if (item.get("tooling") or {}).get("slither", {}).get("present")),
        "forge": sum(1 for item in metrics if (item.get("tooling") or {}).get("forge", {}).get("present")),
        "echidna": sum(1 for item in metrics if (item.get("tooling") or {}).get("echidna", {}).get("present")),
        "heimdall": sum(1 for item in metrics if (item.get("tooling") or {}).get("heimdall", {}).get("present")),
        "source_api": sum(
            1 for item in metrics if (item.get("tooling") or {}).get("source_api", {}).get("getsourcecode")
        ),
        "bytecode_match": sum(
            1 for item in metrics if (item.get("tooling") or {}).get("bytecode_match") is True
        ),
        "slither_signals": sum(
            int((item.get("tooling") or {}).get("slither", {}).get("json_results") or 0) for item in metrics
        ),
        "cast_traces": sum(
            int((item.get("tooling") or {}).get("cast", {}).get("traces") or 0) for item in metrics
        ),
    }

    anomaly_counts: Dict[str, int] = {}
    anomaly_runs = 0
    for item in metrics:
        codes = [entry.get("code", "unknown") for entry in item.get("anomalies") or []]
        if codes:
            anomaly_runs += 1
        for code in codes:
            anomaly_counts[code] = anomaly_counts.get(code, 0) + 1

    addresses = [
        (item.get("target") or {}).get("address", "").lower()
        for item in metrics
        if (item.get("target") or {}).get("address")
    ]
    unique_addresses = len(set(addresses))

    status_counts: Dict[str, int] = {}
    grade_counts: Dict[str, int] = {}
    for item in metrics:
        status = str((item.get("run") or {}).get("status") or "unknown")
        status_counts[status] = status_counts.get(status, 0) + 1
        grade = str((item.get("report") or {}).get("grade") or "missing")
        grade_counts[grade] = grade_counts.get(grade, 0) + 1

    funnel = [
        {"stage": "runs_discovered", "count": len(metrics) + len(missing)},
        {"stage": "metrics_present", "count": len(metrics)},
        {"stage": "report_present", "count": sum(1 for item in metrics if (item.get("report") or {}).get("present"))},
        {"stage": "report_parsed", "count": sum(1 for item in metrics if (item.get("report") or {}).get("parsed"))},
        {
            "stage": "schema_current_or_legacy",
            "count": sum(
                1 for item in metrics if (item.get("report") or {}).get("grade") in {"current", "legacy"}
            ),
        },
        {"stage": "evaluable", "count": len(evaluable)},
        {"stage": "any_finding", "count": runs_any_finding},
        {"stage": "high_plus_finding", "count": runs_high_plus},
    ]

    return {
        "generated_at": generated_at or utc_now_iso(),
        "aggregator_version": __version__,
        "stats_schema_version": STATS_SCHEMA_VERSION,
        "runs_root": runs_root,
        "round": {
            "expected": expected,
            "completed": len(metrics),
            "missing": len(missing),
            "completion_rate": ratio(len(metrics), expected) if expected else None,
            "missing_sample": list(missing)[:MISSING_SAMPLE],
            "stale_metrics": list(stale),
            "load_errors": list(errors),
        },
        "runs": {
            "total": len(metrics),
            "evaluable": len(evaluable),
            "by_status": status_counts,
            "by_schema_grade": grade_counts,
            "with_anomalies": anomaly_runs,
        },
        "funnel": funnel,
        "findings": {
            "total": findings_total,
            "by_severity": severity,
            "by_confidence": confidence,
            "high_plus": sum(severity[name] for name in HIGH_PLUS),
            "critical": severity["critical"],
            "per_evaluable_run": ratio(findings_total, len(evaluable)),
        },
        "detection": {
            "denominator": len(evaluable),
            "runs_any_finding": runs_any_finding,
            "rate_any": ratio(runs_any_finding, len(evaluable)),
            "runs_high_plus": runs_high_plus,
            "rate_high_plus": ratio(runs_high_plus, len(evaluable)),
            "runs_below_high": runs_low_only,
            "rate_below_high": ratio(runs_low_only, len(evaluable)),
            "max_severity_by_run": max_severity_counts,
        },
        "fork": {
            "findings": findings_total,
            "findings_with_reproduction": findings_with_repro,
            "repro_linked": repro_linked,
            "repro_passed": repro_passed,
            "repro_rate": ratio(repro_passed, findings_total),
            "repro_linked_rate": ratio(repro_linked, findings_total),
            "runs_with_harness": len(harness_runs),
            "runs_fork_backed": len(fork_runs),
            "fork_findings": fork_findings,
            "fork_repro_passed": fork_repro_passed,
            "fork_repro_rate": ratio(fork_repro_passed, fork_findings),
            "tests_total": tests_total,
            "tests_fuzz": fuzz_tests,
            "fuzz_runs": fuzz_runs,
            "forge_passed": forge_passed,
            "forge_failed": forge_failed,
        },
        "function_coverage": function_coverage_summary(metrics),
        "stability": stability_summary(evaluable),
        "evidence": {
            "refs_total": evidence_refs,
            "refs_resolved": evidence_resolved,
            "refs_missing": evidence_missing,
            "refs_outside_run": evidence_outside,
            "refs_invalid": evidence_invalid,
            "resolution_rate": ratio(evidence_resolved, evidence_refs),
            "findings_without_evidence": findings_without_evidence,
            "findings_fully_resolved": findings_fully_resolved,
            "runs_clean": runs_evidence_clean,
        },
        "coverage": {
            "declared_true": declared_true,
            "observed_true": observed_true,
            "overclaims": overclaims,
            "underclaims": underclaims,
        },
        "tooling": tooling,
        "anomalies": {
            "total": sum(anomaly_counts.values()),
            "runs_affected": anomaly_runs,
            "by_code": dict(sorted(anomaly_counts.items(), key=lambda item: (-item[1], item[0]))),
        },
        "latency": {
            **_duration_stats(
                [
                    (item.get("run") or {}).get("duration_seconds")
                    for item in metrics
                    if (item.get("run") or {}).get("duration_seconds") is not None
                ]
            ),
        },
        "targets": {
            "unique_addresses": unique_addresses,
            "repeated": max(0, len(metrics) - unique_addresses),
        },
        "strata": {
            "by_model": _strata(metrics, lambda item: str((item.get("run") or {}).get("model") or "unknown")),
            "by_chain": _strata(metrics, lambda item: str((item.get("target") or {}).get("chain") or "unknown")),
            "by_verified": _strata(
                metrics,
                lambda item: (
                    "verified"
                    if (item.get("target") or {}).get("verified") is True
                    else "unverified"
                    if (item.get("target") or {}).get("verified") is False
                    else "unknown"
                ),
            ),
            "by_schema_grade": _strata(
                metrics, lambda item: str((item.get("report") or {}).get("grade") or "missing")
            ),
        },
    }


# --- rendering --------------------------------------------------------------


def _table(headers: Sequence[str], rows: Sequence[Sequence[Any]]) -> str:
    lines = ["| " + " | ".join(headers) + " |", "| " + " | ".join("---" for _ in headers) + " |"]
    for row in rows:
        lines.append("| " + " | ".join(str(value) for value in row) + " |")
    return "\n".join(lines)


def render_markdown(summary: Dict[str, Any], metrics: Sequence[Dict[str, Any]]) -> str:
    findings = summary["findings"]
    detection = summary["detection"]
    fork = summary["fork"]
    evidence = summary["evidence"]
    latency = summary["latency"]

    out: List[str] = []
    out.append("# Audit round statistics")
    out.append("")
    out.append(
        "Generated {generated} by audit_stats {version} from `{root}`.".format(
            generated=summary["generated_at"],
            version=summary["aggregator_version"],
            root=summary["runs_root"],
        )
    )
    out.append("")
    out.append(
        "**Runs:** {total} discovered, {evaluable} evaluable. "
        "**Findings:** {findings} ({high_plus} high+). "
        "**Fork reproduction:** {repro_passed}/{findings} findings ({rate}).".format(
            total=summary["runs"]["total"],
            evaluable=summary["runs"]["evaluable"],
            findings=findings["total"],
            high_plus=findings["high_plus"],
            repro_passed=fork["repro_passed"],
            rate=format_pct(fork["repro_rate"]),
        )
    )
    out.append("")

    out.append("## 1. Funnel")
    out.append("")
    out.append(
        _table(
            ["Stage", "Runs", "Of discovered"],
            [
                [
                    stage["stage"],
                    stage["count"],
                    format_pct(ratio(stage["count"], summary["funnel"][0]["count"])),
                ]
                for stage in summary["funnel"]
            ],
        )
    )
    out.append("")

    out.append("## 2. Findings")
    out.append("")
    out.append(
        _table(
            ["Severity", "Findings", "Share"],
            [
                [name, findings["by_severity"].get(name, 0), format_pct(ratio(findings["by_severity"].get(name, 0), findings["total"]))]
                for name in SEVERITY_ORDER
            ],
        )
    )
    out.append("")
    out.append(
        _table(
            ["Detection metric", "Runs", "Rate"],
            [
                ["Any finding", detection["runs_any_finding"], format_pct(detection["rate_any"])],
                ["High or critical", detection["runs_high_plus"], format_pct(detection["rate_high_plus"])],
                [
                    "Below high (no high/critical)",
                    detection["runs_below_high"],
                    format_pct(detection["rate_below_high"]),
                ],
            ],
        )
    )
    out.append("")
    out.append(
        "Denominator for rates is the {n} evaluable run(s): report parsed, schema current or "
        "legacy, run not failed, report status not `blocked`.".format(n=detection["denominator"])
    )
    out.append("")
    out.append(
        "`Below high` counts evaluable runs with at least one finding and no high/critical; "
        "zero-finding runs are not in this row."
    )
    out.append("")

    high_plus_rows: List[Dict[str, Any]] = []
    for item in metrics:
        for finding in item.get("findings") or []:
            if finding.get("severity") in HIGH_PLUS:
                high_plus_rows.append(
                    {
                        "run_id": item.get("run_id"),
                        "id": finding.get("id"),
                        "severity": finding.get("severity"),
                        "title": str(finding.get("title") or "")[:120],
                        "repro": "yes" if finding.get("repro_passed") else "no",
                    }
                )
    high_plus_rows.sort(key=lambda row: (SEVERITY_ORDER.index(row["severity"]), str(row["run_id"])))
    if high_plus_rows:
        out.append("### High and critical findings")
        out.append("")
        out.append(
            _table(
                ["Run", "ID", "Severity", "Title", "Fork repro"],
                [
                    [row["run_id"], row["id"], row["severity"], row["title"], row["repro"]]
                    for row in high_plus_rows[:30]
                ],
            )
        )
        if len(high_plus_rows) > 30:
            out.append("")
            out.append("_{} more omitted._".format(len(high_plus_rows) - 30))
        out.append("")

    out.append("## 3. Fork reproduction")
    out.append("")
    out.append(
        _table(
            ["Metric", "Count", "Rate"],
            [
                ["Findings", fork["findings"], "n/a"],
                [
                    "With a reproduction command",
                    fork["findings_with_reproduction"],
                    format_pct(ratio(fork["findings_with_reproduction"], fork["findings"])),
                ],
                ["Linked to a harness test", fork["repro_linked"], format_pct(fork["repro_linked_rate"])],
                ["Linked test passed", fork["repro_passed"], format_pct(fork["repro_rate"])],
                [
                    "Findings in fork-backed runs",
                    fork["fork_findings"],
                    format_pct(fork["fork_repro_rate"]),
                ],
            ],
        )
    )
    out.append("")
    out.append(
        "Runs with a fork harness: {harness}; runs with a detected fork: {backed}; "
        "harness tests: {tests}; fuzz tests: {fuzz}; fuzz runs executed: {runs}; "
        "forge pass/fail: {passed}/{failed}.".format(
            harness=fork["runs_with_harness"],
            backed=fork["runs_fork_backed"],
            tests=fork["tests_total"],
            fuzz=fork["tests_fuzz"],
            runs=fork["fuzz_runs"],
            passed=fork["forge_passed"],
            failed=fork["forge_failed"],
        )
    )
    out.append("")

    fc = summary["function_coverage"]
    out.append("## 4. Function coverage")
    out.append("")
    out.append(
        _table(
            ["Metric", "Value"],
            [
                ["Runs with a target ABI", fc["runs_with_abi"]],
                ["Runs with trace evidence", fc["runs_with_trace"]],
                ["ABI functions (sum over runs)", fc["abi_functions_total"]],
                ["Covered by trace or harness (sum)", fc["covered_total"]],
                ["Coverage (trace + harness)", format_pct(fc["coverage"])],
                ["Coverage (trace only)", format_pct(fc["trace_coverage"])],
                ["State-changing coverage", format_pct(fc["state_changing_coverage"])],
                ["Runs fully covered", fc["runs_fully_covered"]],
                ["Runs exercising fallback/receive", fc["runs_with_fallback"]],
            ],
        )
    )
    out.append("")
    out.append(
        "Basis: {}. `trace + harness` is an upper bound (a referenced call may sit in "
        "an untaken branch); `trace only` is a lower bound limited to traces the agent "
        "saved.".format(
            ", ".join("{}={}".format(key, value) for key, value in fc["basis"].items()) or "none"
        )
    )
    out.append("")
    if fc["uncovered_functions"]:
        out.append(
            _table(
                ["Never executed function", "Runs"],
                [[row["function"], row["runs"]] for row in fc["uncovered_functions"]],
            )
        )
        out.append("")

    out.append("## 5. Evidence integrity")
    out.append("")
    out.append(
        _table(
            ["Metric", "Value"],
            [
                ["Evidence references", evidence["refs_total"]],
                ["Resolved on disk", "{} ({})".format(evidence["refs_resolved"], format_pct(evidence["resolution_rate"]))],
                ["Missing", evidence["refs_missing"]],
                ["Outside run directory (rejected)", evidence["refs_outside_run"]],
                ["Findings without evidence", evidence["findings_without_evidence"]],
                ["Runs with clean evidence", evidence["runs_clean"]],
            ],
        )
    )
    out.append("")

    out.append("## 6. Coverage: claimed vs observed")
    out.append("")
    out.append(
        _table(
            ["Signal", "Declared true", "Observed on disk", "Overclaims"],
            [
                [
                    key,
                    summary["coverage"]["declared_true"].get(key, 0),
                    summary["coverage"]["observed_true"].get(key, 0),
                    summary["coverage"]["overclaims"]["by_key"].get(key, 0),
                ]
                for key in ("source_review", "static_analysis", "fork_testing", "fuzzing")
            ],
        )
    )
    out.append("")

    out.append("## 7. Tooling")
    out.append("")
    out.append(
        _table(
            ["Tool/signal", "Runs"],
            [[key, value] for key, value in summary["tooling"].items()],
        )
    )
    out.append("")

    out.append("## 8. Anomalies")
    out.append("")
    if summary["anomalies"]["by_code"]:
        out.append(
            _table(
                ["Code", "Occurrences"],
                [
                    [code, count]
                    for code, count in list(summary["anomalies"]["by_code"].items())[:ANOMALY_TOP_N]
                ],
            )
        )
    else:
        out.append("No anomalies recorded.")
    out.append("")

    out.append("## 9. Latency")
    out.append("")
    out.append(
        _table(
            ["n", "p50", "p90", "max", "total"],
            [
                [
                    latency["n"],
                    format_seconds(latency["p50"]),
                    format_seconds(latency["p90"]),
                    format_seconds(latency["max"]),
                    format_seconds(latency["total_seconds"]),
                ]
            ],
        )
    )
    out.append("")

    stability = summary["stability"]
    out.append("## 10. Stability across repeated audits")
    out.append("")
    if stability["pairs"]:
        out.append(
            _table(
                ["Metric", "Value"],
                [
                    ["Targets audited more than once", stability["targets_with_repeat"]],
                    ["Comparable pairs", stability["pairs"]],
                    ["Mean finding-set overlap", format_pct(stability["mean_overlap"])],
                    ["Median finding-set overlap", format_pct(stability["median_overlap"])],
                    ["Mean best title similarity", format_pct(stability["mean_best_similarity"])],
                    ["Pairs with zero overlap", stability["zero_overlap_pairs"]],
                    ["Matches anchored by function", stability["matched_by_function"]],
                    ["Mean absolute severity delta", (
                        "{:.2f}".format(stability["mean_abs_severity_delta"])
                        if stability["mean_abs_severity_delta"] is not None
                        else "n/a"
                    )],
                    ["Pairs with severity jump >= 2", stability["severe_drift_pairs"]],
                    [
                        "Like-for-like (same model/image/prompt)",
                        "{} pair(s), {}".format(
                            stability["like_for_like"]["pairs"],
                            format_pct(stability["like_for_like"]["mean_overlap"]),
                        ),
                    ],
                    [
                        "Changed conditions",
                        "{} pair(s), {}".format(
                            stability["changed_conditions"]["pairs"],
                            format_pct(stability["changed_conditions"]["mean_overlap"]),
                        ),
                    ],
                ],
            )
        )
        out.append("")
        if stability["severity_delta_distribution"]:
            out.append("Severity delta (new run minus old run):")
            out.append("")
            out.append(
                _table(
                    ["Delta", "Matched findings"],
                    [[key, value] for key, value in stability["severity_delta_distribution"].items()],
                )
            )
            out.append("")
        if stability["worst_targets"]:
            out.append("Least stable targets:")
            out.append("")
            out.append(
                _table(
                    ["Chain", "Address", "Runs", "Pairs", "Mean overlap"],
                    [
                        [
                            row["chain"],
                            row["address"],
                            row["runs"],
                            row["pairs"],
                            format_pct(row["mean_overlap"]),
                        ]
                        for row in stability["worst_targets"]
                    ],
                )
            )
            out.append("")
    else:
        out.append(
            "No target has two evaluable runs yet. Re-audit a target (same prompt scope) "
            "to compute title overlap and severity drift."
        )
        out.append("")
    out.append(
        "Matching uses the finding `function` when it overlaps (title similarity floor "
        "{floor}), otherwise title similarity at {threshold}. Title similarity is the max of "
        "token Jaccard, overlap coefficient, and sequence ratio. Stability measures "
        "repeatability, not correctness: two runs can agree and both be wrong.".format(
            floor=stability["function_floor"], threshold=stability["threshold"]
        )
    )
    out.append("")

    out.append("## 11. Strata")
    out.append("")
    for name in ("by_model", "by_chain", "by_verified", "by_schema_grade"):
        rows = summary["strata"][name]
        out.append("### {}".format(name))
        out.append("")
        out.append(
            _table(
                ["Key", "Runs", "Evaluable", "Findings", "High+", "High+ rate", "Repro rate", "p50"],
                [
                    [
                        row["key"],
                        row["runs"],
                        row["evaluable"],
                        row["findings"],
                        row["high_plus"],
                        format_pct(row["high_plus_rate"]),
                        format_pct(row["repro_rate"]),
                        format_seconds(row["p50_duration"]),
                    ]
                    for row in rows
                ],
            )
        )
        out.append("")

    out.append("## 12. Round completeness")
    out.append("")
    round_info = summary["round"]
    if round_info["expected"]:
        out.append(
            "Expected {expected}, collected {completed} ({rate}).".format(
                expected=round_info["expected"],
                completed=round_info["completed"],
                rate=format_pct(round_info["completion_rate"]),
            )
        )
    else:
        out.append("Expected count not provided; `--expected N` computes round completion.")
    if round_info["missing_sample"]:
        out.append("")
        out.append(
            "Runs without metrics ({n}): {}".format(
                n=round_info["missing"], sample=", ".join(round_info["missing_sample"])
            )
        )
    out.append("")

    out.append("## Caveats")
    out.append("")
    out.append(
        "- Severity is assigned by the audit agent. It is a triage signal, not ground truth.\n"
        "- Evidence checks prove a referenced file exists inside the run directory; they do not\n"
        "  prove the file supports the claim.\n"
        "- A passing harness test proves the test ran; `runs_fork_backed` shows whether a fork was\n"
        "  actually used. A green local-EVM test is not a fork reproduction.\n"
        "- Function coverage unions trace-observed and harness-referenced calls; it is an upper\n"
        "  bound, and `trace only` sees just the traces the agent saved. It is not line/branch\n"
        "  coverage of the target source.\n"
        "- Stability overlap measures repeatability, not correctness. Two runs can agree and both\n"
        "  be wrong; compare like-for-like model/image/prompt conditions. Function-anchored\n"
        "  matches can pair different issues in the same function; the CSV records how each\n"
        "  match was anchored.\n"
        "- Legacy reports (older schema) are counted when they contain a findings list; see the\n"
        "  schema-grade stratum."
    )
    out.append("")
    return "\n".join(out)


# --- outputs ----------------------------------------------------------------


RUN_FIELDS = (
    "run_id",
    "status",
    "model",
    "reasoning_effort",
    "chain",
    "address",
    "verified",
    "schema_grade",
    "findings_total",
    "critical",
    "high",
    "medium",
    "low",
    "informational",
    "high_plus",
    "max_severity",
    "repro_linked",
    "repro_passed",
    "repro_rate",
    "evidence_refs",
    "evidence_missing",
    "anomalies",
    "duration_seconds",
    "artifacts_files",
    "function_coverage",
    "function_coverage_trace",
    "uncovered_functions",
)

STABILITY_FIELDS = (
    "chain",
    "address",
    "run_a",
    "run_b",
    "findings_a",
    "findings_b",
    "matched",
    "union",
    "overlap",
    "empty_pair",
    "mean_best_similarity",
    "matched_by_function",
    "matched_by_title",
    "mean_best_jaccard",
    "severity_delta_mean",
    "severity_delta_max_abs",
    "max_severity_a",
    "max_severity_b",
    "only_in_a",
    "only_in_b",
    "same_model",
    "same_image",
    "same_prompt",
    "like_for_like",
)

FINDING_FIELDS = (
    "run_id",
    "id",
    "title",
    "severity",
    "confidence",
    "contract",
    "function",
    "has_reproduction",
    "repro_linked",
    "repro_passed",
    "evidence_total",
    "evidence_missing",
    "severity_ok",
    "confidence_ok",
)


def run_rows(metrics: Sequence[Dict[str, Any]]) -> List[Dict[str, Any]]:
    rows: List[Dict[str, Any]] = []
    for item in metrics:
        report = item.get("report") or {}
        run = item.get("run") or {}
        target = item.get("target") or {}
        fork = item.get("fork") or {}
        repro = fork.get("findings_repro") or {}
        severity = report.get("by_severity") or {}
        rows.append(
            {
                "run_id": item.get("run_id"),
                "status": run.get("status"),
                "model": run.get("model"),
                "reasoning_effort": run.get("reasoning_effort"),
                "chain": target.get("chain"),
                "address": target.get("address"),
                "verified": target.get("verified"),
                "schema_grade": report.get("grade"),
                "findings_total": report.get("findings_total"),
                "critical": severity.get("critical", 0),
                "high": severity.get("high", 0),
                "medium": severity.get("medium", 0),
                "low": severity.get("low", 0),
                "informational": severity.get("informational", 0),
                "high_plus": report.get("high_plus"),
                "max_severity": report.get("max_severity"),
                "repro_linked": repro.get("linked"),
                "repro_passed": repro.get("passed"),
                "repro_rate": repro.get("rate"),
                "evidence_refs": (item.get("evidence") or {}).get("refs_total"),
                "evidence_missing": (item.get("evidence") or {}).get("refs_missing"),
                "anomalies": len(item.get("anomalies") or []),
                "duration_seconds": run.get("duration_seconds"),
                "artifacts_files": (item.get("volume") or {}).get("artifacts_files"),
                "function_coverage": (item.get("function_coverage") or {}).get("coverage"),
                "function_coverage_trace": (item.get("function_coverage") or {}).get("trace_coverage"),
                "uncovered_functions": ";".join(
                    (item.get("function_coverage") or {}).get("uncovered") or []
                ),
            }
        )
    return rows


def finding_rows(metrics: Sequence[Dict[str, Any]]) -> List[Dict[str, Any]]:
    rows: List[Dict[str, Any]] = []
    for item in metrics:
        for finding in item.get("findings") or []:
            row = {field: finding.get(field) for field in FINDING_FIELDS if field != "run_id"}
            row["run_id"] = item.get("run_id")
            row["repro_linked"] = int(bool(finding.get("repro_linked")))
            row["repro_passed"] = int(bool(finding.get("repro_passed")))
            rows.append(row)
    return rows


def write_outputs(
    out_dir: Path,
    summary: Dict[str, Any],
    metrics: Sequence[Dict[str, Any]],
) -> Dict[str, Path]:
    out_dir = Path(out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    paths: Dict[str, Path] = {}

    summary_path = out_dir / "summary.json"
    summary_path.write_text(
        json.dumps(summary, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
    )
    paths["summary"] = summary_path

    report_path = out_dir / "REPORT.md"
    report_path.write_text(render_markdown(summary, metrics), encoding="utf-8")
    paths["report"] = report_path

    runs_path = out_dir / "runs.csv"
    with runs_path.open("w", encoding="utf-8", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=list(RUN_FIELDS))
        writer.writeheader()
        writer.writerows(run_rows(metrics))
    paths["runs_csv"] = runs_path

    findings_path = out_dir / "findings.csv"
    with findings_path.open("w", encoding="utf-8", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=list(FINDING_FIELDS))
        writer.writeheader()
        writer.writerows(finding_rows(metrics))
    paths["findings_csv"] = findings_path

    stability_path = out_dir / "stability.csv"
    with stability_path.open("w", encoding="utf-8", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=list(STABILITY_FIELDS), extrasaction="ignore")
        writer.writeheader()
        writer.writerows((summary.get("stability") or {}).get("pairs_detail") or [])
    paths["stability_csv"] = stability_path

    return paths
