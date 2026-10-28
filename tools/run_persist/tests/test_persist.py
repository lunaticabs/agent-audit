from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path
from typing import Any, Dict, List

from run_persist import (
    PersistConfig,
    build_meta_document,
    build_run_payload,
    collect_run_materials,
    write_run,
)
from run_persist.persist import FileMaterial, _iter_chunks, _sanitize_bson_value, resolve_status


def _write_json(path: Path, payload: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload), encoding="utf-8")


def _write_text(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def make_run(root: Path, *, with_metrics: bool = True) -> Path:
    run = root / "run-1"
    for directory in (
        "input",
        "reports",
        "artifacts",
        "sources",
        "stats",
        "logs",
        "test",
        "harness",
        "out",
        "batch_material",
    ):
        (run / directory).mkdir(parents=True, exist_ok=True)

    _write_text(run / "input" / "prompt.txt", "Audit 0x1234567890abcdef1234567890abcdef12345678 on eth.\n")
    _write_json(
        run / "input" / "request.json",
        {
            "schema_version": 1,
            "run_id": "run-1",
            "created_at": "2026-10-28T01:00:00Z",
            "prompt": "Audit 0x1234567890abcdef1234567890abcdef12345678 on eth.",
        },
    )
    _write_json(
        run / "input" / "run_meta.json",
        {
            "schema_version": 1,
            "run_id": "run-1",
            "status": "completed",
            "exit_code": 0,
            "started_at": "2026-10-28T01:00:00Z",
            "finished_at": "2026-10-28T01:10:00Z",
            "duration_seconds": 600,
        },
    )
    _write_json(
        run / "reports" / "final_report.json",
        {
            "run_id": "run-1",
            "status": "completed",
            "target": {
                "address": "0x1234567890abcdef1234567890abcdef12345678",
                "chain": "eth",
                "block_number": "26000000",
            },
            "summary": "test",
            "findings": [],
            "hypotheses": [],
            "limitations": [],
            "coverage": {},
        },
    )
    _write_text(run / "reports" / "final_answer.md", "answer\n")
    _write_json(run / "artifacts" / "evidence.json", {"ok": True})
    _write_text(run / "artifacts" / "trace.log", "trace\n")
    _write_json(run / "sources" / "Main.sol.json", {"source": "contract Main {}"})
    _write_text(run / "sources" / "lib" / "Keep.sol", "contract Keep {}\n")
    _write_text(run / "test" / "Run.t.sol", "contract RunTest {}\n")
    _write_text(run / "harness" / "foundry.toml", "[profile.default]\n")
    _write_text(run / "harness" / "test" / "Fork.t.sol", "contract ForkTest {}\n")
    _write_text(run / "harness" / "cache" / "cache.json", '{"cache": true}\n')
    _write_text(run / "harness" / "out" / "Fork.t.sol" / "Fork.json", '{"artifact": true}\n')
    _write_text(run / "foundry.toml", "[profile.default]\n")
    _write_text(run / "out" / "Main.bin", "deployment output\n")
    _write_text(run / "logs" / "agent.stdout.log", "log\n")
    _write_text(run / "batch_material" / "batch.json", '{"batch": true}\n')
    _write_json(run / "stats" / "metrics.json", {"sentinel": "must not be archived"})

    if with_metrics:
        _write_json(
            run / "stats" / "metrics.json",
            {
                "stats_schema_version": 2,
                "run_id": "run-1",
                "run": {"status": "completed", "exit_code": 0},
                "target": {
                    "address": "0x1234567890abcdef1234567890abcdef12345678",
                    "chain": "eth",
                },
                "report": {"findings_total": 0},
                "volume": {"artifacts_files": 2},
            },
        )
    else:
        (run / "stats" / "metrics.json").unlink()
    return run


class CollectMaterialsTests(unittest.TestCase):
    def test_only_per_run_material_is_archived(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            run = make_run(Path(tmp))
            material_set = collect_run_materials(
                run,
                max_inline_file_bytes=1024 * 1024,
                block_number="26000000",
            )
            paths = {material.doc["rel_path"] for material in material_set.materials}

            self.assertIn("input/run_meta.json", paths)
            self.assertIn("reports/final_report.json", paths)
            self.assertIn("artifacts/evidence.json", paths)
            self.assertIn("sources/Main.sol.json", paths)
            self.assertIn("sources/lib/Keep.sol", paths)
            self.assertIn("test/Run.t.sol", paths)
            self.assertIn("harness/foundry.toml", paths)
            self.assertIn("harness/test/Fork.t.sol", paths)
            self.assertIn("foundry.toml", paths)
            self.assertNotIn("harness/cache/cache.json", paths)
            self.assertNotIn("harness/out/Fork.t.sol/Fork.json", paths)
            self.assertNotIn("stats/metrics.json", paths)
            self.assertNotIn("logs/agent.stdout.log", paths)
            self.assertNotIn("out/Main.bin", paths)
            self.assertNotIn("batch_material/batch.json", paths)

            report_doc = next(
                material.doc
                for material in material_set.materials
                if material.doc["rel_path"] == "reports/final_report.json"
            )
            self.assertEqual(report_doc["kind"], "json")
            self.assertEqual(report_doc["block_number"], "26000000")
            self.assertIn("created_at", report_doc)
            self.assertEqual(material_set.skipped_count, 0)

    def test_oversized_file_is_skipped_not_fatal(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            run = make_run(Path(tmp))
            _write_text(run / "artifacts" / "huge.log", "x" * 5000)
            material_set = collect_run_materials(run, max_inline_file_bytes=4096)
            paths = {material.doc["rel_path"] for material in material_set.materials}

            self.assertIn("artifacts/evidence.json", paths)
            self.assertNotIn("artifacts/huge.log", paths)
            self.assertEqual(material_set.skipped_count, 1)
            self.assertEqual(material_set.skipped_samples, ["artifacts/huge.log"])

    def test_unsafe_json_keys_fall_back_to_text(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            run = make_run(Path(tmp))
            _write_text(run / "artifacts" / "dotted.json", '{"a.b": 1}')
            _write_text(run / "artifacts" / "huge_int.json", '{"n": 9223372036854775808}')
            material_set = collect_run_materials(run, max_inline_file_bytes=1024 * 1024)
            docs = {material.doc["rel_path"]: material.doc for material in material_set.materials}

            self.assertEqual(docs["artifacts/dotted.json"]["kind"], "text")
            self.assertIn("a.b", docs["artifacts/dotted.json"]["content_text"])
            self.assertNotIn("content_json", docs["artifacts/dotted.json"])
            self.assertEqual(docs["artifacts/huge_int.json"]["kind"], "text")

    def test_json_document_has_expected_shape(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            run = make_run(Path(tmp))
            material_set = collect_run_materials(run, max_inline_file_bytes=1024 * 1024)
            doc = next(
                material.doc
                for material in material_set.materials
                if material.doc["rel_path"] == "artifacts/evidence.json"
            )
            self.assertEqual(doc["_id"], "run-1:artifacts/evidence.json")
            self.assertEqual(doc["run_id"], "run-1")
            self.assertEqual(doc["kind"], "json")
            self.assertEqual(doc["content_json"], {"ok": True})
            self.assertEqual(len(doc["sha256"]), 64)


class MetaDocumentTests(unittest.TestCase):
    def test_flattens_target_and_embeds_stats(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            run = make_run(Path(tmp))
            material_set = collect_run_materials(run, max_inline_file_bytes=1024 * 1024)
            metrics = json.loads((run / "stats" / "metrics.json").read_text())
            run_meta = json.loads((run / "input" / "run_meta.json").read_text())
            report = json.loads((run / "reports" / "final_report.json").read_text())

            doc = build_meta_document(
                run,
                run_meta=run_meta,
                request={"prompt": "ignored"},
                report=report,
                metrics=metrics,
                material_set=material_set,
            )

            self.assertEqual(doc["schema_version"], 2)
            self.assertEqual(doc["status"], "succeeded")
            self.assertEqual(
                doc["target_address"], "0x1234567890abcdef1234567890abcdef12345678"
            )
            self.assertEqual(doc["target_chain"], "eth")
            self.assertNotIn("target", doc)
            self.assertEqual(doc["stats"], metrics)
            self.assertEqual(doc["block_number"], "26000000")
            self.assertTrue(doc["has_final_report"])
            self.assertEqual(doc["file_count"], material_set.file_count)
            self.assertEqual(doc["total_size_bytes"], material_set.total_size_bytes)
            self.assertEqual(doc["created_at"].isoformat(), "2026-10-28T01:00:00+00:00")
            self.assertEqual(doc["finished_at"].isoformat(), "2026-10-28T01:10:00+00:00")
            self.assertEqual(doc["duration_seconds"], 600.0)

    def test_missing_metrics_is_explicit(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            run = make_run(Path(tmp), with_metrics=False)
            material_set = collect_run_materials(run)
            payload = build_run_payload(run)
            doc = build_meta_document(
                run,
                run_meta={"status": "completed", "exit_code": 0},
                request={},
                report={},
                metrics=None,
                material_set=material_set,
                stats_error="stats/metrics.json: missing",
            )

            self.assertIsNone(doc["stats"])
            self.assertEqual(doc["stats_error"], "stats/metrics.json: missing")
            self.assertIsNone(payload.meta_doc["stats"])

    def test_prompt_fallback_for_failed_run(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            run = make_run(Path(tmp), with_metrics=False)
            (run / "reports" / "final_report.json").unlink()
            (run / "input" / "run_meta.json").write_text(
                json.dumps(
                    {
                        "status": "failed",
                        "exit_code": 1,
                        "started_at": "2026-10-28T01:00:00Z",
                        "finished_at": "2026-10-28T01:00:05Z",
                    }
                ),
                encoding="utf-8",
            )
            payload = build_run_payload(run)
            doc = payload.meta_doc

            self.assertEqual(doc["status"], "failed")
            self.assertEqual(
                doc["target_address"], "0x1234567890abcdef1234567890abcdef12345678"
            )
            self.assertEqual(doc["target_chain"], "eth")
            self.assertFalse(doc["has_final_report"])

    def test_sanitizes_stats_keys_and_oversized_integers(self) -> None:
        sanitized = _sanitize_bson_value(
            {"a.b": 1, "$x": {"nested.key": 2}, "big": 2**70}
        )
        self.assertEqual(
            sanitized,
            {"a_b": 1, "_x": {"nested_key": 2}, "big": str(2**70)},
        )

    def test_chunks_respect_count_and_byte_budget(self) -> None:
        items = [FileMaterial(doc={"_id": str(index)}, size_bytes=10) for index in range(7)]
        chunks = list(_iter_chunks(items, 100, max_bytes=25))
        self.assertEqual([len(chunk) for chunk in chunks], [2, 2, 2, 1])
        self.assertTrue(all(sum(item.size_bytes for item in chunk) <= 25 for chunk in chunks))

    def test_status_mapping(self) -> None:
        self.assertEqual(resolve_status({"status": "completed"}), "succeeded")
        self.assertEqual(resolve_status({"status": "failed"}), "failed")
        self.assertEqual(resolve_status({"status": "error"}), "failed")
        self.assertEqual(resolve_status({"status": "running"}), "running")
        self.assertEqual(resolve_status({"exit_code": 0}), "succeeded")
        self.assertEqual(resolve_status({"exit_code": 1}), "failed")


class _FakeResult:
    upserted_count = 1
    modified_count = 1


class _FakeCollection:
    def __init__(self) -> None:
        self.bulk_calls: List[Any] = []
        self.replace_calls: List[Any] = []

    def bulk_write(self, operations: Any, ordered: bool = True) -> _FakeResult:
        self.bulk_calls.append((list(operations), ordered))
        return _FakeResult()

    def replace_one(self, filter: Dict[str, Any], doc: Dict[str, Any], upsert: bool = False) -> _FakeResult:
        self.replace_calls.append((filter, doc, upsert))
        return _FakeResult()


class _FakeDB:
    def __init__(self) -> None:
        self.collections: Dict[str, _FakeCollection] = {}

    def __getitem__(self, name: str) -> _FakeCollection:
        return self.collections.setdefault(name, _FakeCollection())


class _FakeClient:
    def __init__(self) -> None:
        self.databases: Dict[str, _FakeDB] = {}

    def __getitem__(self, name: str) -> _FakeDB:
        return self.databases.setdefault(name, _FakeDB())

    def close(self) -> None:
        pass


class WriteRunTests(unittest.TestCase):
    def test_one_bulk_file_write_and_one_meta_replace(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            run = make_run(Path(tmp))
            payload = build_run_payload(run)
            client = _FakeClient()
            config = PersistConfig(
                uri="mongodb://unused",
                db="Test",
                meta_collection="runs_meta",
                files_collection="runs_files",
                chunk_size=3,
            )

            written = write_run(
                client,
                config,
                run_id=payload.run_id,
                materials=payload.materials,
                meta_doc=payload.meta_doc,
            )

            files = client["Test"]["runs_files"]
            meta = client["Test"]["runs_meta"]
            self.assertEqual(written, len(payload.materials))
            self.assertEqual(len(files.bulk_calls), (len(payload.materials) + 2) // 3)
            self.assertEqual(
                sum(len(operations) for operations, _ in files.bulk_calls),
                len(payload.materials),
            )
            self.assertTrue(all(ordered is False for _, ordered in files.bulk_calls))
            self.assertEqual(len(meta.replace_calls), 1)
            selector, doc, upsert = meta.replace_calls[0]
            self.assertEqual(selector, {"_id": "run-1"})
            self.assertIs(doc, payload.meta_doc)
            self.assertTrue(upsert)

    def test_no_file_material_still_writes_meta(self) -> None:
        client = _FakeClient()
        config = PersistConfig(uri="mongodb://unused", db="Test")
        written = write_run(
            client,
            config,
            run_id="run-empty",
            materials=[],
            meta_doc={"_id": "ignored", "status": "failed"},
        )
        self.assertEqual(written, 0)
        self.assertEqual(client["Test"]["runs_files"].bulk_calls, [])
        self.assertEqual(len(client["Test"]["runs_meta"].replace_calls), 1)


if __name__ == "__main__":
    unittest.main()
