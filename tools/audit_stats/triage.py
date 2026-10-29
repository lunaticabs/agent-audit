"""Findings that need a human look, for one round.

`collect` records what happened; `report` aggregates it. Neither answers the
question an auditor actually asks after a batch: *which findings are we not yet
entitled to believe?* That is what `triage` produces.

A finding is listed when at least one of these holds:

``not_reproduced``   it names a test, and that test did not pass
``unattested``       its PASS exists only in the agent's log, although a replay ran
``log_only``         its PASS exists only in the agent's log because no replay ran
``evidence_missing`` no evidence reference, or at least one that does not resolve
``no_reproduction``  it names no test at all
``vacuity_risk``     attested, but the test passed at both blocks (may assert nothing)
``unknown_severity`` severity could not be normalized

The output is advisory. `triage` never claims a finding is false, and its absence
from the list is not a clean bill of health: a test that asserts something true
at every block is replayed faithfully and passes.
"""

from __future__ import annotations

import csv
from pathlib import Path
from typing import Any, Dict, Iterable, List, Optional, Sequence

from . import __version__
from .common import HIGH_PLUS, SEVERITY_ORDER, utc_now_iso

TRIAGE_VERSION = 1

#: Reasons in priority order. Lower index = more urgent.
REASON_ORDER = (
    "not_reproduced",
    "unattested",
    "log_only",
    "evidence_missing",
    "no_reproduction",
    "vacuity_risk",
    "unknown_severity",
)

REASON_WEIGHT = {name: index for index, name in enumerate(REASON_ORDER)}

#: Anomalies that mean a claim and what can be recomputed disagree. These are
#: the codes worth reading before trusting a run at all.
INTEGRITY_ANOMALIES = {
    "repro_contradicted_by_replay",
    "repro_unattested",
    "forge_log_test_unknown",
    "fork_marker_not_in_test_body",
    "fork_replay_failed",
    "target_address_mismatch",
    "run_id_mismatch",
    "evidence_outside_run",
    "slither_result_mismatch",
    "declared_coverage_overclaim",
}


def _replay_status(metrics: Dict[str, Any]) -> Optional[str]:
    return ((metrics.get("fork") or {}).get("replay") or {}).get("status")


def finding_reasons(row: Dict[str, Any], replay_status: Optional[str]) -> List[str]:
    """Return the reasons this finding needs review, in priority order."""
    reasons: set = set()
    has_repro = bool(row.get("has_reproduction"))
    passed = list(row.get("repro_passed") or [])

    if has_repro and not passed:
        reasons.add("not_reproduced")
    if not has_repro:
        reasons.add("no_reproduction")
    if passed:
        if row.get("repro_attested"):
            if not row.get("repro_state_sensitive"):
                reasons.add("vacuity_risk")
        elif replay_status in {"ok", "error"}:
            reasons.add("unattested")
        else:
            reasons.add("log_only")
    if not int(row.get("evidence_total") or 0) or int(row.get("evidence_missing") or 0) > 0:
        reasons.add("evidence_missing")
    if row.get("severity") == "unknown":
        reasons.add("unknown_severity")

    return [name for name in REASON_ORDER if name in reasons]


def _severity_index(severity: str) -> int:
    try:
        return SEVERITY_ORDER.index(severity)
    except ValueError:
        return len(SEVERITY_ORDER)


def _sort_key(row: Dict[str, Any]) -> tuple:
    reasons = row.get("reasons") or []
    weight = min((REASON_WEIGHT.get(name, 99) for name in reasons), default=99)
    return (_severity_index(str(row.get("severity") or "unknown")), weight, str(row.get("run_id")), str(row.get("id")))


