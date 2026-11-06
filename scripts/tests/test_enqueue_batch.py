from __future__ import annotations

import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import enqueue_batch as batch  # noqa: E402

DIGEST = "sha256:" + "a" * 64
OTHER = "sha256:" + "b" * 64


def write_manifest(path: Path, image: str, agent_image: str) -> None:
    path.write_text(
        "spec:\n"
        "  jobTargetRef:\n"
        "    template:\n"
        "      spec:\n"
        "        containers:\n"
        "          - name: agent-audit\n"
        "            image: %s\n"
        "            env:\n"
        "              - name: AGENT_AUDIT_IMAGE\n"
        "                value: %s\n" % (image, agent_image),
        encoding="utf-8",
    )


class ScaledJobImageTests(unittest.TestCase):
    def test_reads_both_occurrences(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "scaledjob.yaml"
            write_manifest(path, "ghcr.io/a/b@" + DIGEST, "ghcr.io/a/b@" + DIGEST)
            image, agent_image = batch.read_scaledjob_image(path)
        self.assertEqual(image, "ghcr.io/a/b@" + DIGEST)
        self.assertEqual(agent_image, "ghcr.io/a/b@" + DIGEST)

    def test_mismatch_is_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "scaledjob.yaml"
            write_manifest(path, "ghcr.io/a/b@" + DIGEST, "ghcr.io/a/b@" + OTHER)
            with self.assertRaises(RuntimeError):
                batch.resolve_requested_image("", path, None, None)

    def test_digest_is_used_as_is(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "scaledjob.yaml"
            write_manifest(path, "ghcr.io/a/b@" + DIGEST, "ghcr.io/a/b@" + DIGEST)
            pinned, tag, digest = batch.resolve_requested_image("", path, None, None)
        self.assertEqual(pinned, "ghcr.io/a/b@" + DIGEST)
        self.assertEqual(tag, "")
        self.assertIsNone(digest)

    def test_explicit_image_must_match_pod(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "scaledjob.yaml"
            write_manifest(path, "ghcr.io/a/b@" + DIGEST, "ghcr.io/a/b@" + DIGEST)
            with self.assertRaises(RuntimeError):
                batch.resolve_requested_image("ghcr.io/a/b@" + OTHER, path, None, None)


class PortTests(unittest.TestCase):
    def test_free_port_is_usable(self):
        port = batch.find_free_port()
        self.assertGreater(port, 0)
        self.assertLess(port, 65536)


if __name__ == "__main__":
    unittest.main()
