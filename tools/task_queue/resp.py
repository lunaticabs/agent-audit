"""A minimal RESP2 client, standard library only.

The runner image does not ship ``redis-py`` (the only pinned Python
dependency is pymongo for the archive), and pulling a driver just to read one
stream entry is not worth the image surface. RESP2 is small enough to speak
directly; this module implements only what ``claim``/``ack``/``pending``
need.
"""

from __future__ import annotations

import socket
from typing import Any, BinaryIO, List, Optional, Tuple, Union

Reply = Union[bytes, str, int, None, List[Any]]


class RedisError(RuntimeError):
    """An error reply (``-ERR ...``) or a broken connection."""

    def __init__(self, message: str) -> None:
        super().__init__(message)
        self.message = message


class RedisClient:
    """One connection, one command at a time."""

    def __init__(
        self,
        host: str,
        port: int = 6379,
        db: int = 0,
        password: Optional[str] = None,
        timeout: float = 5.0,
    ) -> None:
        self.host = host
        self.port = port
        self.db = db
        self.password = password
        self.timeout = timeout
        self._sock: Optional[socket.socket] = None
        self._reader: Optional[BinaryIO] = None

    def __enter__(self) -> "RedisClient":
        self.connect()
        return self

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        self.close()

    def connect(self) -> None:
        self._sock = socket.create_connection((self.host, self.port), self.timeout)
        self._sock.settimeout(self.timeout)
        self._reader = self._sock.makefile("rb")
        if self.password:
            self.execute("AUTH", self.password)
        if self.db:
            self.execute("SELECT", str(self.db))

    def set_timeout(self, seconds: float) -> None:
        """Raise the socket timeout for a blocking command (XREADGROUP BLOCK)."""

        self.timeout = seconds
        if self._sock is not None:
            self._sock.settimeout(seconds)

    def close(self) -> None:
        if self._reader is not None:
            try:
                self._reader.close()
            except OSError:
                pass
            self._reader = None
        if self._sock is not None:
            try:
                self._sock.close()
            except OSError:
                pass
            self._sock = None

    def execute(self, *parts: str) -> Reply:
        """Send one command and return its decoded-enough reply.

        Bulk strings stay as ``bytes`` so prompts survive without a round trip
        through the terminal encoding. Callers decode the fields they use.
        """

        if self._sock is None or self._reader is None:
            raise RedisError("redis client is not connected")
        self._sock.sendall(encode_command(parts))
        return read_response(self._reader)


def encode_command(parts: Tuple[str, ...]) -> bytes:
    chunks = [("*%d\r\n" % len(parts)).encode("ascii")]
    for part in parts:
        raw = part.encode("utf-8")
        chunks.append(("$%d\r\n" % len(raw)).encode("ascii"))
        chunks.append(raw)
        chunks.append(b"\r\n")
    return b"".join(chunks)


def read_response(reader: BinaryIO) -> Reply:
    prefix = reader.read(1)
    if prefix == b"":
        raise RedisError("unexpected EOF from Redis")
    if prefix == b"+":
        return read_line(reader).decode("utf-8")
    if prefix == b"-":
        raise RedisError(read_line(reader).decode("utf-8", "replace"))
    if prefix == b":":
        return int(read_line(reader))
    if prefix == b"$":
        length = int(read_line(reader))
        if length == -1:
            return None
        data = reader.read(length)
        trailer = reader.read(2)
        if len(data) != length or trailer != b"\r\n":
            raise RedisError("invalid bulk string response")
        return data
    if prefix == b"*":
        count = int(read_line(reader))
        if count == -1:
            return None
        return [read_response(reader) for _ in range(count)]
    raise RedisError("unsupported RESP prefix: %r" % (prefix,))


def read_line(reader: BinaryIO) -> bytes:
    line = reader.readline()
    if line == b"":
        raise RedisError("unexpected EOF while reading Redis response")
    if not line.endswith(b"\r\n"):
        raise RedisError("invalid Redis line terminator")
    return line[:-2]


def as_text(value: Any) -> Optional[str]:
    if value is None:
        return None
    if isinstance(value, bytes):
        return value.decode("utf-8", "replace")
    return str(value)


def split_stream_reply(reply: Reply) -> List[Tuple[str, List[Tuple[str, List[Any]]]]]:
    """Normalise ``XREADGROUP`` output into ``[(stream, [(id, fields), ...])]``."""

    if not reply:
        return []
    result: List[Tuple[str, List[Tuple[str, List[Any]]]]] = []
    for stream_entry in reply:  # type: ignore[union-attr]
        if not isinstance(stream_entry, list) or len(stream_entry) != 2:
            raise RedisError("unexpected XREADGROUP stream entry: %r" % (stream_entry,))
        stream_name = as_text(stream_entry[0]) or ""
        entries: List[Tuple[str, List[Any]]] = []
        for entry in stream_entry[1]:
            if not isinstance(entry, list) or len(entry) != 2:
                raise RedisError("unexpected XREADGROUP entry: %r" % (entry,))
            entries.append((as_text(entry[0]) or "", list(entry[1])))
        result.append((stream_name, entries))
    return result
