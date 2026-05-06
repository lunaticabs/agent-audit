#!/usr/bin/env python3

from __future__ import annotations

import argparse
import hashlib
import re
import socket
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import BinaryIO


ROOT = Path(__file__).resolve().parent
DEFAULT_ADDRESS_FILE = ROOT / "addresses" / "addrs.txt"
DEFAULT_STREAM = "agent-audit:tasks"
DEFAULT_HOST = "127.0.0.1"
DEFAULT_PORT = 6380
DEFAULT_PROMPT_TEMPLATE = "Check AGENTS.md and audit {address} on {chain}."
DEFAULT_TASK_PREFIX = "audit"
ADDRESS_RE = re.compile(r"^0x[a-fA-F0-9]{40}$")


class RedisProtocolError(RuntimeError):
    pass


@dataclass(frozen=True)
class EnqueueItem:
    index: int
    address: str
    task_id: str
    prompt: str


class RedisClient:
    def __init__(self, host: str, port: int, timeout_sec: float) -> None:
        self.host = host
        self.port = port
        self.timeout_sec = timeout_sec
        self.sock: socket.socket | None = None
        self.reader: BinaryIO | None = None

    def __enter__(self) -> "RedisClient":
        self.sock = socket.create_connection((self.host, self.port), self.timeout_sec)
        self.sock.settimeout(self.timeout_sec)
        self.reader = self.sock.makefile("rb")
        return self

    def __exit__(self, exc_type, exc, tb) -> None:
        if self.reader is not None:
            self.reader.close()
        if self.sock is not None:
            self.sock.close()

    def execute(self, *parts: str) -> object:
        if self.sock is None or self.reader is None:
            raise RuntimeError("redis client is not connected")
        self.sock.sendall(encode_command(parts))
        return read_response(self.reader)


def encode_command(parts: tuple[str, ...]) -> bytes:
    chunks = [f"*{len(parts)}\r\n".encode("utf-8")]
    for part in parts:
        raw = part.encode("utf-8")
        chunks.append(f"${len(raw)}\r\n".encode("utf-8"))
        chunks.append(raw)
        chunks.append(b"\r\n")
    return b"".join(chunks)


def read_response(reader: BinaryIO) -> object:
    prefix = reader.read(1)
    if prefix == b"":
        raise RedisProtocolError("unexpected EOF from Redis")
    if prefix == b"+":
        return read_line(reader).decode("utf-8")
    if prefix == b"-":
        message = read_line(reader).decode("utf-8")
        raise RedisProtocolError(message)
    if prefix == b":":
        return int(read_line(reader))
    if prefix == b"$":
        length = int(read_line(reader))
        if length == -1:
            return None
        data = reader.read(length)
        trailer = reader.read(2)
        if len(data) != length or trailer != b"\r\n":
            raise RedisProtocolError("invalid bulk string response")
        return data.decode("utf-8")
    if prefix == b"*":
        count = int(read_line(reader))
        if count == -1:
            return None
        return [read_response(reader) for _ in range(count)]
    raise RedisProtocolError(f"unsupported RESP prefix: {prefix!r}")


def read_line(reader: BinaryIO) -> bytes:
    line = reader.readline()
    if line == b"":
        raise RedisProtocolError("unexpected EOF while reading Redis response")
    if not line.endswith(b"\r\n"):
        raise RedisProtocolError("invalid Redis line terminator")
    return line[:-2]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Build full prompts from an address list and enqueue them into the Redis task stream.",
    )
    parser.add_argument(
        "--address-file",
        default=str(DEFAULT_ADDRESS_FILE),
        help=f"Address file to load. Default: {DEFAULT_ADDRESS_FILE}",
    )
    parser.add_argument(
        "--chain",
        required=True,
        help="Chain label to inject into the prompt, for example eth or arb.",
    )
    parser.add_argument(
        "--stream",
        default=DEFAULT_STREAM,
        help=f"Redis stream name. Default: {DEFAULT_STREAM}",
    )
    parser.add_argument(
        "--host",
        default=DEFAULT_HOST,
        help=f"Redis host. Default: {DEFAULT_HOST}",
    )
    parser.add_argument(
        "--port",
        type=int,
        default=DEFAULT_PORT,
        help=f"Redis port. Default: {DEFAULT_PORT}",
    )
    parser.add_argument(
        "--image",
        default="",
        help="Optional runner image override to include in every task.",
    )
    parser.add_argument(
        "--prompt-template",
        default=DEFAULT_PROMPT_TEMPLATE,
        help=(
            "Prompt template. Available placeholders: {address}, {chain}. "
            f"Default: {DEFAULT_PROMPT_TEMPLATE!r}"
        ),
    )
    parser.add_argument(
        "--task-prefix",
        default=DEFAULT_TASK_PREFIX,
        help=f"Task ID prefix. Default: {DEFAULT_TASK_PREFIX}",
    )
    parser.add_argument(
        "--timeout-sec",
        type=float,
        default=5.0,
        help="Redis socket timeout in seconds. Default: 5",
    )
    parser.add_argument(
        "--max-count",
        type=int,
        default=0,
        help="Only enqueue the first N unique addresses. Default: all",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Print the generated tasks without sending them to Redis.",
    )
    return parser.parse_args()


