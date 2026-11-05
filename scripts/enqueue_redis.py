#!/usr/bin/env python3

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import re
import socket
import sys
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import BinaryIO, Optional, Tuple
from urllib.error import HTTPError, URLError
from urllib.parse import urlencode
from urllib.request import Request, urlopen


ROOT = Path(__file__).resolve().parent
DEFAULT_ADDRESS_FILE = ROOT / "addresses" / "addrs.txt"
DEFAULT_STREAM = "agent-audit:tasks"
# Must match the group in k3s/runner-scaledjob.yaml: KEDA scales on its lag and
# the runner claims from it.
DEFAULT_GROUP = "agent-audit-runners"
DEFAULT_HOST = "127.0.0.1"
DEFAULT_PORT = 6380
DEFAULT_PROMPT_TEMPLATE = "Audit {address} on {chain}."
DEFAULT_TASK_PREFIX = "audit"
ADDRESS_RE = re.compile(r"^0x[a-fA-F0-9]{40}$")

MANIFEST_ACCEPT = ", ".join(
    [
        "application/vnd.oci.image.index.v1+json",
        "application/vnd.docker.distribution.manifest.list.v2+json",
        "application/vnd.oci.image.manifest.v1+json",
        "application/vnd.docker.distribution.manifest.v2+json",
    ]
)


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


@dataclass(frozen=True)
class ImageRef:
    registry: str
    repository: str
    reference: str
    digest: Optional[str]

    @property
    def pinned(self) -> str:
        value = self.digest or self.reference
        return f"{self.registry}/{self.repository}@{value}"


def parse_image_ref(ref: str) -> ImageRef:
    """Split ``[registry/]repo[:tag][@sha256:...]`` without a dependency."""

    text = ref.strip()
    if text == "":
        raise ValueError("image reference is empty")

    digest: Optional[str] = None
    if "@" in text:
        text, digest = text.split("@", 1)
        if not digest.startswith("sha256:"):
            raise ValueError(f"unsupported digest in image reference: {ref!r}")
        tag = ""

    last_slash = text.rfind("/")
    last_colon = text.rfind(":")
    if "@" in ref:
        name = text
    elif last_colon > last_slash:
        name, tag = text[:last_colon], text[last_colon + 1 :]
    else:
        name, tag = text, "latest"

    first, separator, rest = name.partition("/")
    if separator and ("." in first or ":" in first or first == "localhost"):
        registry, repository = first, rest
    else:
        registry, repository = "docker.io", name

    if registry == "docker.io" and "/" not in repository:
        repository = "library/" + repository
    if not repository:
        raise ValueError(f"image reference has no repository: {ref!r}")
    if not digest and not tag:
        raise ValueError(f"image reference has no tag or digest: {ref!r}")

    reference = digest if digest else tag
    return ImageRef(
        registry=registry,
        repository=repository,
        reference=reference or "latest",
        digest=digest,
    )


def _bearer_token(
    challenge: str,
    repository: str,
    username: Optional[str],
    password: Optional[str],
    timeout: float,
) -> Optional[str]:
    scheme, _, params = challenge.partition(" ")
    if scheme.lower() != "bearer":
        return None
    fields = dict(re.findall(r'(\w+)="([^"]*)"', params))
    realm = fields.get("realm")
    if not realm:
        return None

    query = {"scope": f"repository:{repository}:pull"}
    if fields.get("service"):
        query["service"] = fields["service"]
    url = f"{realm}{'&' if '?' in realm else '?'}{urlencode(query)}"

    headers = {"User-Agent": "agent-audit-enqueue"}
    if username and password:
        raw = f"{username}:{password}".encode("utf-8")
        headers["Authorization"] = "Basic " + base64.b64encode(raw).decode("ascii")

    try:
        with urlopen(Request(url, headers=headers), timeout=timeout) as response:
            payload = json.loads(response.read().decode("utf-8"))
    except HTTPError as error:
        raise RuntimeError(
            f"registry token request failed: HTTP {error.code}. "
            "For a private image pass --registry-user and --registry-token."
        ) from error

    token = payload.get("token") or payload.get("access_token")
    if not token:
        raise RuntimeError("registry token response contained no token")
    return "Bearer " + token


