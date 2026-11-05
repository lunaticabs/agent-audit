from __future__ import annotations

import hashlib
import io
import json
import unittest

from task_queue import queue as task_queue
from task_queue import runid
from task_queue.resp import RedisError, encode_command, read_response, split_stream_reply


def stream_reply(stream: str, entries):
    return [
        [
            stream.encode(),
            [
                [mid.encode(), [part.encode() for pair in fields for part in pair]]
                for mid, fields in entries
            ],
        ]
    ]


class FakeClient:
    """Records commands and replays canned replies."""

    def __init__(self, replies):
        self.replies = list(replies)
        self.commands = []
        self.timeout = 10.0

    def execute(self, *parts):
        self.commands.append(parts)
        if not self.replies:
            raise AssertionError("unexpected command: %r" % (parts,))
        reply = self.replies.pop(0)
        if isinstance(reply, Exception):
            raise reply
        return reply

    def set_timeout(self, seconds):
        self.timeout = seconds


class RunIdTests(unittest.TestCase):
    def test_matches_dispatcher_example(self):
        self.assertEqual(
            runid.run_id_for_task("Audit 2026/05/05:Mainnet"),
            "audit-2026-05-05-mainnet",
        )

    def test_short_id_is_only_sanitized(self):
        self.assertEqual(runid.run_id_for_task("audit-20261026-001"), "audit-20261026-001")

    def test_blank_and_symbol_only_ids_become_task(self):
        self.assertEqual(runid.run_id_for_task(""), "task")
        self.assertEqual(runid.run_id_for_task("///"), "task")

    def test_long_id_gets_hash_suffix_and_fits(self):
        task_id = "x" * 128
        value = runid.run_id_for_task(task_id)
        suffix = hashlib.sha256(task_id.encode()).hexdigest()[:8]
        self.assertEqual(value, "x" * 71 + "-" + suffix)
        self.assertEqual(len(value), 80)

    def test_non_ascii_and_punctuation_collapse(self):
        self.assertEqual(runid.run_id_for_task("审计 0xAB/CD"), "0xab-cd")

    def test_repeated_separators_collapse(self):
        self.assertEqual(runid.run_id_for_task("a---b__c"), "a-b-c")


class RedisUrlTests(unittest.TestCase):
    def test_defaults(self):
        host, port, db, password = task_queue.parse_redis_url("redis://redis:6379/0")
        self.assertEqual((host, port, db, password), ("redis", 6379, 0, None))

    def test_password_and_db(self):
        host, port, db, password = task_queue.parse_redis_url("redis://:secret@cache:6380/3")
        self.assertEqual((host, port, db, password), ("cache", 6380, 3, "secret"))

    def test_rejects_other_schemes(self):
        with self.assertRaises(ValueError):
            task_queue.parse_redis_url("rediss://cache:6380/0")


class RespTests(unittest.TestCase):
    def test_encode_command(self):
        self.assertEqual(
            encode_command(("SET", "a", "b")),
            b"*3\r\n$3\r\nSET\r\n$1\r\na\r\n$1\r\nb\r\n",
        )

    def test_parses_simple_types(self):
        reader = io.BytesIO(b"+OK\r\n$3\r\nfoo\r\n:42\r\n*-1\r\n")
        self.assertEqual(read_response(reader), "OK")
        self.assertEqual(read_response(reader), b"foo")
        self.assertEqual(read_response(reader), 42)
        self.assertIsNone(read_response(reader))

    def test_error_reply_raises(self):
        with self.assertRaises(RedisError):
            read_response(io.BytesIO(b"-NOGROUP no such group\r\n"))

    def test_split_stream_reply(self):
        reply = stream_reply("s", [("1-0", [("task_id", "t1")])])
        self.assertEqual(split_stream_reply(reply)[0][1][0][0], "1-0")
        self.assertEqual(split_stream_reply(None), [])


class ClaimTests(unittest.TestCase):
    def test_claims_new_work_with_attempt_one(self):
        client = FakeClient(
            [
                None,  # own pending list is empty
                stream_reply("s", [("1-0", [("task_id", "t1"), ("full_prompt", "audit")])]),
            ]
        )
        task = task_queue.claim(client, stream="s", group="g", consumer="job-a")
        self.assertIsNotNone(task)
        self.assertEqual(task.attempt, 1)
        self.assertEqual(task.run_id, "t1")
        self.assertIn("BLOCK", client.commands[1])

    def test_reclaims_own_pending_with_attempt_two(self):
        client = FakeClient(
            [stream_reply("s", [("1-0", [("task_id", "t1"), ("full_prompt", "audit")])])]
        )
        task = task_queue.claim(client, stream="s", group="g", consumer="job-a")
        self.assertEqual(task.attempt, 2)
        self.assertEqual(client.commands[0][-1], "0")
        self.assertEqual(len(client.commands), 1)

    def test_poison_message_is_acked_and_skipped(self):
        client = FakeClient(
            [
                stream_reply("s", [("1-0", [("full_prompt", "audit")])]),
                1,  # XACK
                None,
                None,
            ]
        )
        task = task_queue.claim(client, stream="s", group="g", consumer="job-a")
        self.assertIsNone(task)
        self.assertEqual(client.commands[1][0], "XACK")
        self.assertEqual(client.commands[1][-1], "1-0")

    def test_image_fields_survive(self):
        client = FakeClient(
            [
                None,
                stream_reply(
                    "s",
                    [
                        (
                            "1-0",
                            [
                                ("task_id", "t1"),
                                ("full_prompt", "audit 0xabc"),
                                ("image", "ghcr.io/x/y@sha256:deadbeef"),
                                ("image_tag", "ghcr.io/x/y:rebuild"),
                            ],
                        )
                    ],
                ),
            ]
        )
        task = task_queue.claim(client, stream="s", group="g", consumer="job-a", block_ms=0)
        self.assertEqual(task.image, "ghcr.io/x/y@sha256:deadbeef")
        self.assertEqual(task.image_tag, "ghcr.io/x/y:rebuild")
        self.assertEqual(json.loads(task_queue.dumps(task))["task_id"], "t1")


class AckAndPendingTests(unittest.TestCase):
    def test_ack_issues_xack(self):
        client = FakeClient([2])
        self.assertEqual(task_queue.ack(client, "s", "g", ["1-0", "2-0"]), 2)
        self.assertEqual(client.commands[0], ("XACK", "s", "g", "1-0", "2-0"))

    def test_ack_without_ids_is_a_noop(self):
        client = FakeClient([])
        self.assertEqual(task_queue.ack(client, "s", "g", []), 0)
        self.assertEqual(client.commands, [])

    def test_pending_summary(self):
        client = FakeClient([[3, b"1-0", b"3-0", [[b"job-a", b"2"], [b"job-b", b"1"]]]])
        summary = task_queue.pending_summary(client, "s", "g")
        self.assertEqual(summary["count"], 3)
        self.assertEqual(summary["consumers"], {"job-a": 2, "job-b": 1})

    def test_requeue_copies_fields(self):
        client = FakeClient(
            [
                [[b"1-0", [b"task_id", b"t1", b"full_prompt", b"audit"]]],
                b"2-0",
                1,
            ]
        )
        new_id = task_queue.requeue(client, "s", "g", "1-0")
        self.assertEqual(new_id, "2-0")
        self.assertEqual(client.commands[1][:3], ("XADD", "s", "*"))
        self.assertEqual(client.commands[2][0], "XACK")


if __name__ == "__main__":
    unittest.main()
