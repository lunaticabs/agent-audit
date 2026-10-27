from __future__ import annotations

import json
import os
import tempfile
import unittest
from pathlib import Path

from audit_stats.aggregate import aggregate, load_metrics, render_markdown, write_outputs
from audit_stats.collect import collect_run, write_run_metrics
from audit_stats.common import (
    coerce_bool,
    normalize_confidence,
    normalize_severity,
    percentile,
    resolve_evidence,
)


def _write(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")


def _write_json(path: Path, payload) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload), encoding="utf-8")


def make_run(root: Path, name: str = "run-1") -> Path:
    """Create a minimal but realistic run directory."""
    run = root / name
    _write(run / "input" / "prompt.txt", "Audit 0x1111111111111111111111111111111111111111 on eth.")
    _write_json(
        run / "input" / "run_meta.json",
        {
            "schema_version": 1,
            "run_id": name,
            "task_id": None,
            "model": "test-model",
            "reasoning_effort": "max",
            "status": "completed",
            "exit_code": 0,
            "started_at": "2026-01-01T00:00:00Z",
            "finished_at": "2026-01-01T00:00:10Z",
            "duration_seconds": 10,
        },
    )
    _write_json(
        run / "reports" / "final_report.json",
        {
            "run_id": name,
            "target": {
                "chain": "eth",
                "address": "0x1111111111111111111111111111111111111111",
                "name": "Foo",
                "verified": True,
            },
            "status": "completed",
            "summary": "One high finding.",
            "findings": [
                {
                    "id": "F-1",
                    "title": "Reentrant withdraw",
                    "severity": "high",
                    "confidence": "confirmed",
                    "evidence": ["sources/Foo.sol", "artifacts/missing.log"],
                    "reproduction": "forge test --match-test test_alpha -vvvv",
                },
                {
                    "id": "F-2",
                    "title": "Odd severity value",
                    "severity": "weird",
                    "evidence": [],
                },
            ],
            "hypotheses": [],
            "limitations": ["Single block"],
            "coverage": {
                "source_review": True,
                "static_analysis": True,
                "fork_testing": True,
                "fuzzing": True,
            },
        },
    )
    _write(run / "reports" / "final_answer.md", "done\n")
    _write(
        run / "test" / "Foo.t.sol",
        "interface IFoo { function beta() external; }\n"
        "contract FooTest {\n"
        "  function test_alpha() public { vm.createSelectFork(vm.envString(\"RPC_URL\"), 1); }\n"
        "  function test_beta() public {\n"
        "    IFoo foo = IFoo(address(0x1111111111111111111111111111111111111111));\n"
        "    foo.beta();\n"
        "  }\n"
        "}\n",
    )
    _write_json(
        run / "sources" / "Foo.abi.json",
        [
            {"type": "function", "name": "alpha", "inputs": [{"type": "uint256"}], "stateMutability": "view"},
            {"type": "function", "name": "beta", "inputs": [], "stateMutability": "nonpayable"},
            {"type": "function", "name": "gamma", "inputs": [], "stateMutability": "view"},
        ],
    )
    _write(
        run / "artifacts" / "trace_alpha.log",
        "[PASS] test_alpha()\n"
        "Traces:\n"
        "  [100] FooTest::test_alpha()\n"
        "    \u251c\u2500 [40] 0x1111111111111111111111111111111111111111::alpha(1) [staticcall]\n",
    )
    _write(
        run / "artifacts" / "forge_test.log",
        "[PASS] test_alpha() (gas: 1234)\n"
        "Suite result: ok. 1 passed; 0 failed; 0 skipped; finished in 1ms\n",
    )
    _write(
        run / "artifacts" / "forge_fuzz.log",
        "[PASS] test_alpha() (gas: 1234)\n"
        "Suite result: ok. 1 passed; 0 failed; 0 skipped; finished in 1ms\n",
    )
    _write_json(
        run / "artifacts" / "slither.json",
        {"success": True, "results": {"detectors": [{"check": "a"}, {"check": "b"}]}},
    )
    _write(run / "artifacts" / "slither_stdout.log", "3 result(s) found\n")
    _write(run / "sources" / "Foo.sol", "contract Foo {}\n")
    return run


