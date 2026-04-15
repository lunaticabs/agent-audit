#!/usr/bin/env python3
"""Probe whether an OpenAI-compatible API is usable for the audit agent."""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
from dataclasses import asdict, dataclass
from typing import Any, Dict, Iterable, List, Optional, Sequence, Tuple
from urllib import error, parse, request


DEFAULT_TIMEOUT = 45.0
TEXT_PROMPT = "Reply with the exact word pong and nothing else."
TOOL_NAME = "probe_echo"
TOOL_SCHEMA = {
    "type": "object",
    "properties": {
        "message": {"type": "string"},
    },
    "required": ["message"],
    "additionalProperties": False,
}
STRUCTURED_SCHEMA = {
    "type": "object",
    "properties": {
        "verdict": {"type": "string"},
        "endpoint": {"type": "string"},
    },
    "required": ["verdict", "endpoint"],
    "additionalProperties": False,
}


@dataclass
class ProbeResult:
    name: str
    endpoint: str
    ok: bool
    http_status: Optional[int]
    latency_ms: int
    detail: str
    error: Optional[str] = None
    payload_excerpt: Optional[str] = None


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Check whether an OpenAI-compatible API supports the features "
            "needed by the smart-contract audit agent."
        )
    )
    parser.add_argument(
        "--base-url",
        default=os.getenv("OPENAI_BASE_URL") or os.getenv("OPENAI_API_BASE"),
        help=(
            "OpenAI-compatible API root. If /v1 is missing, it will be added. "
            "Defaults to OPENAI_BASE_URL or OPENAI_API_BASE."
        ),
    )
    parser.add_argument(
        "--api-key",
        default=os.getenv("OPENAI_API_KEY"),
        help="API key. Defaults to OPENAI_API_KEY.",
    )
    parser.add_argument(
        "--model",
        default=os.getenv("OPENAI_MODEL") or "gpt-5.4",
        help="Model id to probe. Defaults to OPENAI_MODEL or gpt-5.4.",
    )
    parser.add_argument(
        "--timeout",
        type=float,
        default=DEFAULT_TIMEOUT,
        help=f"Request timeout in seconds. Defaults to {DEFAULT_TIMEOUT}.",
    )
    parser.add_argument(
        "--skip-responses",
        action="store_true",
        help="Skip Responses API probes.",
    )
    parser.add_argument(
        "--skip-chat",
        action="store_true",
        help="Skip Chat Completions probes.",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="Emit the final report as JSON.",
    )
    parser.add_argument(
        "--extra-header",
        action="append",
        default=[],
        metavar="NAME=VALUE",
        help="Extra header to send. May be provided multiple times.",
    )
    return parser.parse_args()


def normalize_base_url(base_url: str) -> str:
    base_url = base_url.strip().rstrip("/")
    parsed = parse.urlparse(base_url)
    path = parsed.path.rstrip("/")
    if path.endswith("/v1"):
        return base_url
    new_path = f"{path}/v1" if path else "/v1"
    return parse.urlunparse(parsed._replace(path=new_path))


def parse_extra_headers(pairs: Sequence[str]) -> Dict[str, str]:
    headers: Dict[str, str] = {}
    env_json = os.getenv("OPENAI_EXTRA_HEADERS_JSON")
    if env_json:
        try:
            parsed = json.loads(env_json)
            if isinstance(parsed, dict):
                headers.update({str(k): str(v) for k, v in parsed.items()})
        except json.JSONDecodeError:
            headers["X-Compat-Warning"] = "invalid OPENAI_EXTRA_HEADERS_JSON ignored"
    for pair in pairs:
        if "=" not in pair:
            raise ValueError(f"invalid --extra-header value: {pair!r}")
        name, value = pair.split("=", 1)
        headers[name.strip()] = value.strip()
    return headers


def build_headers(api_key: str, extra_headers: Dict[str, str]) -> Dict[str, str]:
    headers = {
        "Authorization": f"Bearer {api_key}",
        "Content-Type": "application/json",
        "Accept": "application/json",
    }
    if os.getenv("OPENAI_ORG"):
        headers["OpenAI-Organization"] = os.getenv("OPENAI_ORG", "")
    if os.getenv("OPENAI_PROJECT"):
        headers["OpenAI-Project"] = os.getenv("OPENAI_PROJECT", "")
    headers.update(extra_headers)
    return headers


def excerpt(text: str, limit: int = 220) -> str:
    compact = " ".join(text.split())
    return compact if len(compact) <= limit else compact[: limit - 3] + "..."


def dump_excerpt(payload: Any) -> str:
    try:
        return excerpt(json.dumps(payload, ensure_ascii=True, sort_keys=True))
    except TypeError:
        return excerpt(str(payload))


