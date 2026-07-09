from __future__ import annotations

import importlib.util
import io
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "enqueue_redis.py"


def load_module():
    spec = importlib.util.spec_from_file_location("enqueue_redis", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    assert spec is not None
    assert spec.loader is not None
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


enqueue_redis = load_module()


class FakeRedisClient:
    commands: list[tuple[str, ...]] = []

    def __init__(self, host: str, port: int, timeout_sec: float) -> None:
        self.host = host
        self.port = port
        self.timeout_sec = timeout_sec

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc, tb) -> None:
        return None

    def execute(self, *parts: str) -> str:
        self.commands.append(tuple(parts))
        return "1-0"


class EnqueueRedisTests(unittest.TestCase):
    def write_csv(self, text: str) -> Path:
        handle = tempfile.NamedTemporaryFile("w", encoding="utf-8", newline="", delete=False)
        with handle:
            handle.write(text)
        path = Path(handle.name)
        self.addCleanup(path.unlink, missing_ok=True)
        return path

    def test_load_csv_requires_expected_columns(self) -> None:
        path = self.write_csv("address\n0x0000000000000000000000000000000000000000\n")

        with self.assertRaisesRegex(ValueError, "is_open_source"):
            enqueue_redis.load_csv_items(path, "eth", 0)

    def test_load_csv_rejects_invalid_boolean(self) -> None:
        path = self.write_csv(
            "address,is_open_source\n"
            "0x0000000000000000000000000000000000000000,yes\n"
        )

        with self.assertRaisesRegex(ValueError, "invalid is_open_source"):
            enqueue_redis.load_csv_items(path, "eth", 0)

    def test_load_csv_deduplicates_by_chain_address_and_source_kind(self) -> None:
        path = self.write_csv(
            "address,is_open_source\n"
            "0x0000000000000000000000000000000000000000,true\n"
            "0x0000000000000000000000000000000000000000,TRUE\n"
            "0x0000000000000000000000000000000000000000,false\n"
        )

        items = enqueue_redis.load_csv_items(path, "eth", 0)

        self.assertEqual(len(items), 2)
        self.assertEqual([item.source_kind for item in items], ["open_source", "closed_source"])

    def test_build_items_includes_source_kind_prompt_fields(self) -> None:
        rows = [
            enqueue_redis.CsvAddress(
                address="0x0000000000000000000000000000000000000000",
                is_open_source=False,
            )
        ]

        items = enqueue_redis.build_items(
            rows,
            "eth",
            "{address} {chain} {source_kind} {is_open_source}",
            "Audit Batch",
        )

        self.assertEqual(items[0].source_kind, "closed_source")
        self.assertIn("closed_source", items[0].prompt)
        self.assertIn("false", items[0].prompt)
        self.assertIn("-closed-source-", items[0].task_id)

    def test_enqueue_items_xadd_includes_source_fields(self) -> None:
        FakeRedisClient.commands = []
        item = enqueue_redis.EnqueueItem(
            index=1,
            address="0x0000000000000000000000000000000000000000",
            is_open_source=False,
            source_kind="closed_source",
            task_id="audit-1",
            prompt="audit",
        )

        with patch.object(enqueue_redis, "RedisClient", FakeRedisClient):
            output = io.StringIO()
            with redirect_stdout(output):
                enqueue_redis.enqueue_items(
                    [item],
                    "agent-audit:tasks",
                    "127.0.0.1",
                    6380,
                    "eth",
                    "",
                    5.0,
                )

        command = FakeRedisClient.commands[0]
        self.assertIn("source_kind", command)
        self.assertIn("closed_source", command)
        self.assertIn("is_open_source", command)
        self.assertIn("false", command)
        self.assertIn("address", command)
        self.assertIn("chain", command)


if __name__ == "__main__":
    unittest.main()