def make_legacy_run(root: Path) -> Path:
    run = root / "legacy-1"
    _write_json(
        run / "input" / "run_meta.json",
        {"run_id": "legacy-1", "status": "completed", "exit_code": 0},
    )
    _write_json(
        run / "reports" / "final_report.json",
        {
            "run_id": "legacy-1",
            "target": {"chain": "eth", "address": "0x2222222222222222222222222222222222222222"},
            "summary": {"assessment": "one medium", "confirmed_findings": 1},
            "findings": [
                {"id": "RD-001", "title": "Old shape", "severity": "medium", "confidence": "high"}
            ],
        },
    )
    return run


class NormalizationTests(unittest.TestCase):
    def test_severity_aliases(self):
        self.assertEqual(normalize_severity("Very High"), ("critical", True))
        self.assertEqual(normalize_severity("informational"), ("informational", True))
        self.assertEqual(normalize_severity("INFO"), ("informational", True))
        self.assertEqual(normalize_severity("weird"), ("unknown", False))
        self.assertEqual(normalize_severity(None), ("unknown", False))

    def test_confidence_aliases(self):
        self.assertEqual(normalize_confidence("high"), ("confirmed", True))
        self.assertEqual(normalize_confidence("likely"), ("likely", True))
        self.assertEqual(normalize_confidence(""), ("unknown", False))

    def test_coerce_bool(self):
        self.assertEqual(coerce_bool(True), (True, True))
        self.assertEqual(coerce_bool("false"), (False, True))
        self.assertEqual(coerce_bool(None), (None, True))
        self.assertEqual(coerce_bool("maybe"), (None, False))

    def test_percentile_nearest_rank(self):
        self.assertEqual(percentile([1, 2, 3, 4], 0.5), 2)
        self.assertEqual(percentile([1, 2, 3, 4], 0.9), 4)
        self.assertIsNone(percentile([], 0.5))


class EvidencePathTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.run = self.root / "run"
        (self.run / "sources").mkdir(parents=True)
        (self.run / "sources" / "A.sol").write_text("contract A {}", encoding="utf-8")
        (self.root / "outside.txt").write_text("secret", encoding="utf-8")

    def tearDown(self):
        self.tmp.cleanup()

    def test_resolves_inside_run(self):
        status, path = resolve_evidence(self.run, "sources/A.sol")
        self.assertEqual(status, "ok")
        self.assertTrue(path.exists())

    def test_missing(self):
        self.assertEqual(resolve_evidence(self.run, "artifacts/nope.log")[0], "missing")

    def test_parent_escape_rejected(self):
        self.assertEqual(resolve_evidence(self.run, "../outside.txt")[0], "escape")
        self.assertEqual(resolve_evidence(self.run, "sources/../../outside.txt")[0], "escape")

    def test_absolute_path_rejected(self):
        self.assertEqual(resolve_evidence(self.run, str(self.root / "outside.txt"))[0], "absolute")

    @unittest.skipIf(os.name == "nt", "symlink semantics differ on Windows")
    def test_symlink_escape_rejected(self):
        (self.run / "sources" / "link.txt").symlink_to(self.root / "outside.txt")
        self.assertEqual(resolve_evidence(self.run, "sources/link.txt")[0], "escape")


class CollectTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def test_current_report(self):
        run = make_run(self.root)
        metrics = collect_run(run, now="2026-01-01T00:00:00Z")

        self.assertEqual(metrics["report"]["grade"], "current")
        self.assertEqual(metrics["report"]["findings_total"], 2)
        self.assertEqual(metrics["report"]["by_severity"]["high"], 1)
        self.assertEqual(metrics["report"]["by_severity"]["unknown"], 1)
        self.assertEqual(metrics["report"]["high_plus"], 1)
        self.assertEqual(metrics["report"]["max_severity"], "high")

        # Evidence: one resolved, one missing.
        self.assertEqual(metrics["evidence"]["refs_total"], 2)
        self.assertEqual(metrics["evidence"]["refs_resolved"], 1)
        self.assertEqual(metrics["evidence"]["refs_missing"], 1)
        self.assertEqual(metrics["evidence"]["findings_with_evidence"], 1)

        # Forge logs are merged by test name, not summed.
        self.assertEqual(metrics["fork"]["forge"]["tests_observed"], 1)
        self.assertEqual(metrics["fork"]["forge"]["passed"], 1)
        # The fork is detected from the harness source.
        self.assertTrue(metrics["fork"]["fork_backed"])

        # Only F-1 has a reproduction and it points at a passing test.
        self.assertEqual(metrics["fork"]["findings_repro"]["linked"], 1)
        self.assertEqual(metrics["fork"]["findings_repro"]["passed"], 1)
        self.assertEqual(metrics["fork"]["findings_repro"]["rate"], 0.5)

        coverage = metrics["function_coverage"]
        self.assertTrue(coverage["abi_available"])
        self.assertEqual(coverage["abi_source"], "sources/Foo.abi.json")
        self.assertEqual(coverage["abi_functions"], 3)
        self.assertEqual(coverage["abi_state_changing"], 1)
        self.assertEqual(coverage["covered"], 2)
        self.assertAlmostEqual(coverage["coverage"], 2 / 3)
        self.assertAlmostEqual(coverage["trace_coverage"], 1 / 3)
        self.assertEqual(coverage["uncovered"], ["gamma"])
        self.assertEqual(coverage["harness_only"], ["beta"])
        self.assertEqual(coverage["basis"], "trace+harness")
        self.assertEqual(coverage["state_changing_coverage"], 1.0)
        self.assertIsNotNone(metrics["run"]["prompt_sha256"])

        codes = {entry["code"] for entry in metrics["anomalies"]}
        self.assertIn("evidence_missing", codes)
        self.assertIn("finding_without_evidence", codes)
        self.assertIn("finding_severity_unknown", codes)
        self.assertIn("finding_confidence_unknown", codes)
        self.assertIn("slither_result_mismatch", codes)
        self.assertIn("declared_coverage_overclaim", codes)
        self.assertEqual(metrics["coverage"]["overclaims"], ["fuzzing"])

    def test_legacy_report_is_graceful(self):
        run = make_legacy_run(self.root)
        metrics = collect_run(run, now="2026-01-01T00:00:00Z")
        self.assertEqual(metrics["report"]["grade"], "legacy")
        self.assertEqual(metrics["report"]["findings_total"], 1)
        self.assertEqual(metrics["report"]["by_severity"]["medium"], 1)
        codes = {entry["code"] for entry in metrics["anomalies"]}
        self.assertIn("schema_legacy", codes)

    def test_missing_report(self):
        run = self.root / "empty-run"
        (run / "input").mkdir(parents=True)
        metrics = collect_run(run, now="2026-01-01T00:00:00Z")
        self.assertEqual(metrics["report"]["grade"], "invalid")
        codes = {entry["code"] for entry in metrics["anomalies"]}
        self.assertIn("report_missing", codes)
        self.assertIn("run_meta_missing", codes)