def post_json(
    url: str,
    payload: Dict[str, Any],
    headers: Dict[str, str],
    timeout: float,
) -> Tuple[Optional[int], Dict[str, Any], Optional[str], int]:
    body = json.dumps(payload).encode("utf-8")
    req = request.Request(url=url, data=body, headers=headers, method="POST")
    started = time.perf_counter()
    try:
        with request.urlopen(req, timeout=timeout) as resp:
            raw = resp.read().decode("utf-8")
            latency_ms = int((time.perf_counter() - started) * 1000)
            parsed_body = json.loads(raw) if raw else {}
            return resp.status, parsed_body, None, latency_ms
    except error.HTTPError as exc:
        latency_ms = int((time.perf_counter() - started) * 1000)
        raw = exc.read().decode("utf-8", errors="replace")
        try:
            parsed_body = json.loads(raw) if raw else {}
        except json.JSONDecodeError:
            parsed_body = {"raw": raw}
        return exc.code, parsed_body, raw, latency_ms
    except Exception as exc:  # pragma: no cover - network/runtime failure path
        latency_ms = int((time.perf_counter() - started) * 1000)
        return None, {}, str(exc), latency_ms


def iter_dicts(value: Any) -> Iterable[Dict[str, Any]]:
    if isinstance(value, dict):
        yield value
        for child in value.values():
            yield from iter_dicts(child)
    elif isinstance(value, list):
        for item in value:
            yield from iter_dicts(item)


def extract_responses_text(body: Dict[str, Any]) -> str:
    if isinstance(body.get("output_text"), str) and body["output_text"].strip():
        return body["output_text"].strip()

    output = body.get("output")
    if isinstance(output, list):
        fragments: List[str] = []
        for item in output:
            if not isinstance(item, dict):
                continue
            content = item.get("content")
            if isinstance(content, list):
                for part in content:
                    if not isinstance(part, dict):
                        continue
                    if part.get("type") in {"output_text", "text"} and isinstance(
                        part.get("text"), str
                    ):
                        fragments.append(part["text"])
        if fragments:
            return "".join(fragments).strip()
    return ""


def extract_chat_text(body: Dict[str, Any]) -> str:
    choices = body.get("choices")
    if not isinstance(choices, list) or not choices:
        return ""
    first = choices[0]
    if not isinstance(first, dict):
        return ""
    message = first.get("message")
    if not isinstance(message, dict):
        return ""
    content = message.get("content")
    if isinstance(content, str):
        return content.strip()
    if isinstance(content, list):
        fragments: List[str] = []
        for part in content:
            if isinstance(part, dict) and isinstance(part.get("text"), str):
                fragments.append(part["text"])
        return "".join(fragments).strip()
    return ""


def detect_tool_call(body: Dict[str, Any], tool_name: str) -> bool:
    for item in iter_dicts(body):
        name = item.get("name")
        if name != tool_name:
            continue
        if item.get("type") in {"function_call", "function"}:
            return True
        if "arguments" in item or "tool_calls" in item:
            return True
    choices = body.get("choices")
    if isinstance(choices, list) and choices:
        first = choices[0]
        if isinstance(first, dict) and first.get("finish_reason") == "tool_calls":
            return True
    return False


def parse_json_text(text: str) -> Optional[Dict[str, Any]]:
    try:
        parsed = json.loads(text)
    except json.JSONDecodeError:
        return None
    return parsed if isinstance(parsed, dict) else None


def probe_responses_text(
    base_url: str, model: str, headers: Dict[str, str], timeout: float
) -> ProbeResult:
    payload = {"model": model, "input": TEXT_PROMPT}
    status, body, raw_error, latency_ms = post_json(
        f"{base_url}/responses", payload, headers, timeout
    )
    text = extract_responses_text(body)
    ok = status == 200 and text.lower() == "pong"
    detail = f"response_text={text!r}" if text else "no text extracted"
    error_msg = None if ok else infer_error(raw_error, body)
    return ProbeResult(
        name="responses_text",
        endpoint="/responses",
        ok=ok,
        http_status=status,
        latency_ms=latency_ms,
        detail=detail,
        error=error_msg,
        payload_excerpt=dump_excerpt(body),
    )


