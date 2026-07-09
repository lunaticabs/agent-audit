#!/usr/bin/env python3

from __future__ import annotations

import argparse
import csv
import hashlib
import re
import socket
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import BinaryIO


ROOT = Path(__file__).resolve().parent
DEFAULT_INPUT_CSV = ROOT / "addresses" / "addrs.csv"
DEFAULT_STREAM = "agent-audit:tasks"
DEFAULT_HOST = "127.0.0.1"
DEFAULT_PORT = 6380
DEFAULT_PROMPT_TEMPLATE = (
    "Check AGENTS.md and audit {address} on {chain}. "
    "source_kind={source_kind}; is_open_source={is_open_source}. "
    "Use the open-source workflow for open_source and the closed-source bytecode/Heimdall workflow for closed_source."
)
DEFAULT_TASK_PREFIX = "audit"
ADDRESS_RE = re.compile(r"^0x[a-fA-F0-9]{40}$")
REQUIRED_COLUMNS = {"address", "is_open_source"}


class RedisProtocolError(RuntimeError):
    pass


@dataclass(frozen=True)
class CsvAddress:
    address: str
    is_open_source: bool

    @property
    def source_kind(self) -> str:
        return "open_source" if self.is_open_source else "closed_source"


@dataclass(frozen=True)
class EnqueueItem:
    index: int
    address: str
    is_open_source: bool
    source_kind: str
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
        description="Build full prompts from a CSV address list and enqueue them into the Redis task stream.",
    )
    parser.add_argument(
        "--input-csv",
        "--address-file",
        dest="input_csv",
        default=str(DEFAULT_INPUT_CSV),
        help=f"CSV file with address,is_open_source columns. Default: {DEFAULT_INPUT_CSV}",
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
            "Prompt template. Available placeholders: {address}, {chain}, "
            "{source_kind}, {is_open_source}. "
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
        help="Only enqueue the first N unique chain/address/source_kind rows. Default: all",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Print the generated tasks without sending them to Redis.",
    )
    return parser.parse_args()


def load_csv_items(path: Path, chain: str, max_count: int) -> list[CsvAddress]:
    if not path.exists():
        raise FileNotFoundError(f"input CSV not found: {path}")

    items: list[CsvAddress] = []
    seen: set[tuple[str, str, str]] = set()
    with path.open("r", encoding="utf-8", newline="") as handle:
        reader = csv.DictReader(line for line in handle if not line.lstrip().startswith("#"))
        columns = set(reader.fieldnames or [])
        missing = REQUIRED_COLUMNS - columns
        if missing:
            joined = ", ".join(sorted(missing))
            raise ValueError(f"CSV is missing required column(s): {joined}")

        for line_no, row in enumerate(reader, start=2):
            address = (row.get("address") or "").strip()
            if not ADDRESS_RE.fullmatch(address):
                raise ValueError(f"invalid address at {path}:{line_no}: {address}")
            is_open_source = parse_bool(row.get("is_open_source"), path, line_no)
            source_kind = "open_source" if is_open_source else "closed_source"
            key = (chain.strip().lower(), address.lower(), source_kind)
            if key in seen:
                continue
            seen.add(key)
            items.append(CsvAddress(address=address, is_open_source=is_open_source))
            if max_count > 0 and len(items) >= max_count:
                break

    if not items:
        raise RuntimeError(f"no valid rows loaded from {path}")
    return items


def parse_bool(value: str | None, path: Path, line_no: int) -> bool:
    text = (value or "").strip().lower()
    if text == "true":
        return True
    if text == "false":
        return False
    raise ValueError(f"invalid is_open_source at {path}:{line_no}: {value!r}")


def build_items(
    csv_items: list[CsvAddress],
    chain: str,
    prompt_template: str,
    task_prefix: str,
) -> list[EnqueueItem]:
    batch_stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S%fZ").lower()
    chain_slug = slugify(chain)
    prefix_slug = slugify(task_prefix)
    items: list[EnqueueItem] = []

    for index, row in enumerate(csv_items, start=1):
        source_kind = row.source_kind
        is_open_source = "true" if row.is_open_source else "false"
        prompt = prompt_template.format(
            address=row.address,
            chain=chain,
            source_kind=source_kind,
            is_open_source=is_open_source,
        )
        addr_slug = row.address[2:14].lower()
        digest = hashlib.sha256(
            f"{chain}:{row.address}:{source_kind}".encode("utf-8")
        ).hexdigest()[:8]
        task_id = (
            f"{prefix_slug}-{batch_stamp}-{chain_slug}-{index:04d}-"
            f"{addr_slug}-{source_kind.replace('_', '-')}-{digest}"
        )
        items.append(
            EnqueueItem(
                index=index,
                address=row.address,
                is_open_source=row.is_open_source,
                source_kind=source_kind,
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
    chain: str,
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
                "address",
                item.address,
                "chain",
                chain,
                "is_open_source",
                "true" if item.is_open_source else "false",
                "source_kind",
                item.source_kind,
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
                f"source_kind={item.source_kind} "
                f"redis_id={reply}"
            )


def print_dry_run(
    items: list[EnqueueItem],
    stream: str,
    host: str,
    port: int,
    chain: str,
    image: str,
) -> None:
    print(
        f"[DRY  ] stream={stream} host={host} port={port} "
        f"chain={chain} count={len(items)} image={image or '-'}"
    )
    for item in items:
        print(
            f"[TASK ] index={item.index:04d} task_id={item.task_id} "
            f"address={item.address} source_kind={item.source_kind} "
            f"is_open_source={'true' if item.is_open_source else 'false'}"
        )
        print(f"         prompt={item.prompt}")


def main() -> int:
    args = parse_args()
    path = Path(args.input_csv).expanduser()
    rows = load_csv_items(path, args.chain, args.max_count)
    items = build_items(
        csv_items=rows,
        chain=args.chain,
        prompt_template=args.prompt_template,
        task_prefix=args.task_prefix,
    )

    print(
        f"[LOAD ] file={path} unique_rows={len(rows)} "
        f"stream={args.stream} redis={args.host}:{args.port}"
    )

    if args.dry_run:
        print_dry_run(items, args.stream, args.host, args.port, args.chain, args.image)
        return 0

    enqueue_items(
        items=items,
        stream=args.stream,
        host=args.host,
        port=args.port,
        chain=args.chain,
        image=args.image,
        timeout_sec=args.timeout_sec,
    )
    print(f"[DONE ] enqueued={len(items)} stream={args.stream}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
