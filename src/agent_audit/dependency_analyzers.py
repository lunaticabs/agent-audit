from __future__ import annotations

import re
from pathlib import Path
from typing import Any, Dict, List, Optional


def analyze_dependencies(bundle_payload: Dict[str, Any], workspace_root: Path) -> List[Dict[str, Any]]:
    findings: List[Dict[str, Any]] = []
    dependencies = bundle_payload.get("dependencies")
    if not isinstance(dependencies, list):
        return findings

    verifier_records: List[Dict[str, Any]] = []
    for dependency in dependencies:
        if not isinstance(dependency, dict):
            continue
        if str(dependency.get("role") or "") == "verifier":
            verifier_records.append(dependency)
        else:
            findings.extend(analyze_dependency_record(dependency, workspace_root))

    if verifier_records:
        findings.extend(analyze_verifier_group(bundle_payload, verifier_records, workspace_root))
    return findings


def analyze_dependency_record(record: Dict[str, Any], workspace_root: Path) -> List[Dict[str, Any]]:
    findings: List[Dict[str, Any]] = []

    related = record.get("related_contracts")
    if isinstance(related, list):
        for nested in related:
            if isinstance(nested, dict):
                findings.extend(analyze_dependency_record(nested, workspace_root))
    return findings


def analyze_verifier_group(
    bundle_payload: Dict[str, Any],
    records: List[Dict[str, Any]],
    workspace_root: Path,
) -> List[Dict[str, Any]]:
    findings: List[Dict[str, Any]] = []
    expected_arities = extract_expected_verifier_arities(bundle_payload, workspace_root)
    gamma_delta_hits: List[Dict[str, Any]] = []

    for record in records:
        if record.get("status") != "fetched":
            continue
        role_findings = analyze_single_verifier_record(
            record,
            workspace_root,
            expected_arities,
        )
        for item in role_findings:
            if item.get("title") == "verifier-gamma-delta-equality":
                gamma_delta_hits.append(item)
            else:
                findings.append(item)

    if gamma_delta_hits:
        findings.append(group_gamma_delta_findings(gamma_delta_hits))
    return findings


def analyze_single_verifier_record(
    record: Dict[str, Any],
    workspace_root: Path,
    expected_arities: Dict[str, int],
) -> List[Dict[str, Any]]:
    findings: List[Dict[str, Any]] = []
    abi_arity = verifier_abi_pubsignal_arity(record)
    expected_arity = expected_verifier_arity(record, expected_arities)
    if expected_arity is not None and abi_arity is not None and expected_arity != abi_arity:
        findings.append(
            {
                "title": "verifier-pubsignals-arity-mismatch",
                "severity": "critical",
                "confidence": "high",
                "summary": (
                    f"Main-contract interface expects {expected_arity} public signals for {record.get('name')}, "
                    f"but the fetched verifier ABI exposes {abi_arity}. This indicates an interface/verifier mismatch "
                    "on a critical proof-validation path."
                ),
                "source": "dependency-verifier",
                "location": "",
                "evidence_artifacts": verifier_evidence_artifacts(record),
            }
        )

    for file_entry in record.get("files", []):
        if not isinstance(file_entry, dict):
            continue
        relative_path = file_entry.get("path")
        if not isinstance(relative_path, str) or not relative_path:
            continue
        source_path = workspace_root / "sources" / relative_path
        if not source_path.exists():
            continue
        findings.extend(analyze_verifier_source_file(record, relative_path, source_path.read_text()))
    return findings


def analyze_verifier_source_file(
    record: Dict[str, Any], relative_path: str, source_text: str
) -> List[Dict[str, Any]]:
    findings: List[Dict[str, Any]] = []
    constants = extract_uint_constants(source_text)
    gamma = tuple(constants.get(name, ("", 0))[0] for name in ["gammax1", "gammax2", "gammay1", "gammay2"])
    delta = tuple(constants.get(name, ("", 0))[0] for name in ["deltax1", "deltax2", "deltay1", "deltay2"])
    if all(gamma) and gamma == delta:
        location_line = constants.get("gammax1", ("", 1))[1]
        findings.append(
            {
                "title": "verifier-gamma-delta-equality",
                "severity": "critical",
                "confidence": "high",
                "summary": (
                    f"Verifier {record.get('name') or record.get('address')} defines identical gamma and delta G2 points. "
                    "In Groth16-style verifiers this is a high-signal anomaly that can break input binding "
                    "and should be treated as a critical manual-review item."
                ),
                "source": "dependency-verifier",
                "location": f"{relative_path}:{location_line}",
                "evidence_artifacts": [f"sources/{relative_path}"],
            }
        )
    return findings


