from __future__ import annotations

import json
import re
from typing import Any, Dict, List, Optional, Tuple


ROLE_KEYWORDS = {
    "admin",
    "owner",
    "govern",
    "governance",
    "guardian",
    "pauser",
    "operator",
    "upgrader",
    "manager",
    "controller",
    "minter",
    "masterminter",
    "blacklister",
    "assetprotection",
    "proxyadmin",
}

SENSITIVE_NAME_PATTERNS = [
    (re.compile(r"upgrade", re.IGNORECASE), "upgrade-path"),
    (re.compile(r"admin", re.IGNORECASE), "admin-surface"),
    (re.compile(r"owner", re.IGNORECASE), "ownership-surface"),
    (re.compile(r"pause|unpause", re.IGNORECASE), "pause-control"),
    (re.compile(r"mint|burn", re.IGNORECASE), "supply-control"),
    (re.compile(r"blacklist|whitelist", re.IGNORECASE), "list-control"),
    (re.compile(r"set[A-Z_]|configure|initialize", re.IGNORECASE), "configuration"),
    (re.compile(r"grantRole|revokeRole", re.IGNORECASE), "role-management"),
]


def build_lightweight_ir(
    *, bundle_payload: Dict[str, Any], sources: Dict[str, str]
) -> Tuple[Dict[str, Any], Dict[str, Any], Dict[str, Any]]:
    contracts: List[Dict[str, Any]] = []
    functions: List[Dict[str, Any]] = []
    role_evidence: Dict[str, List[Dict[str, Any]]] = {}

    for path, source in sources.items():
        cleaned = strip_comments(source)
        for contract_info in extract_contracts(path, cleaned):
            contract_entry = {
                "name": contract_info["name"],
                "kind": contract_info["kind"],
                "file": path,
                "line_start": contract_info["line_start"],
                "bases": contract_info["bases"],
                "functions_count": 0,
                "modifiers": [],
                "state_variables": [],
                "role_signals": [],
            }

            top_level_items = extract_top_level_items(
                cleaned[contract_info["body_start"] : contract_info["body_end"]],
                absolute_offset=contract_info["body_start"],
                full_text=cleaned,
                contract_name=contract_info["name"],
                file_path=path,
            )

            modifier_names: List[str] = []
            state_vars: List[Dict[str, Any]] = []

            for item in top_level_items:
                if item["item_type"] == "function":
                    functions.append(item)
                    contract_entry["functions_count"] += 1
                elif item["item_type"] == "modifier":
                    modifier_names.append(item["name"])
                elif item["item_type"] == "state_variable":
                    state_vars.append(item)

            contract_entry["modifiers"] = modifier_names
            contract_entry["state_variables"] = state_vars

            role_signals = collect_contract_role_signals(
                contract_name=contract_info["name"],
                modifier_names=modifier_names,
                state_vars=state_vars,
                functions=[item for item in functions if item["contract"] == contract_info["name"] and item["file"] == path],
            )
            contract_entry["role_signals"] = sorted(role_signals)
            contracts.append(contract_entry)

            for role in role_signals:
                role_evidence.setdefault(role, []).append(
                    {
                        "contract": contract_info["name"],
                        "file": path,
                        "source": "contract-signal",
                    }
                )

            for modifier in modifier_names:
                for role in match_role_keywords(modifier):
                    role_evidence.setdefault(role, []).append(
                        {
                            "contract": contract_info["name"],
                            "file": path,
                            "source": "modifier",
                            "name": modifier,
                        }
                    )

            for state_var in state_vars:
                if is_noise_state_variable_name(state_var["name"]):
                    continue
                for role in match_role_keywords(state_var["name"]):
                    role_evidence.setdefault(role, []).append(
                        {
                            "contract": contract_info["name"],
                            "file": path,
                            "source": "state-variable",
                            "name": state_var["name"],
                        }
                    )

    analysis_target = bundle_payload.get("analysis_target")
    target_contract_name = ""
    if isinstance(analysis_target, dict):
        target_contract_name = str(analysis_target.get("contract_name") or "")
    if not target_contract_name:
        target_contract_name = (
            str(bundle_payload.get("contract", {}).get("name") or "")
            if isinstance(bundle_payload.get("contract"), dict)
            else ""
        )
    target_functions = [item for item in functions if item["contract"] == target_contract_name] if target_contract_name else []

    contracts_payload = {
        "target": bundle_payload.get("target", {}),
        "source_status": bundle_payload.get("status"),
        "proxy_resolution": bundle_payload.get("proxy_resolution", {}),
        "analysis_target": bundle_payload.get("analysis_target", {}),
        "contracts": contracts,
        "entry_contract": target_contract_name,
    }
    functions_payload = {
        "target": bundle_payload.get("target", {}),
        "analysis_target": bundle_payload.get("analysis_target", {}),
        "functions": functions,
        "entry_contract": target_contract_name,
        "entry_contract_functions": target_functions,
    }
    privilege_payload = {
        "target": bundle_payload.get("target", {}),
        "proxy_resolution": bundle_payload.get("proxy_resolution", {}),
        "analysis_target": bundle_payload.get("analysis_target", {}),
        "roles": [
            {
                "role": role,
                "evidence": evidence,
            }
            for role, evidence in sorted(role_evidence.items())
        ],
        "privileged_functions": [
            {
                "contract": item["contract"],
                "file": item["file"],
                "line_start": item["line_start"],
                "name": item["name"],
                "kind": item["function_kind"],
                "visibility": item["visibility"],
                "state_mutability": item["state_mutability"],
                "modifiers": item["modifiers"],
                "sensitivity_reasons": item["sensitivity_reasons"],
            }
            for item in functions
            if item["sensitivity_reasons"]
        ],
    }
    return contracts_payload, functions_payload, privilege_payload