def resolve_image_digest(
    ref: str,
    username: Optional[str] = None,
    password: Optional[str] = None,
    timeout: float = 20.0,
) -> Tuple[str, Optional[str]]:
    """Return ``(repo@sha256:..., digest)``; digest is None when already pinned."""

    parsed = parse_image_ref(ref)
    if parsed.digest:
        return parsed.pinned, None

    url = f"https://{parsed.registry}/v2/{parsed.repository}/manifests/{parsed.reference}"
    authorization: Optional[str] = None

    for attempt in range(2):
        headers = {"Accept": MANIFEST_ACCEPT, "User-Agent": "agent-audit-enqueue"}
        if authorization:
            headers["Authorization"] = authorization
        try:
            with urlopen(Request(url, headers=headers), timeout=timeout) as response:
                body = response.read()
                digest = response.headers.get("Docker-Content-Digest")
                if not digest:
                    digest = "sha256:" + hashlib.sha256(body).hexdigest()
                return (
                    f"{parsed.registry}/{parsed.repository}@{digest}",
                    digest,
                )
        except HTTPError as error:
            if error.code == 401 and attempt == 0:
                authorization = _bearer_token(
                    error.headers.get("WWW-Authenticate", ""),
                    parsed.repository,
                    username,
                    password,
                    timeout,
                )
                if authorization:
                    continue
                raise RuntimeError(
                    f"cannot resolve {ref}: registry asked for auth but sent no bearer challenge"
                ) from error
            detail = ""
            try:
                detail = error.read().decode("utf-8", "replace")[:200]
            except Exception:  # pragma: no cover - best effort only
                pass
            raise RuntimeError(
                f"cannot resolve {ref}: HTTP {error.code} from {parsed.registry}{detail and ' - ' + detail}"
            ) from error
        except URLError as error:
            raise RuntimeError(f"cannot reach {parsed.registry}: {error.reason}") from error

    raise RuntimeError(f"cannot resolve {ref}: exhausted auth attempts")



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
        "--group",
        default=DEFAULT_GROUP,
        help=(
            "Consumer group KEDA and the runners use. Created from $ (new "
            f"tasks only) when missing. Default: {DEFAULT_GROUP}"
        ),
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
        help=(
            "Runner image. Resolved to repo@sha256:... at enqueue time when "
            "--pin-digest is active (the default)."
        ),
    )
    parser.add_argument(
        "--pin-digest",
        action=argparse.BooleanOptionalAction,
        default=True,
        help="Resolve the image tag to a digest and enqueue the digest form. Default: on",
    )
    parser.add_argument(
        "--registry-user",
        default="",
        help="Registry username for the token endpoint (private images).",
    )
    parser.add_argument(
        "--registry-token",
        default="",
        help="Registry token/password for the token endpoint (private images).",
    )
    parser.add_argument(
        "--allow-unresolved-image",
        action="store_true",
        help="Enqueue the tag as-is when digest resolution fails instead of aborting.",
    )
    parser.add_argument(
        "--manifest-out",
        default="",
        help="Optional path: write a JSON manifest of image/tag/digest/task ids.",
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
) -> Tuple[list[EnqueueItem], str]:
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
    return items, batch_stamp


def slugify(value: str) -> str:
    text = value.strip().lower()
    slug = re.sub(r"[^a-z0-9]+", "-", text).strip("-")
    if slug == "":
        raise ValueError(f"cannot derive slug from value: {value!r}")
    return slug


