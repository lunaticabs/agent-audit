from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import PurePosixPath
from typing import Any, Dict, List, Optional
from urllib import error, parse, request


CHAIN_ID_ALIASES = {
    "1": "1",
    "eth": "1",
    "ethereum": "1",
    "mainnet": "1",
    "base": "8453",
    "8453": "8453",
    "arb": "42161",
    "arbitrum": "42161",
    "42161": "42161",
    "op": "10",
    "optimism": "10",
    "10": "10",
    "polygon": "137",
    "matic": "137",
    "137": "137",
    "bsc": "56",
    "56": "56",
    "sepolia": "11155111",
    "11155111": "11155111",
}


@dataclass
class SourceFile:
    path: str
    content: str


@dataclass
class SourceBundle:
    provider_payload: Dict[str, Any]
    normalized_payload: Dict[str, Any]
    files: List[SourceFile]


def chain_to_chain_id(chain: str) -> str:
    normalized = chain.strip().lower()
    return CHAIN_ID_ALIASES.get(normalized, normalized)


def normalize_api_endpoint(base_url: str) -> str:
    base_url = base_url.strip().rstrip("/")
    parsed = parse.urlparse(base_url)
    path = parsed.path.rstrip("/")

    if path.endswith("/v2/api") or path.endswith("/api"):
        return base_url

    if path:
        path = f"{path}/v2/api"
    else:
        path = "/v2/api"
    return parse.urlunparse(parsed._replace(path=path))


def fetch_verified_source(
    *,
    base_url: str,
    api_key: Optional[str],
    headers: Dict[str, str],
    address: str,
    chain: str,
    timeout: float = 30.0,
) -> SourceBundle:
    chain_id = chain_to_chain_id(chain)
    endpoint = normalize_api_endpoint(base_url)
    params = {
        "module": "contract",
        "action": "getsourcecode",
        "address": address,
        "chainid": chain_id,
    }
    if api_key:
        params["apikey"] = api_key

    url = f"{endpoint}?{parse.urlencode(params)}"
    req = request.Request(
        url=url,
        headers={
            "Accept": "application/json",
            **headers,
        },
        method="GET",
    )

    try:
        with request.urlopen(req, timeout=timeout) as resp:
            payload = json.loads(resp.read().decode("utf-8"))
    except error.HTTPError as exc:
        body = exc.read().decode("utf-8", errors="replace")
        raise RuntimeError(
            f"source API request failed with HTTP {exc.code}: {body[:300]}"
        ) from exc
    except error.URLError as exc:
        raise RuntimeError(f"source API request failed: {exc.reason}") from exc

    status = str(payload.get("status", ""))
    message = str(payload.get("message", ""))
    result = payload.get("result")
    if status != "1" or not isinstance(result, list) or not result:
        raise RuntimeError(
            f"source API returned an unusable payload: status={status!r} message={message!r}"
        )

    primary = result[0]
    if not isinstance(primary, dict):
        raise RuntimeError("source API returned an unexpected result shape")

    files, source_layout, source_meta = parse_source_code_result(primary)
    normalized = {
        "target": {
            "address": address,
            "chain": chain,
            "chain_id": chain_id,
        },
        "provider": {
            "type": "etherscan-compatible",
            "endpoint": endpoint,
            "message": message,
            "result_count": len(result),
        },
        "contract": {
            "name": primary.get("ContractName") or "",
            "file_name": primary.get("ContractFileName") or "",
            "proxy": str(primary.get("Proxy", "0")) == "1",
            "implementation": primary.get("Implementation") or "",
            "similar_match": primary.get("SimilarMatch") or "",
        },
        "compiler": {
            "version": primary.get("CompilerVersion") or "",
            "type": primary.get("CompilerType") or "",
            "optimization_used": primary.get("OptimizationUsed") or "",
            "runs": primary.get("Runs") or "",
            "evm_version": primary.get("EVMVersion") or "",
            "constructor_arguments": primary.get("ConstructorArguments") or "",
            "license_type": primary.get("LicenseType") or "",
            "library": primary.get("Library") or "",
            "swarm_source": primary.get("SwarmSource") or "",
        },
        "abi": parse_json_string(primary.get("ABI")),
        "source_layout": source_layout,
        "source_meta": source_meta,
        "files": [{"path": item.path, "length": len(item.content)} for item in files],
    }
    return SourceBundle(
        provider_payload=payload,
        normalized_payload=normalized,
        files=files,
    )


def parse_source_code_result(result: Dict[str, Any]) -> tuple[List[SourceFile], str, Dict[str, Any]]:
    raw_source = str(result.get("SourceCode") or "")
    contract_name = str(result.get("ContractName") or "Contract")
    contract_file_name = str(result.get("ContractFileName") or "")

    stripped = raw_source.strip()
    if stripped.startswith("{{") and stripped.endswith("}}"):
        stripped = stripped[1:-1].strip()

    parsed_json = parse_json_string(stripped)
    if isinstance(parsed_json, dict) and isinstance(parsed_json.get("sources"), dict):
        files: List[SourceFile] = []
        for raw_path, source_entry in parsed_json["sources"].items():
            if not isinstance(raw_path, str):
                continue
            content = extract_source_content(source_entry)
            if content is None:
                continue
            files.append(
                SourceFile(
                    path=sanitize_source_path(raw_path),
                    content=content,
                )
            )

        layout = "standard-json"
        meta = {
            "language": parsed_json.get("language") or "",
            "settings": parsed_json.get("settings") or {},
        }
        if files:
            return files, layout, meta

    filename = contract_file_name or f"{contract_name}.sol"
    extension = guess_extension(result, filename)
    if not filename.endswith(extension):
        filename = f"{filename}{extension}"

    return (
        [SourceFile(path=sanitize_source_path(filename), content=raw_source)],
        "flattened",
        {},
    )


def parse_json_string(raw: Any) -> Optional[Any]:
    if not isinstance(raw, str):
        return None
    text = raw.strip()
    if not text:
        return None
    try:
        return json.loads(text)
    except json.JSONDecodeError:
        return None


def extract_source_content(source_entry: Any) -> Optional[str]:
    if isinstance(source_entry, dict):
        content = source_entry.get("content")
        if isinstance(content, str):
            return content
    elif isinstance(source_entry, str):
        return source_entry
    return None


def guess_extension(result: Dict[str, Any], filename: str) -> str:
    lowered = filename.lower()
    if lowered.endswith((".sol", ".vy", ".yul")):
        return ""

    compiler_type = str(result.get("CompilerType") or "").lower()
    if "vyper" in compiler_type:
        return ".vy"
    return ".sol"


def sanitize_source_path(raw_path: str) -> str:
    parts = []
    for part in PurePosixPath(raw_path).parts:
        if part in {"", ".", "..", "/"}:
            continue
        parts.append(part)
    return "/".join(parts) if parts else "Contract.sol"
