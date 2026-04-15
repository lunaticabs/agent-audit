from __future__ import annotations

import re
from typing import Any, Dict, List, Optional


ROLE_HINTS = [
    ("verifyproof", "verifier"),
    ("withdraw", "verifier"),
    ("cancel", "verifier"),
    ("update", "verifier"),
    ("proof", "verifier"),
    ("router", "router"),
    ("swap", "router"),
    ("uniswap", "router"),
    ("token", "token"),
    ("erc20", "token"),
    ("oracle", "oracle"),
    ("price", "oracle"),
    ("aggregator", "oracle"),
    ("admin", "access-control"),
    ("owner", "access-control"),
    ("govern", "access-control"),
    ("controller", "controller"),
    ("vault", "vault"),
    ("pool", "pool"),
    ("bridge", "bridge"),
    ("messenger", "bridge"),
    ("weth", "token"),
]


def discover_dependencies(bundle_payload: Dict[str, Any], sources: Dict[str, str]) -> Dict[str, Any]:
    constructor_candidates = discover_constructor_dependencies(bundle_payload)
    constant_candidates = discover_source_constant_dependencies(sources)
    merged = merge_dependency_candidates(constructor_candidates, constant_candidates)
    return {
        "constructor_candidates": constructor_candidates,
        "constant_candidates": constant_candidates,
        "merged_candidates": merged,
    }


def discover_constructor_dependencies(bundle_payload: Dict[str, Any]) -> List[Dict[str, Any]]:
    abi = bundle_payload.get("abi")
    if not isinstance(abi, list):
        return []

    constructor_abi = None
    for item in abi:
        if isinstance(item, dict) and item.get("type") == "constructor":
            constructor_abi = item
            break

    if not isinstance(constructor_abi, dict):
        return []

    inputs = constructor_abi.get("inputs")
    if not isinstance(inputs, list):
        return []

    constructor_args = ""
    compiler = bundle_payload.get("compiler")
    if isinstance(compiler, dict):
        constructor_args = str(compiler.get("constructor_arguments") or "")

    decoded = decode_static_constructor_arguments(inputs, constructor_args)
    candidates: List[Dict[str, Any]] = []
    for item in decoded:
        if item.get("type") != "address":
            continue
        address = str(item.get("value") or "")
        if not is_valid_address(address):
            continue
        role = classify_dependency_role(
            name=str(item.get("name") or ""),
            internal_type=str(item.get("internal_type") or ""),
        )
        candidates.append(
            {
                "address": address.lower(),
                "name": str(item.get("name") or ""),
                "role": role,
                "source": "constructor",
                "internal_type": str(item.get("internal_type") or ""),
                "solidity_type": str(item.get("solidity_type") or ""),
            }
        )
    return candidates


def decode_static_constructor_arguments(
    inputs: List[Dict[str, Any]], constructor_args_hex: str
) -> List[Dict[str, Any]]:
    hex_body = constructor_args_hex.strip().removeprefix("0x")
    if not hex_body:
        return []

    decoded: List[Dict[str, Any]] = []
    offset = 0
    for item in inputs:
        if not isinstance(item, dict):
            continue
        solidity_type = str(item.get("type") or "")
        if not is_static_abi_type(solidity_type):
            break
        if offset + 64 > len(hex_body):
            break
        word = hex_body[offset : offset + 64]
        offset += 64
        decoded.append(
            {
                "name": str(item.get("name") or ""),
                "internal_type": str(item.get("internalType") or ""),
                "solidity_type": solidity_type,
                "type": classify_abi_type(solidity_type),
                "value": decode_static_word(solidity_type, word),
            }
        )
    return decoded


def discover_source_constant_dependencies(sources: Dict[str, str]) -> List[Dict[str, Any]]:
    candidates: List[Dict[str, Any]] = []
    pattern = re.compile(
        r"address(?:\s+payable)?(?:\s+(?:public|private|internal|external|constant|immutable))*\s+"
        r"(?P<name>[A-Za-z_]\w*)\s*=\s*(?:address\s*\()?(?P<addr>0x[a-fA-F0-9]{40})(?:\))?",
        re.MULTILINE,
    )
    for path, source in sources.items():
        for match in pattern.finditer(source):
            address = match.group("addr").lower()
            name = match.group("name")
            if not is_valid_address(address):
                continue
            candidates.append(
                {
                    "address": address,
                    "name": name,
                    "role": classify_dependency_role(name=name),
                    "source": "source_constant",
                    "file": path,
                }
            )
    return candidates


def merge_dependency_candidates(*groups: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
    merged: List[Dict[str, Any]] = []
    by_address: Dict[str, Dict[str, Any]] = {}

    for group in groups:
        for item in group:
            if not isinstance(item, dict):
                continue
            address = str(item.get("address") or "").lower()
            if not is_valid_address(address):
                continue
            existing = by_address.get(address)
            if existing is None:
                clone = dict(item)
                clone["address"] = address
                clone["sources"] = [str(item.get("source") or "unknown")]
                by_address[address] = clone
                merged.append(clone)
                continue

            sources = existing.setdefault("sources", [])
            source_name = str(item.get("source") or "unknown")
            if source_name not in sources:
                sources.append(source_name)
            if not existing.get("name") and item.get("name"):
                existing["name"] = item["name"]
            if existing.get("role") in {"dependency", "unknown"}:
                existing["role"] = item.get("role") or existing.get("role")
            if item.get("internal_type") and not existing.get("internal_type"):
                existing["internal_type"] = item["internal_type"]
            if item.get("file") and not existing.get("file"):
                existing["file"] = item["file"]
    return merged


def classify_dependency_role(name: str = "", internal_type: str = "") -> str:
    haystack = f"{name} {internal_type}".replace("_", "").lower()
    for needle, role in ROLE_HINTS:
        if needle in haystack:
            return role
    if "contract " in internal_type.lower():
        return "contract-dependency"
    return "dependency"


def is_static_abi_type(solidity_type: str) -> bool:
    if not solidity_type:
        return False
    if solidity_type.endswith("]"):
        return False
    if solidity_type in {"string", "bytes"}:
        return False
    return True


def classify_abi_type(solidity_type: str) -> str:
    lowered = solidity_type.lower()
    if lowered == "address":
        return "address"
    if lowered == "bool":
        return "bool"
    if lowered.startswith("uint") or lowered.startswith("int"):
        return "int"
    if lowered.startswith("bytes"):
        return "bytes"
    return "word"


def decode_static_word(solidity_type: str, word_hex: str) -> str:
    lowered = solidity_type.lower()
    if lowered == "address":
        return "0x" + word_hex[-40:].lower()
    if lowered == "bool":
        return "true" if int(word_hex, 16) else "false"
    if lowered.startswith("uint") or lowered.startswith("int"):
        return str(int(word_hex, 16))
    return "0x" + word_hex.lower()


def is_valid_address(value: str) -> bool:
    return bool(re.fullmatch(r"0x[a-fA-F0-9]{40}", value))