def load_addresses(path: Path, max_count: int) -> list[str]:
    if not path.exists():
        raise FileNotFoundError(f"address file not found: {path}")

    addresses: list[str] = []
    seen: set[str] = set()
    with path.open("r", encoding="utf-8") as handle:
        for line_no, raw_line in enumerate(handle, start=1):
            text = raw_line.strip()
            if text == "" or text.startswith("#"):
                continue
            if not ADDRESS_RE.fullmatch(text):
                raise ValueError(f"invalid address at {path}:{line_no}: {text}")
            if text in seen:
                continue
            seen.add(text)
            addresses.append(text)
            if max_count > 0 and len(addresses) >= max_count:
                break

    if not addresses:
        raise RuntimeError(f"no valid addresses loaded from {path}")
    return addresses


def build_items(
    addresses: list[str],
    chain: str,
    prompt_template: str,
    task_prefix: str,
) -> list[EnqueueItem]:
    batch_stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S%fZ").lower()
    chain_slug = slugify(chain)
    prefix_slug = slugify(task_prefix)
    items: list[EnqueueItem] = []

    for index, address in enumerate(addresses, start=1):
        prompt = prompt_template.format(address=address, chain=chain)
        addr_slug = address[2:14].lower()
        digest = hashlib.sha256(f"{chain}:{address}".encode("utf-8")).hexdigest()[:8]
        task_id = f"{prefix_slug}-{batch_stamp}-{chain_slug}-{index:04d}-{addr_slug}-{digest}"
        items.append(
            EnqueueItem(
                index=index,
                address=address,
                task_id=task_id,
                prompt=prompt,
            )
        )
    return items


def slugify(value: str) -> str:
    text = value.strip().lower()
    slug = re.sub(r"[^a-z0-9]+", "-", text).strip("-")
    if slug == "":
        raise ValueError(f"cannot derive slug from value: {value!r}")
    return slug


def enqueue_items(
    items: list[EnqueueItem],
    stream: str,
    host: str,
    port: int,
    image: str,
    timeout_sec: float,
) -> None:
    with RedisClient(host=host, port=port, timeout_sec=timeout_sec) as redis:
        for item in items:
            command = [
                "XADD",
                stream,
                "*",
                "task_id",
                item.task_id,
                "full_prompt",
                item.prompt,
            ]
            if image.strip() != "":
                command.extend(["image", image.strip()])
            reply = redis.execute(*command)
            if not isinstance(reply, str):
                raise RedisProtocolError(f"unexpected XADD reply: {reply!r}")
            print(
                f"[ENQ  ] index={item.index:04d} "
                f"task_id={item.task_id} "
                f"address={item.address} "
                f"redis_id={reply}"
            )


def print_dry_run(items: list[EnqueueItem], stream: str, host: str, port: int, image: str) -> None:
    print(f"[DRY  ] stream={stream} host={host} port={port} count={len(items)} image={image or '-'}")
    for item in items:
        print(f"[TASK ] index={item.index:04d} task_id={item.task_id} address={item.address}")
        print(f"         prompt={item.prompt}")


def main() -> int:
    args = parse_args()
    path = Path(args.address_file).expanduser()
    addresses = load_addresses(path, args.max_count)
    items = build_items(
        addresses=addresses,
        chain=args.chain,
        prompt_template=args.prompt_template,
        task_prefix=args.task_prefix,
    )

    print(
        f"[LOAD ] file={path} unique_addresses={len(addresses)} "
        f"stream={args.stream} redis={args.host}:{args.port}"
    )

    if args.dry_run:
        print_dry_run(items, args.stream, args.host, args.port, args.image)
        return 0

    enqueue_items(
        items=items,
        stream=args.stream,
        host=args.host,
        port=args.port,
        image=args.image,
        timeout_sec=args.timeout_sec,
    )
    print(f"[DONE ] enqueued={len(items)} stream={args.stream}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