def probe_responses_tool(
    base_url: str, model: str, headers: Dict[str, str], timeout: float
) -> ProbeResult:
    payload = {
        "model": model,
        "input": (
            "You must call the probe_echo tool with "
            '{"message":"ping"}. Do not answer in plain text.'
        ),
        "tools": [
            {
                "type": "function",
                "name": TOOL_NAME,
                "description": "Echo the provided message for compatibility checks.",
                "parameters": TOOL_SCHEMA,
            }
        ],
    }
    status, body, raw_error, latency_ms = post_json(
        f"{base_url}/responses", payload, headers, timeout
    )
    ok = status == 200 and detect_tool_call(body, TOOL_NAME)
    detail = "tool_call_detected" if ok else "tool call not detected"
    error_msg = None if ok else infer_error(raw_error, body)
    return ProbeResult(
        name="responses_tool_call",
        endpoint="/responses",
        ok=ok,
        http_status=status,
        latency_ms=latency_ms,
        detail=detail,
        error=error_msg,
        payload_excerpt=dump_excerpt(body),
    )


def probe_responses_structured(
    base_url: str, model: str, headers: Dict[str, str], timeout: float
) -> ProbeResult:
    payload = {
        "model": model,
        "input": 'Return JSON with verdict="ok" and endpoint="responses".',
        "text": {
            "format": {
                "type": "json_schema",
                "name": "compat_probe",
                "strict": True,
                "schema": STRUCTURED_SCHEMA,
            }
        },
    }
    status, body, raw_error, latency_ms = post_json(
        f"{base_url}/responses", payload, headers, timeout
    )
    text = extract_responses_text(body)
    parsed = parse_json_text(text) if text else None
    ok = (
        status == 200
        and isinstance(parsed, dict)
        and parsed.get("verdict") == "ok"
        and parsed.get("endpoint") == "responses"
    )
    detail = f"structured_text={text!r}" if text else "no structured text extracted"
    error_msg = None if ok else infer_error(raw_error, body)
    return ProbeResult(
        name="responses_structured_output",
        endpoint="/responses",
        ok=ok,
        http_status=status,
        latency_ms=latency_ms,
        detail=detail,
        error=error_msg,
        payload_excerpt=dump_excerpt(body),
    )


def probe_chat_text(
    base_url: str, model: str, headers: Dict[str, str], timeout: float
) -> ProbeResult:
    payload = {
        "model": model,
        "messages": [{"role": "user", "content": TEXT_PROMPT}],
    }
    status, body, raw_error, latency_ms = post_json(
        f"{base_url}/chat/completions", payload, headers, timeout
    )
    text = extract_chat_text(body)
    ok = status == 200 and text.lower() == "pong"
    detail = f"response_text={text!r}" if text else "no text extracted"
    error_msg = None if ok else infer_error(raw_error, body)
    return ProbeResult(
        name="chat_text",
        endpoint="/chat/completions",
        ok=ok,
        http_status=status,
        latency_ms=latency_ms,
        detail=detail,
        error=error_msg,
        payload_excerpt=dump_excerpt(body),
    )


def probe_chat_tool(
    base_url: str, model: str, headers: Dict[str, str], timeout: float
) -> ProbeResult:
    payload = {
        "model": model,
        "messages": [
            {
                "role": "user",
                "content": (
                    "You must call the probe_echo tool with "
                    '{"message":"ping"}. Do not answer in plain text.'
                ),
            }
        ],
        "tools": [
            {
                "type": "function",
                "function": {
                    "name": TOOL_NAME,
                    "description": (
                        "Echo the provided message for compatibility checks."
                    ),
                    "parameters": TOOL_SCHEMA,
                },
            }
        ],
    }
    status, body, raw_error, latency_ms = post_json(
        f"{base_url}/chat/completions", payload, headers, timeout
    )
    ok = status == 200 and detect_tool_call(body, TOOL_NAME)
    detail = "tool_call_detected" if ok else "tool call not detected"
    error_msg = None if ok else infer_error(raw_error, body)
    return ProbeResult(
        name="chat_tool_call",
        endpoint="/chat/completions",
        ok=ok,
        http_status=status,
        latency_ms=latency_ms,
        detail=detail,
        error=error_msg,
        payload_excerpt=dump_excerpt(body),
    )


def probe_chat_structured(
    base_url: str, model: str, headers: Dict[str, str], timeout: float
) -> ProbeResult:
    payload = {
        "model": model,
        "messages": [
            {
                "role": "user",
                "content": 'Return JSON with verdict="ok" and endpoint="chat".',
            }
        ],
        "response_format": {
            "type": "json_schema",
            "json_schema": {
                "name": "compat_probe",
                "strict": True,
                "schema": STRUCTURED_SCHEMA,
            },
        },
    }
    status, body, raw_error, latency_ms = post_json(
        f"{base_url}/chat/completions", payload, headers, timeout
    )
    text = extract_chat_text(body)
    parsed = parse_json_text(text) if text else None
    ok = (
        status == 200
        and isinstance(parsed, dict)
        and parsed.get("verdict") == "ok"
        and parsed.get("endpoint") == "chat"
    )
    detail = f"structured_text={text!r}" if text else "no structured text extracted"
    error_msg = None if ok else infer_error(raw_error, body)
    return ProbeResult(
        name="chat_structured_output",
        endpoint="/chat/completions",
        ok=ok,
        http_status=status,
        latency_ms=latency_ms,
        detail=detail,
        error=error_msg,
        payload_excerpt=dump_excerpt(body),
    )


