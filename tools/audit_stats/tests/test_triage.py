from __future__ import annotations

import csv
import tempfile
import unittest
from pathlib import Path

from audit_stats.triage import (
    console_summary,
    finding_reasons,
    render_markdown,
    triage,
    write_outputs,
)


def finding(
    finding_id: str,
    severity: str = "high",
    has_repro: bool = True,
    passed: bool = True,
    attested: bool = True,
    state_sensitive: bool = True,
    evidence_total: int = 1,
    evidence_missing: int = 0,
    source: str = "replay",
) -> dict:
    return {
        "id": finding_id,
        "severity": severity,
        "title": "t {}".format(finding_id),
        "contract": "0xabc",
        "has_reproduction": has_repro,
        "repro_tests": ["test_x"] if has_repro else [],
        "repro_linked": ["test_x"] if has_repro else [],
        "repro_passed": ["test_x"] if passed else [],
        "repro_attested": attested,
        "repro_state_sensitive": state_sensitive,
        "repro_source": source if passed else None,
        "evidence_total": evidence_total,
        "evidence_missing": evidence_missing,
    }


def metric(
    run_id: str = "run-1",
    findings=None,
    replay_status: str = "ok",
    anomalies=None,
    status: str = "completed",
) -> dict:
    findings = findings or []
    return {
        "run_id": run_id,
        "run": {"status": status},
        "report": {"findings_total": len(findings), "high_plus": 1 if findings else 0},
        "findings": findings,
        "anomalies": [{"code": code, "detail": ""} for code in (anomalies or [])],
        "fork": {"replay": {"status": replay_status}},
    }


class FindingReasonTests(unittest.TestCase):
    def test_clean_finding_has_no_reason(self):
        self.assertEqual(finding_reasons(finding("F-1"), "ok"), [])

    def test_reproduction_that_did_not_pass(self):
        reasons = finding_reasons(finding("F-1", passed=False, attested=False), "ok")
        self.assertIn("not_reproduced", reasons)

    def test_unattested_needs_a_replay_that_ran(self):
        row = finding("F-1", attested=False, source="agent_log")
        self.assertIn("unattested", finding_reasons(row, "ok"))
        self.assertIn("unattested", finding_reasons(row, "error"))
        self.assertNotIn("unattested", finding_reasons(row, "skipped"))

    def test_log_only_without_a_replay(self):
        row = finding("F-1", attested=False, source="agent_log")
        self.assertIn("log_only", finding_reasons(row, "skipped"))
        self.assertIn("log_only", finding_reasons(row, None))

    def test_no_reproduction_at_all(self):
        reasons = finding_reasons(finding("F-1", has_repro=False, passed=False, attested=False, source=""), "ok")
        self.assertIn("no_reproduction", reasons)
        self.assertNotIn("not_reproduced", reasons)

    def test_evidence_missing(self):
        self.assertIn("evidence_missing", finding_reasons(finding("F-1", evidence_total=0), "ok"))
        self.assertIn("evidence_missing", finding_reasons(finding("F-1", evidence_missing=1), "ok"))

    def test_vacuity_risk_only_for_attested_passes(self):
        self.assertIn("vacuity_risk", finding_reasons(finding("F-1", state_sensitive=False), "ok"))
        self.assertNotIn(
            "vacuity_risk", finding_reasons(finding("F-1", state_sensitive=False, attested=False), "ok")
        )

    def test_unknown_severity(self):
        self.assertIn("unknown_severity", finding_reasons(finding("F-1", severity="unknown"), "ok"))

    def test_reasons_are_ordered_by_priority(self):
        row = finding("F-1", severity="unknown", passed=False, attested=False, evidence_total=0)
        reasons = finding_reasons(row, "ok")
        self.assertEqual(reasons[0], "not_reproduced")
        self.assertEqual(reasons[-1], "unknown_severity")


class TriageReportTests(unittest.TestCase):
    def test_findings_are_ranked_by_severity_then_reason(self):
        metrics = [
            metric(
                "run-a",
                [
                    finding("F-low", severity="low", passed=False, attested=False),
                    finding("F-crit", severity="critical", passed=False, attested=False),
                    finding("F-high", severity="high", attested=False, source="agent_log"),
                    finding("F-ok"),
                ],
            )
        ]
        payload = triage(metrics, now="2026-01-01T00:00:00Z")
        ids = [row["id"] for row in payload["findings"]]
        # Severity dominates: a high finding proven only by the agent's log is
        # more urgent than a low finding that simply did not reproduce.
        self.assertEqual(ids, ["F-crit", "F-high", "F-low"])
        self.assertEqual(payload["totals"]["needs_review"], 3)
        self.assertEqual(payload["totals"]["findings"], 4)

    def test_runs_with_anomalies_split_integrity(self):
        metrics = [
            metric("run-a", [finding("F-1")], anomalies=["forge_log_test_unknown"]),
            metric("run-b", [finding("F-2")], anomalies=["schema_legacy"]),
        ]
        payload = triage(metrics)
        runs = {row["run_id"]: row for row in payload["runs"]}
        self.assertEqual(runs["run-a"]["integrity"], ["forge_log_test_unknown"])
        self.assertEqual(runs["run-b"]["integrity"], [])
        self.assertEqual(payload["totals"]["runs_with_integrity_anomalies"], 1)

    def test_missing_metrics_are_reported(self):
        payload = triage([metric("run-a", [finding("F-1")])], missing=["run-x"])
        self.assertEqual(payload["totals"]["missing_metrics"], 1)
        self.assertIn("run-x", payload["missing_metrics"])

    def test_markdown_and_csv(self):
        metrics = [
            metric("run-a", [finding("F-1", attested=False, source="agent_log")]),
        ]
        payload = triage(metrics, now="2026-01-01T00:00:00Z")
        markdown = render_markdown(payload)
        self.assertIn("# Audit triage", markdown)
        self.assertIn("## 1. Findings that need a human", markdown)
        self.assertIn("unattested", markdown)

        with tempfile.TemporaryDirectory() as tmp:
            paths = write_outputs(Path(tmp), payload)
            self.assertEqual(set(paths), {"triage_markdown", "triage_csv"})
            self.assertTrue(paths["triage_markdown"].exists())
            with paths["triage_csv"].open(encoding="utf-8", newline="") as handle:
                rows = list(csv.DictReader(handle))
            self.assertEqual(len(rows), 1)
            self.assertEqual(rows[0]["reasons"], "unattested")
            self.assertEqual(rows[0]["run_id"], "run-a")

    def test_console_summary_respects_the_limit(self):
        metrics = [
            metric(
                "run-a",
                [finding("F-{}".format(index), passed=False, attested=False) for index in range(5)],
            )
        ]
        payload = triage(metrics)
        text = "\n".join(console_summary(payload, limit=2))
        self.assertIn("... 3 more", text)


if __name__ == "__main__":
    unittest.main()