def group_gamma_delta_findings(findings: List[Dict[str, Any]]) -> Dict[str, Any]:
    affected = [item.get("location", "") for item in findings if isinstance(item, dict)]
    evidence: List[str] = []
    for item in findings:
        for artifact in item.get("evidence_artifacts", []):
            if artifact not in evidence:
                evidence.append(artifact)
    preview = "; ".join(affected[:4])
    if len(affected) > 4:
        preview += f"; and {len(affected) - 4} more"
    return {
        "title": "verifier-gamma-delta-equality-systemic",
        "severity": "critical",
        "confidence": "high",
        "summary": (
            f"Multiple fetched Groth16 verifier contracts define identical gamma and delta G2 points, "
            f"suggesting a systemic proof-binding flaw across the verifier set. Affected locations include {preview}."
        ),
        "source": "dependency-verifier",
        "location": affected[0] if affected else "",
        "evidence_artifacts": evidence,
    }


def extract_uint_constants(source_text: str) -> Dict[str, tuple[str, int]]:
    constants: Dict[str, tuple[str, int]] = {}
    pattern = re.compile(
        r"uint(?:256)?\s+constant\s+([A-Za-z_]\w*)\s*=\s*([0-9xa-fA-F]+)\s*;"
    )
    for idx, line in enumerate(source_text.splitlines(), start=1):
        match = pattern.search(line)
        if not match:
            continue
        constants[match.group(1).lower()] = (match.group(2).lower(), idx)
    return constants


def extract_expected_verifier_arities(bundle_payload: Dict[str, Any], workspace_root: Path) -> Dict[str, int]:
    expected: Dict[str, int] = {}
    for file_entry in bundle_payload.get("files", []):
        if not isinstance(file_entry, dict):
            continue
        relative_path = file_entry.get("path")
        if not isinstance(relative_path, str):
            continue
        path = workspace_root / "sources" / relative_path
        if not path.exists():
            continue
        text = path.read_text()
        interface_pattern = re.compile(
            r"interface\s+([A-Za-z_]\w*)\s*\{[^}]*?function\s+verifyProof\s*\([^)]*uint\[(\d+)\]\s+calldata\s+_pubSignals",
            re.DOTALL,
        )
        for match in interface_pattern.finditer(text):
            expected[match.group(1)] = int(match.group(2))
    return expected


def verifier_abi_pubsignal_arity(record: Dict[str, Any]) -> Optional[int]:
    abi = record.get("abi")
    if not isinstance(abi, list):
        return None
    for item in abi:
        if not isinstance(item, dict) or item.get("name") != "verifyProof":
            continue
        inputs = item.get("inputs")
        if not isinstance(inputs, list):
            continue
        for arg in inputs:
            if not isinstance(arg, dict):
                continue
            name = str(arg.get("name") or "")
            internal_type = str(arg.get("internalType") or "")
            if name != "_pubSignals" and "pubSignals" not in internal_type:
                continue
            match = re.search(r"uint(?:256)?\[(\d+)\]", internal_type)
            if match:
                return int(match.group(1))
    return None


def expected_verifier_arity(record: Dict[str, Any], expected: Dict[str, int]) -> Optional[int]:
    discovery = record.get("discovery")
    if not isinstance(discovery, dict):
        return None
    internal_type = str(discovery.get("internal_type") or "")
    match = re.search(r"contract\s+([A-Za-z_]\w*)", internal_type)
    if not match:
        return None
    return expected.get(match.group(1))


def verifier_evidence_artifacts(record: Dict[str, Any]) -> List[str]:
    artifacts: List[str] = []
    for file_entry in record.get("files", []):
        if not isinstance(file_entry, dict):
            continue
        path = file_entry.get("path")
        if isinstance(path, str):
            artifacts.append(f"sources/{path}")
    return artifacts