def enqueue_items(
    items: list[EnqueueItem],
    stream: str,
    group: str,
    host: str,
    port: int,
    image: str,
    image_tag: str,
    timeout_sec: float,
) -> None:
    with RedisClient(host=host, port=port, timeout_sec=timeout_sec) as redis:
        # KEDA scales on this group's lag, so it must exist before the first
        # XADD. "$" means "only tasks enqueued from now on"; an existing group
        # raises BUSYGROUP, which is the normal path.
        try:
            redis.execute("XGROUP", "CREATE", stream, group, "$", "MKSTREAM")
            print(f"[GROUP] created stream={stream} group={group}")
        except RedisProtocolError as error:
            if "BUSYGROUP" not in str(error):
                raise
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
            if image_tag.strip() != "":
                command.extend(["image_tag", image_tag.strip()])
            reply = redis.execute(*command)
            if not isinstance(reply, str):
                raise RedisProtocolError(f"unexpected XADD reply: {reply!r}")
            print(
                f"[ENQ  ] index={item.index:04d} "
                f"task_id={item.task_id} "
                f"address={item.address} "
                f"redis_id={reply}"
            )


def print_dry_run(
    items: list[EnqueueItem],
    stream: str,
    group: str,
    host: str,
    port: int,
    image: str,
    image_tag: str,
) -> None:
    print(
        f"[DRY  ] stream={stream} group={group} host={host} port={port} "
        f"count={len(items)} image={image or '-'} tag={image_tag or '-'}"
    )
    for item in items:
        print(f"[TASK ] index={item.index:04d} task_id={item.task_id} address={item.address}")
        print(f"         prompt={item.prompt}")


def write_manifest(
    path: Path,
    items: list[EnqueueItem],
    batch_stamp: str,
    stream: str,
    group: str,
    image: str,
    image_tag: str,
    digest: Optional[str],
    chain: str,
) -> None:
    payload = {
        "schema_version": 1,
        "created_at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "batch_stamp": batch_stamp,
        "stream": stream,
        "group": group,
        "chain": chain,
        "image_tag": image_tag or None,
        "image": image or None,
        "digest": digest,
        "count": len(items),
        "tasks": [
            {"index": item.index, "task_id": item.task_id, "address": item.address}
            for item in items
        ],
    }
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")


def main() -> int:
    args = parse_args()
    path = Path(args.address_file).expanduser()
    addresses = load_addresses(path, args.max_count)
    items, batch_stamp = build_items(
        addresses=addresses,
        chain=args.chain,
        prompt_template=args.prompt_template,
        task_prefix=args.task_prefix,
    )

    image_tag = args.image.strip()
    image = image_tag
    digest: Optional[str] = None
    if image_tag and args.pin_digest:
        try:
            image, digest = resolve_image_digest(
                image_tag,
                username=args.registry_user or None,
                password=args.registry_token or None,
            )
        except (RuntimeError, ValueError) as error:
            if not args.allow_unresolved_image:
                print(f"[FAIL ] {error}", file=sys.stderr)
                print(
                    "[FAIL ] pass --no-pin-digest or --allow-unresolved-image "
                    "to enqueue the tag as-is",
                    file=sys.stderr,
                )
                return 2
            print(f"[WARN ] {error}; enqueueing the tag as-is", file=sys.stderr)
            image, digest = image_tag, None
        else:
            print(f"[IMAGE] tag={image_tag} digest={digest} pinned={image}")

    print(
        f"[LOAD ] file={path} unique_addresses={len(addresses)} "
        f"stream={args.stream} group={args.group} redis={args.host}:{args.port}"
    )

    if args.dry_run:
        print_dry_run(items, args.stream, args.group, args.host, args.port, image, image_tag)
        return 0

    enqueue_items(
        items=items,
        stream=args.stream,
        group=args.group,
        host=args.host,
        port=args.port,
        image=image,
        image_tag=image_tag,
        timeout_sec=args.timeout_sec,
    )
    print(f"[DONE ] enqueued={len(items)} stream={args.stream} group={args.group}")

    if args.manifest_out:
        manifest_path = Path(args.manifest_out).expanduser()
        write_manifest(
            manifest_path,
            items,
            batch_stamp,
            args.stream,
            args.group,
            image,
            image_tag,
            digest,
            args.chain,
        )
        print(f"[MANIF] {manifest_path}")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