def triage(
    metrics: Sequence[Dict[str, Any]],
    missing: Iterable[str] = (),
    errors: Optional[Sequence[Dict[str, str]]] = None,
    now: Optional[str] = None,
) -> Dict[str, Any]:
    """Collect findings that need review plus the runs with integrity anomalies."""
    rows: List[Dict[str, Any]] = []
    run_rows: List[Dict[str, Any]] = []
    reason_counts = {name: 0 for name in REASON_ORDER}
    findings_total = 0
    high_plus_total = 0

    for item in metrics:
        run_id = str(item.get("run_id") or "")
        report = item.get("report") or {}
        findings_total += int(report.get("findings_total") or 0)
        high_plus_total += int(report.get("high_plus") or 0)

        anomalies = [
            str(entry.get("code") or "unknown")
            for entry in item.get("anomalies") or []
            if isinstance(entry, dict)
        ]
        integrity = sorted({code for code in anomalies if code in INTEGRITY_ANOMALIES})
        if anomalies:
            run_rows.append(
                {
                    "run_id": run_id,
                    "status": (item.get("run") or {}).get("status"),
                    "anomalies": anomalies,
                    "integrity": integrity,
                }
            )

        replay_status = _replay_status(item)
        for finding in item.get("findings") or []:
            reasons = finding_reasons(finding, replay_status)
            if not reasons:
                continue
            for name in reasons:
                reason_counts[name] += 1
            rows.append(
                {
                    "run_id": run_id,
                    "id": finding.get("id"),
                    "severity": finding.get("severity"),
                    "title": finding.get("title"),
                    "contract": finding.get("contract"),
                    "reasons": reasons,
                    "repro_source": finding.get("repro_source"),
                    "repro_tests": finding.get("repro_tests") or [],
                    "repro_passed": finding.get("repro_passed") or [],
                    "evidence_total": finding.get("evidence_total"),
                    "evidence_missing": finding.get("evidence_missing"),
                }
            )

    rows.sort(key=_sort_key)
    run_rows.sort(key=lambda row: (not row["integrity"], row["run_id"]))

    return {
        "triage_version": TRIAGE_VERSION,
        "tool_version": __version__,
        "generated_at": now or utc_now_iso(),
        "totals": {
            "runs": len(metrics),
            "findings": findings_total,
            "high_plus": high_plus_total,
            "needs_review": len(rows),
            "runs_with_anomalies": len(run_rows),
            "runs_with_integrity_anomalies": sum(1 for row in run_rows if row["integrity"]),
            "missing_metrics": len(list(missing)),
            "by_reason": reason_counts,
        },
        "findings": rows,
        "runs": run_rows,
        "missing_metrics": list(missing),
        "errors": list(errors or []),
    }


# --- rendering --------------------------------------------------------------

TRIAGE_FIELDS = (
    "run_id",
    "id",
    "severity",
    "title",
    "reasons",
    "repro_source",
    "repro_tests",
    "repro_passed",
    "evidence_total",
    "evidence_missing",
)


def _table(headers: Sequence[str], rows: Sequence[Sequence[Any]]) -> str:
    lines = ["| " + " | ".join(headers) + " |", "| " + " | ".join("---" for _ in headers) + " |"]
    for row in rows:
        lines.append("| " + " | ".join(str(value) for value in row) + " |")
    return "\n".join(lines)