def infer_error(raw_error: Optional[str], body: Dict[str, Any]) -> str:
    if raw_error and raw_error.strip():
        return excerpt(raw_error)
    for item in iter_dicts(body):
        err = item.get("error")
        if isinstance(err, dict):
            message = err.get("message")
            if isinstance(message, str) and message.strip():
                return excerpt(message)
            code = err.get("code")
            if code is not None:
                return excerpt(str(code))
        if item.get("message") and item.get("type") == "error":
            return excerpt(str(item["message"]))
    if body:
        return excerpt(json.dumps(body, ensure_ascii=True))
    return "unknown error"


def classify(results: Sequence[ProbeResult]) -> Tuple[str, str]:
    status = {result.name: result.ok for result in results}

    if (
        status.get("responses_text")
        and status.get("responses_tool_call")
        and status.get("responses_structured_output")
    ):
        return (
            "responses_ready",
            "Suitable for a Responses API + Agents SDK audit orchestrator.",
        )

    if status.get("responses_text") and status.get("responses_tool_call"):
        return (
            "responses_partial",
            "Usable with Responses API, but structured outputs appear unsupported.",
        )

    if (
        status.get("chat_text")
        and status.get("chat_tool_call")
        and status.get("chat_structured_output")
    ):
        return (
            "chat_ready",
            "Usable through Chat Completions or the Agents SDK chat fallback.",
        )

    if status.get("chat_text") and status.get("chat_tool_call"):
        return (
            "chat_partial",
            "Usable through Chat Completions fallback, but structured outputs appear unsupported.",
        )

    if status.get("responses_text") or status.get("chat_text"):
        return (
            "text_only",
            "Text generation works, but tool calling does not. That is not enough for the planned audit agent.",
        )

    return (
        "unsupported",
        "The API did not pass even the minimal text probe. Check base_url, api_key, model, and endpoint compatibility.",
    )


def print_human_report(
    base_url: str,
    model: str,
    results: Sequence[ProbeResult],
    verdict_code: str,
    verdict_text: str,
) -> None:
    print("API compatibility probe")
    print(f"base_url: {base_url}")
    print(f"model:    {model}")
    print()

    for result in results:
        mark = "PASS" if result.ok else "FAIL"
        status_text = result.http_status if result.http_status is not None else "n/a"
        print(f"[{mark}] {result.name}")
        print(f"  endpoint: {result.endpoint}")
        print(f"  http:     {status_text}")
        print(f"  latency:  {result.latency_ms} ms")
        print(f"  detail:   {result.detail}")
        if result.error:
            print(f"  error:    {result.error}")
        if result.payload_excerpt:
            print(f"  body:     {result.payload_excerpt}")
        print()

    print(f"verdict: {verdict_code}")
    print(f"summary: {verdict_text}")


def main() -> int:
    args = parse_args()

    if not args.base_url:
        print("missing --base-url or OPENAI_BASE_URL", file=sys.stderr)
        return 2
    if not args.api_key:
        print("missing --api-key or OPENAI_API_KEY", file=sys.stderr)
        return 2

    base_url = normalize_base_url(args.base_url)
    extra_headers = parse_extra_headers(args.extra_header)
    headers = build_headers(args.api_key, extra_headers)

    probes: List[ProbeResult] = []

    if not args.skip_responses:
        probes.append(probe_responses_text(base_url, args.model, headers, args.timeout))
        probes.append(probe_responses_tool(base_url, args.model, headers, args.timeout))
        probes.append(
            probe_responses_structured(base_url, args.model, headers, args.timeout)
        )

    if not args.skip_chat:
        probes.append(probe_chat_text(base_url, args.model, headers, args.timeout))
        probes.append(probe_chat_tool(base_url, args.model, headers, args.timeout))
        probes.append(probe_chat_structured(base_url, args.model, headers, args.timeout))

    verdict_code, verdict_text = classify(probes)

    if args.json:
        report = {
            "base_url": base_url,
            "model": args.model,
            "verdict": verdict_code,
            "summary": verdict_text,
            "results": [asdict(item) for item in probes],
        }
        print(json.dumps(report, indent=2, ensure_ascii=False))
    else:
        print_human_report(base_url, args.model, probes, verdict_code, verdict_text)

    return 0 if verdict_code not in {"text_only", "unsupported"} else 1


if __name__ == "__main__":
    sys.exit(main())
