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
    "holesky": "17000",
    "17000": "17000",
    "hoodi": "560048",
    "560048": "560048",
    "sepolia": "11155111",
    "11155111": "11155111",
    "bsc": "56",
    "bnb": "56",
    "bnbsmartchain": "56",
    "binancesmartchain": "56",
    "56": "56",
    "bsctestnet": "97",
    "bnbtestnet": "97",
    "97": "97",
    "polygon": "137",
    "matic": "137",
    "polygonmainnet": "137",
    "137": "137",
    "amoy": "80002",
    "polygonamoy": "80002",
    "80002": "80002",
    "base": "8453",
    "basemainnet": "8453",
    "8453": "8453",
    "basesepolia": "84532",
    "84532": "84532",
    "arb": "42161",
    "arbone": "42161",
    "arbitrum": "42161",
    "arbitrumone": "42161",
    "42161": "42161",
    "arbnova": "42170",
    "arbitrumnova": "42170",
    "42170": "42170",
    "arbsepolia": "421614",
    "arbitrumsepolia": "421614",
    "421614": "421614",
    "op": "10",
    "optimism": "10",
    "opmainnet": "10",
    "10": "10",
    "opsepolia": "11155420",
    "optimismsepolia": "11155420",
    "11155420": "11155420",
    "avalanche": "43114",
    "avax": "43114",
    "avalanchecchain": "43114",
    "43114": "43114",
    "fuji": "43113",
    "avalanchefuji": "43113",
    "43113": "43113",
    "linea": "59144",
    "59144": "59144",
    "lineasepolia": "59141",
    "59141": "59141",
    "blast": "81457",
    "81457": "81457",
    "blastsepolia": "168587773",
    "168587773": "168587773",
    "scroll": "534352",
    "534352": "534352",
    "scrollsepolia": "534351",
    "534351": "534351",
    "mantle": "5000",
    "5000": "5000",
    "mantlesepolia": "5003",
    "5003": "5003",
    "gnosis": "100",
    "xdai": "100",
    "100": "100",
    "celo": "42220",
    "42220": "42220",
    "celosepolia": "11142220",
    "11142220": "11142220",
    "zksync": "324",
    "zksyncmainnet": "324",
    "324": "324",
    "zksyncsepolia": "300",
    "300": "300",
    "opbnb": "204",
    "204": "204",
    "opbnbtestnet": "5611",
    "5611": "5611",
    "moonbeam": "1284",
    "1284": "1284",
    "moonriver": "1285",
    "1285": "1285",
    "moonbasealpha": "1287",
    "1287": "1287",
    "bittorrent": "199",
    "btt": "199",
    "199": "199",
    "btttestnet": "1029",
    "1029": "1029",
    "fraxtal": "252",
    "252": "252",
    "fraxtalhoodi": "2523",
    "2523": "2523",
    "sonic": "146",
    "146": "146",
    "sonictestnet": "14601",
    "14601": "14601",
    "sei": "1329",
    "1329": "1329",
    "seitestnet": "1328",
    "1328": "1328",
    "taiko": "167000",
    "167000": "167000",
    "taikohoodi": "167013",
    "167013": "167013",
    "unichain": "130",
    "130": "130",
    "unichainsepolia": "1301",
    "1301": "1301",
    "world": "480",
    "worldchain": "480",
    "480": "480",
    "worldsepolia": "4801",
    "4801": "4801",
    "xdc": "50",
    "50": "50",
    "xdcapothem": "51",
    "51": "51",
    "abstract": "2741",
    "2741": "2741",
    "abstractsepolia": "11124",
    "11124": "11124",
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
    normalized = normalize_chain_alias(chain)
    return CHAIN_ID_ALIASES.get(normalized, normalized)


def normalize_chain_alias(chain: str) -> str:
    lowered = chain.strip().lower()
    return "".join(char for char in lowered if char.isalnum())


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