def render_markdown(payload: Dict[str, Any]) -> str:
    totals = payload["totals"]
    reasons = totals["by_reason"]
    out: List[str] = []
    out.append("# Audit triage")
    out.append("")
    out.append(
        "Generated {generated} by audit_stats {version}.".format(
            generated=payload["generated_at"], version=payload["tool_version"]
        )
    )
    out.append("")
    out.append(
        "**{runs} run(s), {findings} finding(s), {review} need a human look.** "
        "not_reproduced={nr}, unattested={ua}, log_only={lo}, evidence_missing={em}, "
        "no_reproduction={nrep}, vacuity_risk={vr}, unknown_severity={us}.".format(
            runs=totals["runs"],
            findings=totals["findings"],
            review=totals["needs_review"],
            nr=reasons["not_reproduced"],
            ua=reasons["unattested"],
            lo=reasons["log_only"],
            em=reasons["evidence_missing"],
            nrep=reasons["no_reproduction"],
            vr=reasons["vacuity_risk"],
            us=reasons["unknown_severity"],
        )
    )
    out.append("")
    out.append(
        "`not_reproduced` = names a test that did not pass. `unattested` = the PASS is only in the "
        "agent's log even though a replay ran. `log_only` = no replay ran, so every PASS is log-only. "
        "`vacuity_risk` = attested, but the test passed at both blocks, so it may assert nothing."
    )
    out.append("")

    out.append("## 1. Findings that need a human")
    out.append("")
    if payload["findings"]:
        out.append(
            _table(
                ["Run", "ID", "Severity", "Title", "Why"],
                [
                    [
                        row["run_id"],
                        row["id"],
                        row["severity"],
                        str(row["title"] or "")[:80],
                        ", ".join(row["reasons"]),
                    ]
                    for row in payload["findings"][:100]
                ],
            )
        )
        if len(payload["findings"]) > 100:
            out.append("")
            out.append("_{} more in triage.csv._".format(len(payload["findings"]) - 100))
    else:
        out.append("None.")
    out.append("")

    out.append("## 2. Runs with anomalies")
    out.append("")
    if payload["runs"]:
        out.append(
            _table(
                ["Run", "Status", "Integrity", "Anomalies"],
                [
                    [
                        row["run_id"],
                        row["status"],
                        "yes" if row["integrity"] else "no",
                        ", ".join(row["anomalies"]),
                    ]
                    for row in payload["runs"]
                ],
            )
        )
    else:
        out.append("None.")
    out.append("")

    if payload["missing_metrics"]:
        out.append("## 3. Runs without metrics")
        out.append("")
        out.append(
            "{} run(s): {}".format(
                len(payload["missing_metrics"]), ", ".join(payload["missing_metrics"][:20])
            )
        )
        out.append("")

    return "\n".join(out).rstrip() + "\n"


def _csv_value(value: Any) -> str:
    if isinstance(value, (list, tuple)):
        return ";".join(str(item) for item in value)
    return "" if value is None else str(value)


def write_outputs(out_dir: Path, payload: Dict[str, Any]) -> Dict[str, Path]:
    out_dir = Path(out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)

    markdown_path = out_dir / "TRIAGE.md"
    markdown_path.write_text(render_markdown(payload), encoding="utf-8")

    csv_path = out_dir / "triage.csv"
    with csv_path.open("w", encoding="utf-8", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=list(TRIAGE_FIELDS))
        writer.writeheader()
        for row in payload["findings"]:
            writer.writerow({field: _csv_value(row.get(field)) for field in TRIAGE_FIELDS})

    return {"triage_markdown": markdown_path, "triage_csv": csv_path}


def console_summary(payload: Dict[str, Any], limit: int = 20) -> List[str]:
    totals = payload["totals"]
    reasons = totals["by_reason"]
    lines = [
        "triage: {runs} run(s), {findings} finding(s); {review} need review".format(
            runs=totals["runs"], findings=totals["findings"], review=totals["needs_review"]
        ),
        "  not_reproduced={nr} unattested={ua} log_only={lo} evidence_missing={em} "
        "no_reproduction={nrep} vacuity_risk={vr}".format(
            nr=reasons["not_reproduced"],
            ua=reasons["unattested"],
            lo=reasons["log_only"],
            em=reasons["evidence_missing"],
            nrep=reasons["no_reproduction"],
            vr=reasons["vacuity_risk"],
        ),
    ]
    for row in payload["findings"][: max(0, limit)]:
        lines.append(
            "  [{severity}] {run_id} {id} {title}  ({reasons})".format(
                severity=row["severity"],
                run_id=row["run_id"],
                id=row["id"],
                title=str(row["title"] or "")[:60],
                reasons=",".join(row["reasons"]),
            )
        )
    if len(payload["findings"]) > limit:
        lines.append("  ... {} more".format(len(payload["findings"]) - limit))

    if totals["runs_with_integrity_anomalies"]:
        lines.append(
            "  runs with integrity anomalies: {}".format(totals["runs_with_integrity_anomalies"])
        )
        for row in payload["runs"]:
            if row["integrity"]:
                lines.append("    {}: {}".format(row["run_id"], ",".join(row["integrity"])))
    return lines


def high_plus_needing_review(payload: Dict[str, Any]) -> List[Dict[str, Any]]:
    """Convenience for callers that only care about high and critical findings."""
    return [row for row in payload["findings"] if row["severity"] in HIGH_PLUS]
