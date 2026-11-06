from __future__ import annotations

import sys
import unittest
from datetime import datetime, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import watch_batch as watch  # noqa: E402


def job(name, condition=None, active=0, failed=0):
    status = {"active": active, "failed": failed}
    if condition:
        status["conditions"] = [{"type": condition, "status": "True"}]
    return {"metadata": {"name": name}, "status": status}


class JobStatusTests(unittest.TestCase):
    def test_complete_condition_wins(self):
        self.assertEqual(watch.job_status(job("j", "Complete")), "Complete")

    def test_failed_condition_wins(self):
        self.assertEqual(watch.job_status(job("j", "Failed")), "Failed")

    def test_active_is_running(self):
        self.assertEqual(watch.job_status(job("j", active=1)), "Running")

    def test_no_status_is_pending(self):
        self.assertEqual(watch.job_status(job("j")), "Pending")


class FormatTests(unittest.TestCase):
    def test_durations(self):
        self.assertEqual(watch.fmt_duration(9), "9s")
        self.assertEqual(watch.fmt_duration(600), "10m")
        self.assertEqual(watch.fmt_duration(3660), "1h01m")
        self.assertEqual(watch.fmt_duration(90000), "1d01h")

    def test_pod_phase_and_attempts(self):
        pods = {
            "j": [
                {"metadata": {"creationTimestamp": "2026-01-01T00:00:00Z"}, "status": {"phase": "Failed"}},
                {"metadata": {"creationTimestamp": "2026-01-01T00:01:00Z"}, "status": {"phase": "Running"}},
            ]
        }
        self.assertEqual(watch.job_pods("j", pods), "run x2")
        self.assertEqual(watch.job_pods("missing", pods), "-")


class RenderTests(unittest.TestCase):
    def test_render_summarises_queue_and_jobs(self):
        data = {
            "now": datetime(2026, 1, 1, 0, 2, tzinfo=timezone.utc),
            "queue": {"lag": 3, "pending": 1, "consumers": 2},
            "stream_length": 100,
            "scaledjob": {"ready": "True", "active": "True", "paused": "False", "max": 26, "image": "x@sha256:1"},
            "counts": {"Running": 2, "Pending": 0, "Complete": 5, "Failed": 1},
            "jobs": [
                {"name": "job-a", "status": "Running", "age": 60, "duration": 60, "pods": "run x1"},
                {"name": "job-b", "status": "Failed", "age": 120, "duration": 120, "pods": "err x2"},
            ],
            "errors": [],
        }
        text = watch.render(data, limit=10, colors=False, group="g")
        self.assertIn("lag 3", text)
        self.assertIn("stream 100", text)
        self.assertIn("running 2  pending 0  complete 5  failed 1", text)
        self.assertIn("job-a", text)
        self.assertIn("job-b", text)


if __name__ == "__main__":
    unittest.main()