def _metric(
    run_id: str,
    *,
    findings: int = 0,
    high_plus: int = 0,
    severity: str = "informational",
    repro_passed: int = 0,
    status: str = "completed",
    grade: str = "current",
    duration: float = 60.0,
    chain: str = "eth",
    model: str = "m1",
    verified=True,
    anomalies=(),
    evidence_missing: int = 0,
    fork_backed: bool = False,
) -> dict:
    severity_map = {"critical": 0, "high": 0, "medium": 0, "low": 0, "informational": 0, "unknown": 0}
    if findings:
        severity_map[severity] = findings
    return {
        "stats_schema_version": 2,
        "run_id": run_id,
        "run": {
            "status": status,
            "duration_seconds": duration,
            "model": model,
            "reasoning_effort": "max",
            "started_at": "2026-01-01T00:00:00Z",
        },
        "target": {"chain": chain, "address": "0x" + run_id, "verified": verified},
        "report": {
            "present": True,
            "parsed": grade != "invalid",
            "grade": grade,
            "declared_status": "completed",
            "findings_total": findings,
            "high_plus": high_plus,
            "max_severity": severity if findings else "unknown",
            "by_severity": severity_map,
            "by_confidence": {},
        },
        "evidence": {
            "refs_total": findings,
            "refs_resolved": max(0, findings - evidence_missing),
            "refs_missing": evidence_missing,
            "refs_outside_run": 0,
        },
        "fork": {
            "tests": ["test_a"] if fork_backed else [],
            "tests_fuzz": [],
            "fork_backed": fork_backed,
            "forge": {"passed": 1 if fork_backed else 0, "failed": 0, "fuzz_runs": 0},
            "findings_repro": {
                "linked": repro_passed,
                "passed": repro_passed,
                "rate": (repro_passed / findings) if findings else None,
            },
        },
        "coverage": {
            "declared": {},
            "observed": {},
            "overclaims": [],
            "underclaims": [],
        },
        "tooling": {},
        "volume": {},
        "findings": [
            {
                "id": "F-{}".format(index),
                "title": "t",
                "severity": severity,
                "confidence": "confirmed",
                "contract": "0x",
                "function": "",
                "has_reproduction": True,
                "repro_linked": ["test_a"] if index <= repro_passed else [],
                "repro_passed": ["test_a"] if index <= repro_passed else [],
                "evidence_total": 1,
                "evidence_missing": 1 if evidence_missing and index == 1 else 0,
                "severity_ok": True,
                "confidence_ok": True,
            }
            for index in range(1, findings + 1)
        ],
        "anomalies": [{"code": code, "detail": ""} for code in anomalies],
    }


class AggregateTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def test_rates_use_evaluable_runs(self):
        metrics = [
            _metric("a", findings=2, high_plus=1, severity="high", repro_passed=1, fork_backed=True),
            _metric("b"),
            _metric("c", findings=1, status="failed"),
        ]
        summary = aggregate(metrics, expected=3, generated_at="2026-01-01T00:00:00Z")
        self.assertEqual(summary["runs"]["total"], 3)
        self.assertEqual(summary["runs"]["evaluable"], 2)
        self.assertEqual(summary["findings"]["total"], 2)
        self.assertEqual(summary["detection"]["runs_high_plus"], 1)
        self.assertEqual(summary["detection"]["rate_high_plus"], 0.5)
        self.assertEqual(summary["detection"]["rate_any"], 0.5)
        self.assertEqual(summary["fork"]["repro_passed"], 1)
        self.assertEqual(summary["fork"]["repro_rate"], 0.5)
        self.assertEqual(summary["fork"]["runs_fork_backed"], 1)
        self.assertEqual(summary["round"]["completion_rate"], 1.0)

    def test_funnel_and_strata(self):
        metrics = [
            _metric("a", findings=1, high_plus=1, severity="critical", chain="eth"),
            _metric("b", chain="base", verified=False),
        ]
        summary = aggregate(metrics)
        funnel = {stage["stage"]: stage["count"] for stage in summary["funnel"]}
        self.assertEqual(funnel["metrics_present"], 2)
        self.assertEqual(funnel["evaluable"], 2)
        self.assertEqual(funnel["any_finding"], 1)
        chains = {row["key"]: row for row in summary["strata"]["by_chain"]}
        self.assertEqual(chains["eth"]["findings"], 1)
        self.assertEqual(chains["base"]["findings"], 0)
        verified = {row["key"]: row for row in summary["strata"]["by_verified"]}
        self.assertEqual(verified["unverified"]["runs"], 1)

    def test_anomalies_are_counted(self):
        metrics = [
            _metric("a", anomalies=("evidence_missing", "schema_legacy")),
            _metric("b", anomalies=("evidence_missing",)),
        ]
        summary = aggregate(metrics)
        self.assertEqual(summary["anomalies"]["total"], 3)
        self.assertEqual(summary["anomalies"]["by_code"]["evidence_missing"], 2)
        self.assertEqual(summary["anomalies"]["runs_affected"], 2)

    def test_load_metrics_backfill(self):
        make_run(self.root, "run-a")
        make_legacy_run(self.root)
        # No run has stored metrics yet.
        loaded = load_metrics(self.root, backfill=False)
        self.assertEqual(len(loaded["metrics"]), 0)
        self.assertEqual(len(loaded["missing"]), 2)

        run_a = self.root / "run-a"
        write_run_metrics(run_a, collect_run(run_a, now="2026-01-01T00:00:00Z"))
        loaded = load_metrics(self.root, backfill=False)
        self.assertEqual(len(loaded["metrics"]), 1)
        self.assertEqual(loaded["missing"], ["legacy-1"])

        loaded = load_metrics(self.root, backfill=True)
        self.assertEqual(len(loaded["metrics"]), 2)
        self.assertEqual(loaded["missing"], [])

    def test_render_and_write_outputs(self):
        metrics = [
            _metric("a", findings=1, high_plus=1, severity="high", repro_passed=1, fork_backed=True)
        ]
        summary = aggregate(metrics, expected=1, generated_at="2026-01-01T00:00:00Z")
        markdown = render_markdown(summary, metrics)
        self.assertIn("# Audit round statistics", markdown)
        self.assertIn("100.0%", markdown)
        self.assertIn("## 3. Fork reproduction", markdown)

        out = self.root / "out"
        paths = write_outputs(out, summary, metrics)
        self.assertEqual(
            set(paths), {"summary", "report", "runs_csv", "findings_csv", "stability_csv"}
        )
        self.assertTrue((out / "REPORT.md").exists())
        self.assertTrue((out / "summary.json").exists())
        header = (out / "runs.csv").read_text(encoding="utf-8").splitlines()[0]
        self.assertIn("run_id", header)
        self.assertIn("repro_passed", header)
        findings_header = (out / "findings.csv").read_text(encoding="utf-8").splitlines()[0]
        self.assertIn("evidence_missing", findings_header)