def strip_comments(text: str) -> str:
    result: List[str] = []
    i = 0
    in_line_comment = False
    in_block_comment = False
    in_string: Optional[str] = None

    while i < len(text):
        ch = text[i]
        nxt = text[i + 1] if i + 1 < len(text) else ""

        if in_line_comment:
            if ch == "\n":
                in_line_comment = False
                result.append(ch)
            else:
                result.append(" ")
            i += 1
            continue

        if in_block_comment:
            if ch == "*" and nxt == "/":
                result.extend("  ")
                in_block_comment = False
                i += 2
            else:
                result.append("\n" if ch == "\n" else " ")
                i += 1
            continue

        if in_string:
            result.append(ch)
            if ch == "\\" and i + 1 < len(text):
                result.append(text[i + 1])
                i += 2
                continue
            if ch == in_string:
                in_string = None
            i += 1
            continue

        if ch == "/" and nxt == "/":
            result.extend("  ")
            in_line_comment = True
            i += 2
            continue

        if ch == "/" and nxt == "*":
            result.extend("  ")
            in_block_comment = True
            i += 2
            continue

        if ch in {'"', "'"}:
            in_string = ch
            result.append(ch)
            i += 1
            continue

        result.append(ch)
        i += 1

    return "".join(result)


def extract_contracts(file_path: str, cleaned_source: str) -> List[Dict[str, Any]]:
    pattern = re.compile(
        r"\b(contract|library|interface)\s+([A-Za-z_]\w*)(?:\s+is\s+([^{]+))?\s*\{"
    )
    contracts: List[Dict[str, Any]] = []
    for match in pattern.finditer(cleaned_source):
        open_brace_index = match.end() - 1
        close_brace_index = find_matching_brace(cleaned_source, open_brace_index)
        if close_brace_index is None:
            continue
        bases_raw = match.group(3) or ""
        bases = [item.strip() for item in bases_raw.split(",") if item.strip()]
        contracts.append(
            {
                "file": file_path,
                "kind": match.group(1),
                "name": match.group(2),
                "bases": bases,
                "line_start": line_number(cleaned_source, match.start()),
                "body_start": open_brace_index + 1,
                "body_end": close_brace_index,
            }
        )
    return contracts


def find_matching_brace(text: str, open_index: int) -> Optional[int]:
    depth = 0
    in_string: Optional[str] = None
    i = open_index
    while i < len(text):
        ch = text[i]
        if in_string:
            if ch == "\\" and i + 1 < len(text):
                i += 2
                continue
            if ch == in_string:
                in_string = None
            i += 1
            continue
        if ch in {'"', "'"}:
            in_string = ch
            i += 1
            continue
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
            if depth == 0:
                return i
        i += 1
    return None


def extract_top_level_items(
    contract_body: str,
    *,
    absolute_offset: int,
    full_text: str,
    contract_name: str,
    file_path: str,
) -> List[Dict[str, Any]]:
    items: List[Dict[str, Any]] = []
    i = 0
    seg_start: Optional[int] = None

    while i < len(contract_body):
        ch = contract_body[i]
        if seg_start is None:
            if ch.isspace():
                i += 1
                continue
            seg_start = i

        if ch == ";":
            header = contract_body[seg_start : i + 1]
            item = parse_top_level_header(
                header,
                absolute_offset + seg_start,
                full_text,
                contract_name,
                file_path,
            )
            if item is not None:
                items.append(item)
            seg_start = None
            i += 1
            continue

        if ch == "{":
            header = contract_body[seg_start:i]
            item = parse_top_level_header(
                header,
                absolute_offset + seg_start,
                full_text,
                contract_name,
                file_path,
            )
            if item is not None:
                items.append(item)
            end_index = find_matching_brace(contract_body, i)
            if end_index is None:
                break
            seg_start = None
            i = end_index + 1
            continue

        i += 1

    return items


