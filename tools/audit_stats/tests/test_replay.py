from __future__ import annotations

import json
import shutil
import stat
import tempfile
import unittest
from pathlib import Path

from audit_stats.replay import (
    discover_foundry_roots,
    linked_tests,
    load_replay,
    read_pinned_block,
    run_replay,
    write_replay,
)


def _write(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")


def _write_json(path: Path, payload) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload), encoding="utf-8")


#: Stands in for the real forge binary. It reports PASS at the pinned block and
#: FAIL at the earlier block, which is exactly the state-sensitive case.
FORGE_SCRIPT = """#!/usr/bin/env bash
set -u
block=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "--fork-block-number" ]; then block="$arg"; fi
  prev="$arg"
done
echo "Ran 1 test for test/Foo.t.sol:FooTest"
if [ "$block" = "900" ]; then
  echo "[FAIL] test_alpha()"
  echo "Suite result: failed. 0 passed; 1 failed; 0 skipped; finished in 1ms"
  exit 1
fi
echo "[PASS] test_alpha() (gas: 1234)"
echo "Suite result: ok. 1 passed; 0 failed; 0 skipped; finished in 1ms"
"""


def make_project_run(root: Path, name: str = "run-replay") -> Path:
    run = root / name
    _write(run / "input" / "prompt.txt", "Audit 0x1111111111111111111111111111111111111111 on eth.")
    _write_json(
        run / "reports" / "final_report.json",
        {
            "run_id": name,
            "target": {
                "chain": "eth",
                "address": "0x1111111111111111111111111111111111111111",
                "verified": True,
            },
            "status": "completed",
            "summary": "one high finding",
            "findings": [
                {
                    "id": "F-1",
                    "title": "Reentrant withdraw",
                    "severity": "high",
                    "confidence": "confirmed",
                    "evidence": [],
                    "reproduction": "forge test --match-test test_alpha -vvvv",
                }
            ],
            "hypotheses": [],
            "limitations": [],
            "coverage": {},
        },
    )
    _write(run / "proj" / "foundry.toml", '[profile.default]\nsrc = "src"\n')
    _write(
        run / "proj" / "test" / "Foo.t.sol",
        "contract FooTest {\n"
        '  function setUp() public { vm.createSelectFork(vm.envString("RPC_URL")); }\n'
        "  function test_alpha() public { assertTrue(true); }\n"
        "}\n",
    )
    (run / "artifacts").mkdir(parents=True, exist_ok=True)
    return run


#: A failing test exits 1, which is a valid replay result and must not be
#: reported as a replay error.
FAILING_FORGE = """#!/usr/bin/env bash
echo "[FAIL] test_alpha()"
echo "Suite result: failed. 0 passed; 1 failed; 0 skipped; finished in 1ms"
exit 1
"""


class ReplayModuleTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def _forge(self) -> Path:
        path = self.root / "bin" / "forge"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(FORGE_SCRIPT, encoding="utf-8")
        path.chmod(path.stat().st_mode | stat.S_IEXEC)
        return path

    def test_two_block_replay_records_state_sensitivity(self):
        run = make_project_run(self.root)
        payload = run_replay(
            run,
            rpc_url="http://localhost:8545",
            block=1000,
            delta=100,
            budget=60_000,
            forge_bin=str(self._forge()),
            now="2026-01-01T00:00:00Z",
        )

        self.assertEqual(payload["status"], "ok")
        self.assertEqual(payload["block"], 1000)
        self.assertEqual(payload["delta_block"], 900)

        row = payload["tests"]["test_alpha"]
        self.assertEqual(row["status"], "PASS")
        self.assertEqual(row["delta_status"], "FAIL")
        self.assertTrue(row["state_sensitive"])
        self.assertEqual(row["sha256"] and len(row["sha256"]), 64)

        log = run / row["log"]
        self.assertTrue(log.exists())
        self.assertTrue(log.read_text(encoding="utf-8").startswith("[agent-audit replay]"))
        self.assertIn("--fork-block-number 1000", log.read_text(encoding="utf-8"))

    def test_project_at_the_run_root_lands_under_root(self):
        run = make_project_run(self.root)
        foundry = (run / "proj" / "foundry.toml").read_text(encoding="utf-8")
        harness = (run / "proj" / "test" / "Foo.t.sol").read_text(encoding="utf-8")
        (run / "foundry.toml").write_text(foundry, encoding="utf-8")
        _write(run / "test" / "Foo.t.sol", harness)
        shutil.rmtree(run / "proj")

        payload = run_replay(
            run,
            rpc_url="http://localhost:8545",
            block=1000,
            delta=100,
            budget=60_000,
            forge_bin=str(self._forge()),
        )
        self.assertEqual(payload["status"], "ok")
        self.assertEqual(payload["tests"]["test_alpha"]["log"], "artifacts/replay/root/block-1000.log")

    def test_rpc_credentials_are_redacted_in_the_log(self):
        run = make_project_run(self.root)
        payload = run_replay(
            run,
            rpc_url="https://mainnet.example.com/v3/SECRETKEY123",
            block=1000,
            delta=0,
            budget=60_000,
            forge_bin=str(self._forge()),
        )
        log = (run / payload["tests"]["test_alpha"]["log"]).read_text(encoding="utf-8")
        self.assertNotIn("SECRETKEY123", log)
        self.assertIn("/redacted", log)

    def test_replay_is_persisted_and_reloaded(self):
        run = make_project_run(self.root)
        payload = run_replay(
            run,
            rpc_url="http://localhost:8545",
            block=1000,
            delta=100,
            budget=60_000,
            forge_bin=str(self._forge()),
        )
        path = write_replay(run, payload)
        self.assertTrue(path.exists())
        loaded = load_replay(run)
        self.assertIsNotNone(loaded)
        self.assertEqual(loaded["status"], "ok")
        self.assertIn("test_alpha", loaded["tests"])

    def test_skips_without_rpc(self):
        run = make_project_run(self.root)
        payload = run_replay(
            run,
            rpc_url="",
            block=1,
            delta=1,
            forge_bin=str(self._forge()),
        )
        self.assertEqual(payload["status"], "skipped")
        self.assertIn("RPC_URL", payload["reason"])
        self.assertEqual(payload["tests"], {})

    def test_skips_when_no_finding_names_a_test(self):
        run = make_project_run(self.root)
        _write_json(
            run / "reports" / "final_report.json",
            {
                "run_id": "run-replay",
                "target": {"address": "0x1111111111111111111111111111111111111111"},
                "status": "completed",
                "summary": "x",
                "findings": [{"id": "F-1", "title": "t", "severity": "high"}],
                "hypotheses": [],
                "limitations": [],
                "coverage": {},
            },
        )
        payload = run_replay(
            run, rpc_url="http://localhost:8545", block=1, delta=1, forge_bin=str(self._forge())
        )
        self.assertEqual(payload["status"], "skipped")
        self.assertIn("no finding names a test", payload["reason"])

    def test_missing_claimed_test_skips_the_root(self):
        run = make_project_run(self.root)
        report = json.loads((run / "reports" / "final_report.json").read_text(encoding="utf-8"))
        report["findings"][0]["reproduction"] = "forge test --match-test test_absent"
        _write_json(run / "reports" / "final_report.json", report)

        payload = run_replay(
            run, rpc_url="http://localhost:8545", block=1, delta=1, forge_bin=str(self._forge())
        )
        self.assertEqual(payload["status"], "skipped")
        self.assertIn("not declared under any foundry project", payload["reason"])

    def test_failing_test_is_a_result_not_an_error(self):
        run = make_project_run(self.root)
        failing = self.root / "bin" / "forge-fail"
        failing.parent.mkdir(parents=True, exist_ok=True)
        failing.write_text(FAILING_FORGE, encoding="utf-8")
        failing.chmod(failing.stat().st_mode | stat.S_IEXEC)

        payload = run_replay(
            run,
            rpc_url="http://localhost:8545",
            block=1000,
            delta=0,
            budget=60_000,
            forge_bin=str(failing),
        )
        self.assertEqual(payload["status"], "ok")
        self.assertEqual(payload["tests"]["test_alpha"]["status"], "FAIL")
        self.assertIsNone(payload["tests"]["test_alpha"]["delta_status"])
        self.assertIsNone(payload["tests"]["test_alpha"]["state_sensitive"])
        self.assertEqual(payload["roots"][0]["error"], None)

    def test_pinned_block_file_is_used(self):
        run = make_project_run(self.root)
        _write_json(run / "artifacts" / "pinned_block.json", {"block_number": "123"})
        self.assertEqual(read_pinned_block(run), 123)

    def test_pinned_block_accepts_a_raw_rpc_block(self):
        run = make_project_run(self.root)
        _write_json(
            run / "artifacts" / "pinned_block.json",
            {"schema_version": 1, "success": True, "data": {"number": "0x1910921"}},
        )
        self.assertEqual(read_pinned_block(run), 0x1910921)

    def test_discover_roots_skips_dependencies(self):
        run = make_project_run(self.root)
        _write(run / "proj" / "lib" / "dep" / "foundry.toml", "")
        _write(run / "proj" / "node_modules" / "x" / "foundry.toml", "")
        roots = discover_foundry_roots(run)
        self.assertEqual([path.name for path in roots], ["proj"])

    def test_apostrophe_in_a_comment_does_not_hide_later_tests(self):
        run = make_project_run(self.root)
        _write(
            run / "proj" / "test" / "Foo.t.sol",
            "contract FooTest {\n"
            "  function test_alpha() public {\n"
            "    // Bob front-runs Alice's reduction to 0.5.\n"
            "    assertTrue(true);\n"
            "  }\n"
            "  function test_beta() public { assertTrue(true); }\n"
            "}\n",
        )
        from audit_stats.forge import test_names

        names = test_names((run / "proj" / "test" / "Foo.t.sol").read_text(encoding="utf-8"))
        self.assertEqual(names, ["test_alpha", "test_beta"])

    def test_linked_tests_reads_the_report(self):
        run = make_project_run(self.root)
        report = json.loads((run / "reports" / "final_report.json").read_text(encoding="utf-8"))
        self.assertEqual(linked_tests(report), ["test_alpha"])

    def test_linked_tests_accepts_a_list(self):
        report = {
            "findings": [
                {"reproduction": ["forge test --match-test test_a", "then test_b"]},
            ]
        }
        self.assertEqual(linked_tests(report), ["test_a", "test_b"])


if __name__ == "__main__":
    unittest.main()