def _finding(finding_id: str, title: str, severity: str) -> dict:
    return {"id": finding_id, "title": title, "severity": severity}


def _run(
    run_id: str,
    address: str,
    findings,
    model="m1",
    image=None,
    prompt="p1",
    started_at="2026-01-01T00:00:00Z",
) -> dict:
    return {
        "run_id": run_id,
        "run": {
            "model": model,
            "image": image,
            "prompt_sha256": prompt,
            "started_at": started_at,
        },
        "target": {"chain": "eth", "address": address},
        "report": {"max_severity": findings[0]["severity"] if findings else "unknown"},
        "findings": findings,
    }


class StabilityTests(unittest.TestCase):
    def test_title_normalization_is_order_insensitive(self):
        from audit_stats.stability import jaccard, title_tokens

        left = title_tokens("Reentrancy in the withdraw path")
        right = title_tokens("withdraw path reentrancy")
        self.assertEqual(jaccard(left, right), 1.0)
        self.assertEqual(jaccard(title_tokens("Missing zero-address check"), title_tokens("Approve race")), 0.0)

    def test_compare_pair_overlap_and_severity_delta(self):
        from audit_stats.stability import compare_pair

        left = _run(
            "run-a",
            "0xabc",
            [
                _finding("A1", "Reentrancy in withdraw path", "high"),
                _finding("A2", "Missing zero-address check", "informational"),
            ],
        )
        right = _run(
            "run-b",
            "0xabc",
            [
                _finding("B1", "Reentrancy in withdraw path", "medium"),
                _finding("B2", "Standard ERC20 approve race", "low"),
            ],
        )
        pair = compare_pair(left, right)
        self.assertEqual(pair["matched"], 1)
        self.assertEqual(pair["union"], 3)
        self.assertAlmostEqual(pair["overlap"], 1 / 3)
        self.assertEqual(pair["severity_deltas"], [-1])
        self.assertEqual(pair["severity_delta_max_abs"], 1)
        self.assertEqual(pair["only_in_a"], 1)
        self.assertEqual(pair["only_in_b"], 1)
        self.assertTrue(pair["like_for_like"])

    def test_stability_summary_groups_by_target(self):
        from audit_stats.stability import stability_summary

        left = _run("run-a", "0xabc", [_finding("A1", "Reentrancy in withdraw", "high")])
        right = _run("run-b", "0xabc", [_finding("B1", "Reentrancy in withdraw", "low")])
        other = _run("run-c", "0xdef", [_finding("C1", "Reentrancy in withdraw", "high")])

        summary = stability_summary([left, right, other])
        self.assertEqual(summary["targets_with_repeat"], 1)
        self.assertEqual(summary["pairs"], 1)
        self.assertEqual(summary["mean_overlap"], 1.0)
        self.assertEqual(summary["severity_delta_distribution"], {"-2": 1})
        self.assertEqual(summary["severe_drift_pairs"], 1)
        self.assertEqual(summary["worst_targets"][0]["address"], "0xabc")

    def test_empty_pair_counts_as_agreement(self):
        from audit_stats.stability import compare_pair

        pair = compare_pair(_run("a", "0xabc", []), _run("b", "0xabc", []))
        self.assertTrue(pair["empty_pair"])
        self.assertEqual(pair["overlap"], 1.0)
        self.assertIsNone(pair["mean_best_jaccard"])

    def test_delta_is_newer_minus_older(self):
        from audit_stats.stability import stability_summary

        older = _run(
            "zz-old",
            "0xabc",
            [_finding("A1", "Reentrancy in withdraw", "high")],
            started_at="2026-01-01T00:00:00Z",
        )
        newer = _run(
            "aa-new",
            "0xabc",
            [_finding("B1", "Reentrancy in withdraw", "low")],
            started_at="2026-02-01T00:00:00Z",
        )
        summary = stability_summary([older, newer])
        self.assertEqual(summary["severity_delta_distribution"], {"-2": 1})

    def test_changed_conditions_are_separated(self):
        from audit_stats.stability import stability_summary

        left = _run("run-a", "0xabc", [_finding("A1", "Reentrancy in withdraw", "high")], model="m1")
        right = _run("run-b", "0xabc", [_finding("B1", "Reentrancy in withdraw", "high")], model="m2")
        summary = stability_summary([left, right])
        self.assertEqual(summary["like_for_like"]["pairs"], 0)
        self.assertEqual(summary["changed_conditions"]["pairs"], 1)