def parse_top_level_header(
    header: str,
    absolute_index: int,
    full_text: str,
    contract_name: str,
    file_path: str,
) -> Optional[Dict[str, Any]]:
    normalized = " ".join(header.split()).strip()
    if not normalized:
        return None

    line_start = line_number(full_text, absolute_index)

    if normalized.startswith("function "):
        return parse_function_header(
            normalized, line_start=line_start, contract_name=contract_name, file_path=file_path
        )
    if normalized.startswith("constructor("):
        return parse_constructor_header(
            normalized, line_start=line_start, contract_name=contract_name, file_path=file_path
        )
    if normalized.startswith("modifier "):
        name_match = re.match(r"modifier\s+([A-Za-z_]\w*)", normalized)
        if not name_match:
            return None
        return {
            "item_type": "modifier",
            "name": name_match.group(1),
            "file": file_path,
            "contract": contract_name,
            "line_start": line_start,
        }
    if normalized.startswith(("event ", "struct ", "enum ", "using ")):
        return None

    state_var = parse_state_variable_header(
        normalized, line_start=line_start, contract_name=contract_name, file_path=file_path
    )
    return state_var


def parse_function_header(
    header: str, *, line_start: int, contract_name: str, file_path: str
) -> Dict[str, Any]:
    match = re.match(r"function\s*(?:(?P<name>[A-Za-z_]\w*)\s*)?\((?P<params>[^)]*)\)\s*(?P<tail>.*)", header)
    if not match:
        name = "fallback"
        params = ""
        tail = header
    else:
        name = match.group("name") or "fallback"
        params = match.group("params") or ""
        tail = match.group("tail") or ""

    parsed = classify_function_tail(name=name, tail=tail, contract_name=contract_name)
    return {
        "item_type": "function",
        "function_kind": parsed["function_kind"],
        "contract": contract_name,
        "file": file_path,
        "line_start": line_start,
        "name": name,
        "signature": f"{name}({normalize_params_for_signature(params)})",
        "params": normalize_params(params),
        "visibility": parsed["visibility"],
        "state_mutability": parsed["state_mutability"],
        "modifiers": parsed["modifiers"],
        "returns": parsed["returns"],
        "sensitivity_reasons": parsed["sensitivity_reasons"],
    }


def parse_constructor_header(
    header: str, *, line_start: int, contract_name: str, file_path: str
) -> Dict[str, Any]:
    match = re.match(r"constructor\((?P<params>[^)]*)\)\s*(?P<tail>.*)", header)
    params = match.group("params") if match else ""
    tail = match.group("tail") if match else ""
    parsed = classify_function_tail(name="constructor", tail=tail, contract_name=contract_name)
    return {
        "item_type": "function",
        "function_kind": "constructor",
        "contract": contract_name,
        "file": file_path,
        "line_start": line_start,
        "name": "constructor",
        "signature": f"constructor({normalize_params_for_signature(params)})",
        "params": normalize_params(params),
        "visibility": parsed["visibility"],
        "state_mutability": parsed["state_mutability"],
        "modifiers": parsed["modifiers"],
        "returns": parsed["returns"],
        "sensitivity_reasons": parsed["sensitivity_reasons"],
    }


def classify_function_tail(name: str, tail: str, contract_name: str) -> Dict[str, Any]:
    returns_match = re.search(r"returns\s*\(([^)]*)\)", tail)
    returns = normalize_params(returns_match.group(1)) if returns_match else []
    tail_without_returns = re.sub(r"returns\s*\([^)]*\)", " ", tail)
    tokens = [token for token in re.split(r"\s+", tail_without_returns.strip()) if token]

    visibility = ""
    state_mutability = ""
    modifiers: List[str] = []
    function_kind = "function"

    for token in tokens:
        if token in {"public", "external", "internal", "private"}:
            visibility = token
        elif token in {"view", "pure", "payable", "constant"}:
            state_mutability = token
        elif token in {"virtual", "override", "memory", "calldata"}:
            continue
        elif re.match(r"^[A-Za-z_]\w*$", token):
            modifiers.append(token)

    if name == "fallback":
        function_kind = "fallback"
    elif name == "constructor" or name == contract_name:
        function_kind = "constructor"

    sensitivity_reasons = classify_function_sensitivity(
        function_name=name,
        modifiers=modifiers,
        visibility=visibility,
        state_mutability=state_mutability,
    )

    return {
        "visibility": visibility,
        "state_mutability": state_mutability,
        "modifiers": modifiers,
        "returns": returns,
        "function_kind": function_kind,
        "sensitivity_reasons": sensitivity_reasons,
    }


