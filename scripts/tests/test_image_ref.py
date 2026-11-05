from __future__ import annotations

import hashlib
import io
import json
import unittest
from email.message import Message
from pathlib import Path
from unittest import mock
from urllib.error import HTTPError

import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import enqueue_redis as enq  # noqa: E402


class FakeResponse:
    def __init__(self, body: bytes, headers=None):
        self._body = body
        self.headers = headers or {}

    def read(self) -> bytes:
        return self._body

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc, tb):
        return False


class ImageRefTests(unittest.TestCase):
    def test_ghcr_tag(self):
        ref = enq.parse_image_ref("ghcr.io/lunaticabs/agent-audit:rebuild")
        self.assertEqual(ref.registry, "ghcr.io")
        self.assertEqual(ref.repository, "lunaticabs/agent-audit")
        self.assertEqual(ref.reference, "rebuild")
        self.assertIsNone(ref.digest)

    def test_docker_hub_shorthand_gets_library_prefix(self):
        ref = enq.parse_image_ref("alpine:3.20")
        self.assertEqual(ref.registry, "docker.io")
        self.assertEqual(ref.repository, "library/alpine")

    def test_registry_with_port(self):
        ref = enq.parse_image_ref("localhost:5000/app")
        self.assertEqual(ref.registry, "localhost:5000")
        self.assertEqual(ref.repository, "app")
        self.assertEqual(ref.reference, "latest")

    def test_digest_reference(self):
        ref = enq.parse_image_ref("ghcr.io/a/b@sha256:" + "a" * 64)
        self.assertEqual(ref.digest, "sha256:" + "a" * 64)
        self.assertEqual(ref.pinned, "ghcr.io/a/b@sha256:" + "a" * 64)

    def test_rejects_non_sha256_digest(self):
        with self.assertRaises(ValueError):
            enq.parse_image_ref("ghcr.io/a/b@md5:abc")


class ResolveDigestTests(unittest.TestCase):
    def test_uses_docker_content_digest_header(self):
        digest = "sha256:" + "b" * 64
        with mock.patch.object(
            enq, "urlopen", return_value=FakeResponse(b"{}", {"Docker-Content-Digest": digest})
        ) as fake:
            pinned, resolved = enq.resolve_image_digest("ghcr.io/a/b:tag")
        self.assertEqual(pinned, "ghcr.io/a/b@" + digest)
        self.assertEqual(resolved, digest)
        request = fake.call_args[0][0]
        self.assertEqual(
            request.full_url, "https://ghcr.io/v2/a/b/manifests/tag"
        )
        self.assertIn("application/vnd.oci.image.index.v1+json", request.headers["Accept"])

    def test_falls_back_to_body_hash(self):
        body = b'{"schemaVersion": 2}'
        with mock.patch.object(enq, "urlopen", return_value=FakeResponse(body, {})):
            pinned, digest = enq.resolve_image_digest("ghcr.io/a/b:tag")
        expected = "sha256:" + hashlib.sha256(body).hexdigest()
        self.assertEqual(digest, expected)
        self.assertEqual(pinned, "ghcr.io/a/b@" + expected)

    def test_already_pinned_is_a_noop(self):
        ref = "ghcr.io/a/b@sha256:" + "c" * 64
        with mock.patch.object(enq, "urlopen") as fake:
            pinned, digest = enq.resolve_image_digest(ref)
        fake.assert_not_called()
        self.assertEqual(pinned, ref)
        self.assertIsNone(digest)

    def test_bearer_challenge_then_manifest(self):
        digest = "sha256:" + "d" * 64
        challenge = Message()
        challenge["WWW-Authenticate"] = (
            'Bearer realm="https://ghcr.io/token",service="ghcr.io",'
            'scope="repository:a/b:pull"'
        )
        unauthorized = HTTPError("https://ghcr.io/v2/a/b/manifests/tag", 401, "Unauthorized", challenge, None)
        token = FakeResponse(json.dumps({"token": "t0ken"}).encode())
        manifest = FakeResponse(b"{}", {"Docker-Content-Digest": digest})

        with mock.patch.object(enq, "urlopen", side_effect=[unauthorized, token, manifest]):
            pinned, resolved = enq.resolve_image_digest("ghcr.io/a/b:tag", timeout=5)

        self.assertEqual(resolved, digest)
        self.assertEqual(pinned, "ghcr.io/a/b@" + digest)

    def test_missing_tag_reports_http_error(self):
        headers = Message()
        error = HTTPError("https://ghcr.io/v2/a/b/manifests/nope", 404, "Not Found", headers, io.BytesIO(b'{"errors":[]}'))
        with mock.patch.object(enq, "urlopen", side_effect=error):
            with self.assertRaises(RuntimeError) as ctx:
                enq.resolve_image_digest("ghcr.io/a/b:nope")
        self.assertIn("HTTP 404", str(ctx.exception))


class EnqueueTests(unittest.TestCase):
    class FakeRedis:
        def __init__(self, group_error=None):
            self.commands = []
            self.group_error = group_error

        def __enter__(self):
            return self

        def __exit__(self, exc_type, exc, tb):
            return False

        def execute(self, *parts):
            self.commands.append(parts)
            if parts[0] == "XGROUP":
                if self.group_error is not None:
                    error = self.group_error
                    self.group_error = None
                    raise error
                return "OK"
            return "1-0"

    def _items(self):
        return [enq.EnqueueItem(1, "0x" + "a" * 40, "task-1", "Audit 0x... on eth.")]

    def test_creates_group_and_writes_digest_and_tag(self):
        client = self.FakeRedis()
        with mock.patch.object(enq, "RedisClient", return_value=client):
            enq.enqueue_items(
                self._items(),
                "s",
                "g",
                "127.0.0.1",
                6379,
                "ghcr.io/a/b@sha256:" + "e" * 64,
                "ghcr.io/a/b:rebuild",
                5,
            )
        self.assertEqual(client.commands[0], ("XGROUP", "CREATE", "s", "g", "$", "MKSTREAM"))
        xadd = client.commands[1]
        self.assertEqual(xadd[:3], ("XADD", "s", "*"))
        self.assertIn("ghcr.io/a/b@sha256:" + "e" * 64, xadd)
        self.assertIn("image_tag", xadd)
        self.assertIn("ghcr.io/a/b:rebuild", xadd)

    def test_busygroup_is_tolerated(self):
        client = self.FakeRedis(group_error=enq.RedisProtocolError("BUSYGROUP already exists"))
        with mock.patch.object(enq, "RedisClient", return_value=client):
            enq.enqueue_items(self._items(), "s", "g", "h", 1, "", "", 5)
        self.assertEqual(len(client.commands), 2)

    def test_other_group_errors_propagate(self):
        client = self.FakeRedis(group_error=enq.RedisProtocolError("ERR boom"))
        with mock.patch.object(enq, "RedisClient", return_value=client):
            with self.assertRaises(enq.RedisProtocolError):
                enq.enqueue_items(self._items(), "s", "g", "h", 1, "", "", 5)


if __name__ == "__main__":
    unittest.main()