class FunctionCoverageAggregateTests(unittest.TestCase):
    def test_coverage_is_summed_across_runs(self):
        metrics = [
            _metric("a", findings=1, high_plus=1, severity="high"),
            _metric("b", findings=1, high_plus=0, severity="low"),
        ]
        metrics[0]["function_coverage"] = {
            "abi_available": True,
            "abi_functions": 10,
            "covered": 5,
            "coverage": 0.5,
            "abi_state_changing": 4,
            "state_changing_covered": 2,
            "trace_coverage": 0.3,
            "trace_covered": 3,
            "basis": "trace+harness",
            "fallback_exercised": False,
            "uncovered": ["f1", "f2"],
        }
        metrics[1]["function_coverage"] = {
            "abi_available": True,
            "abi_functions": 10,
            "covered": 10,
            "coverage": 1.0,
            "abi_state_changing": 6,
            "state_changing_covered": 6,
            "trace_coverage": 0.8,
            "trace_covered": 8,
            "basis": "trace",
            "fallback_exercised": True,
            "uncovered": [],
        }
        summary = aggregate(metrics)
        coverage = summary["function_coverage"]
        self.assertEqual(coverage["abi_functions_total"], 20)
        self.assertEqual(coverage["covered_total"], 15)
        self.assertEqual(coverage["coverage"], 0.75)
        self.assertEqual(coverage["state_changing_coverage"], 0.8)
        self.assertEqual(coverage["runs_fully_covered"], 1)
        self.assertEqual(coverage["uncovered_functions"][0], {"function": "f1", "runs": 1})


if __name__ == "__main__":
    unittest.main()