def parse_state_variable_header(
    header: str, *, line_start: int, contract_name: str, file_path: str
) -> Optional[Dict[str, Any]]:
    candidate = header.rstrip(";").strip()
    if not candidate or candidate.startswith(("mapping(", "mapping (")):
        return parse_mapping_state_variable(
            candidate, line_start=line_start, contract_name=contract_name, file_path=file_path
        )

    lhs = candidate.split("=", 1)[0].strip()
    tokens = lhs.split()
    if len(tokens) < 2:
        return None
    name = tokens[-1]
    if not re.match(r"^[A-Za-z_]\w*$", name):
        return None
    attr_keywords = {"public", "private", "internal", "external", "constant", "immutable"}
    attrs: List[str] = []
    type_tokens = tokens[:-1]
    while type_tokens and type_tokens[-1] in attr_keywords:
        attrs.insert(0, type_tokens.pop())
    if not type_tokens:
        return None
    return {
        "item_type": "state_variable",
        "contract": contract_name,
        "file": file_path,
        "line_start": line_start,
        "type": " ".join(type_tokens),
        "name": name,
        "attributes": attrs,
    }


def parse_mapping_state_variable(
    candidate: str, *, line_start: int, contract_name: str, file_path: str
) -> Optional[Dict[str, Any]]:
    match = re.match(
        r"^(?P<type>mapping\s*\([^)]*\))\s+"
        r"(?P<attrs>(?:(?:public|private|internal|external|constant|immutable)\s+)*)"
        r"(?P<name>[A-Za-z_]\w*)"
        r"(?:\s*=.*)?$",
        candidate,
    )
    if not match:
        return None

    attrs = [token for token in match.group("attrs").split() if token]
    return {
        "item_type": "state_variable",
        "contract": contract_name,
        "file": file_path,
        "line_start": line_start,
        "type": match.group("type").strip(),
        "name": match.group("name"),
        "attributes": attrs,
    }


def normalize_params(params: str) -> List[Dict[str, str]]:
    result: List[Dict[str, str]] = []
    for raw in split_params(params):
        parts = raw.strip().split()
        if not parts:
            continue
        if len(parts) == 1:
            result.append({"type": parts[0], "name": ""})
        else:
            result.append({"type": " ".join(parts[:-1]), "name": parts[-1]})
    return result


def normalize_params_for_signature(params: str) -> str:
    return ",".join(item["type"] for item in normalize_params(params))


def split_params(params: str) -> List[str]:
    items: List[str] = []
    current: List[str] = []
    depth = 0
    for ch in params:
        if ch == "," and depth == 0:
            item = "".join(current).strip()
            if item:
                items.append(item)
            current = []
            continue
        if ch in "([{":
            depth += 1
        elif ch in ")]}":
            depth = max(0, depth - 1)
        current.append(ch)
    item = "".join(current).strip()
    if item:
        items.append(item)
    return items


def classify_function_sensitivity(
    *, function_name: str, modifiers: List[str], visibility: str, state_mutability: str
) -> List[str]:
    reasons: List[str] = []
    if visibility in {"public", "external"} and state_mutability not in {"view", "pure", "constant"}:
        for pattern, reason in SENSITIVE_NAME_PATTERNS:
            if pattern.search(function_name):
                reasons.append(reason)

    for modifier in modifiers:
        for role in match_role_keywords(modifier):
            reasons.append(f"guarded-by-{role}")

    if function_name == "fallback":
        reasons.append("fallback-entrypoint")

    deduped: List[str] = []
    for reason in reasons:
        if reason not in deduped:
            deduped.append(reason)
    return deduped


def collect_contract_role_signals(
    *,
    contract_name: str,
    modifier_names: List[str],
    state_vars: List[Dict[str, Any]],
    functions: List[Dict[str, Any]],
) -> List[str]:
    roles = set(match_role_keywords(contract_name))
    for modifier in modifier_names:
        roles.update(match_role_keywords(modifier))
    for state_var in state_vars:
        if is_noise_state_variable_name(state_var["name"]):
            continue
        roles.update(match_role_keywords(state_var["name"]))
    for function in functions:
        roles.update(match_role_keywords(function["name"]))
        for modifier in function["modifiers"]:
            roles.update(match_role_keywords(modifier))
    return sorted(roles)


def match_role_keywords(value: str) -> List[str]:
    lowered = value.replace("_", "").lower()
    matches = [keyword for keyword in ROLE_KEYWORDS if keyword in lowered]
    return sorted(set(matches))


def is_noise_state_variable_name(name: str) -> bool:
    return name.isupper() or name.endswith("_SLOT")


def line_number(text: str, index: int) -> int:
    return text.count("\n", 0, index) + 1


def load_bundle_payload(raw_text: str) -> Dict[str, Any]:
    parsed = json.loads(raw_text)
    if not isinstance(parsed, dict):
        raise ValueError("bundle payload must be a JSON object")
    return parsed
