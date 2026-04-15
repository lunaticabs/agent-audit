from __future__ import annotations

from dataclasses import asdict
import json
from pathlib import Path
from typing import Any, Dict, List

from agent_audit.config import AppConfig
from agent_audit.dependency_analyzers import analyze_dependencies
from agent_audit.dependency_discovery import discover_dependencies
from agent_audit.schemas import ArtifactRecord
from agent_audit.source_fetch import fetch_verified_source
from agent_audit.source_ir import build_lightweight_ir, load_bundle_payload
from agent_audit.workspace import RunWorkspace


class AuditPipelineService:
    def __init__(self, config: AppConfig, workspace: RunWorkspace):
        self.config = config
        self.workspace = workspace
        self.artifacts: List[ArtifactRecord] = self._load_existing_artifacts()

    def fetch_contract_source(self, address: str, chain: str) -> str:
        request_payload = {
            "address": address,
            "chain": chain,
            "source_api_base": self.config.source_api_base,
            "source_api_configured": bool(self.config.source_api_base),
            "source_api_header_names": sorted(self.config.source_api_headers.keys()),
            "rpc_url_configured": bool(self.config.rpc_url),
        }
        request_path = self.workspace.write_json("input/source_request.json", request_payload)

        if not self.config.source_api_base:
            bundle_path = self.workspace.write_json(
                "artifacts/source_bundle.json",
                {
                    "target": {"address": address, "chain": chain},
                    "contracts": [],
                    "proxy_resolution": {"status": "not_attempted"},
                    "status": "source_api_not_configured",
                    "note": "Configure AGENT_AUDIT_SOURCE_API_BASE to enable verified source fetching.",
                },
            )
            self._record(
                step="fetch_contract_source",
                path=request_path,
                kind="request",
                status="configured_not_executed",
                summary="Persisted source fetch request metadata.",
            )
            self._record(
                step="fetch_contract_source",
                path=bundle_path,
                kind="artifact",
                status="configured_not_executed",
                summary="Skipped source fetch because the source API is not configured.",
            )
            return "source_api_not_configured"

        try:
            bundle = fetch_verified_source(
                base_url=self.config.source_api_base,
                api_key=self.config.source_api_key,
                headers=self.config.source_api_headers,
                address=address,
                chain=chain,
            )
        except Exception as exc:
            bundle_path = self.workspace.write_json(
                "artifacts/source_bundle.json",
                {
                    "target": {"address": address, "chain": chain},
                    "contracts": [],
                    "proxy_resolution": {"status": "not_attempted"},
                    "status": "source_fetch_failed",
                    "error": str(exc),
                },
            )
            self._record(
                step="fetch_contract_source",
                path=request_path,
                kind="request",
                status="executed_with_error",
                summary="Persisted source fetch request metadata.",
            )
            self._record(
                step="fetch_contract_source",
                path=bundle_path,
                kind="artifact",
                status="executed_with_error",
                summary="Source fetch failed; inspect the stored error payload.",
            )
            return "source_fetch_failed"

        proxy_contract = bundle.normalized_payload.get("contract", {})
        implementation_address = (
            str(proxy_contract.get("implementation") or "").strip()
            if isinstance(proxy_contract, dict)
            else ""
        )

        raw_response_path = self.workspace.write_json(
            "artifacts/source_provider_response.json",
            bundle.provider_payload,
        )
        primary_sources = self._write_fetched_source_files(
            bundle.files,
            prefix="",
            summary_prefix="Stored a fetched source file.",
        )

        related_contracts: List[Dict[str, Any]] = []
        if (
            isinstance(proxy_contract, dict)
            and proxy_contract.get("proxy")
            and implementation_address
            and implementation_address.lower() != address.lower()
        ):
            implementation_record = self._fetch_dependency_bundle_record(
                address=implementation_address,
                chain=chain,
                role="implementation",
                name="implementation",
                prefix="implementation",
            )
            related_contracts.append(implementation_record)

        source_map_for_discovery = {}
        for item in primary_sources:
            relative_path = item.get("path")
            if isinstance(relative_path, str) and relative_path:
                file_path = self.workspace.root / "sources" / relative_path
                if file_path.exists():
                    source_map_for_discovery[relative_path] = file_path.read_text()

        dependency_discovery = discover_dependencies(
            bundle.normalized_payload,
            source_map_for_discovery,
        )
        dependencies = self._fetch_discovered_dependencies(
            dependency_discovery.get("merged_candidates", []),
            target_address=address,
            chain=chain,
            skip_addresses={implementation_address.lower()} if implementation_address else set(),
        )

        bundle_path = self.workspace.write_json(
            "artifacts/source_bundle.json",
            {
                **bundle.normalized_payload,
                "status": "fetched",
                "proxy_resolution": {
                    "status": "provider_flag_only",
                    "proxy": bundle.normalized_payload["contract"]["proxy"],
                    "implementation": bundle.normalized_payload["contract"]["implementation"],
                },
                "dependency_discovery": dependency_discovery,
                "dependencies": dependencies,
                "related_contracts": related_contracts,
                "analysis_target": self._analysis_target_from_bundle(
                    address=address,
                    primary_contract=bundle.normalized_payload.get("contract", {}),
                    primary_files=primary_sources,
                    related_contracts=related_contracts,
                ),
            },
        )

        self._record(
            step="fetch_contract_source",
            path=request_path,
            kind="request",
            status="executed",
            summary="Persisted source fetch request metadata.",
        )
        self._record(
            step="fetch_contract_source",
            path=raw_response_path,
            kind="artifact",
            status="executed",
            summary="Stored the raw source provider response.",
        )
        self._record(
            step="fetch_contract_source",
            path=bundle_path,
            kind="artifact",
            status="executed",
            summary="Fetched and normalized verified source metadata.",
        )
        return "source_fetched"

    def build_ir(self, address: str, chain: str) -> str:
        bundle_path = self.workspace.root / "artifacts" / "source_bundle.json"
        if not bundle_path.exists():
            contracts_path = self.workspace.write_json(
                "ir/contracts.json",
                {
                    "target": {"address": address, "chain": chain},
                    "contracts": [],
                    "status": "source_bundle_missing",
                },
            )
            functions_path = self.workspace.write_json(
                "ir/functions.json",
                {"functions": [], "status": "source_bundle_missing"},
            )
            privilege_path = self.workspace.write_json(
                "ir/privilege_matrix.json",
                {
                    "roles": [],
                    "privileged_functions": [],
                    "status": "source_bundle_missing",
                },
            )
            self._record(
                step="build_ir",
                path=contracts_path,
                kind="ir",
                status="configured_not_executed",
                summary="Skipped contract inventory because source bundle is missing.",
            )
            self._record(
                step="build_ir",
                path=functions_path,
                kind="ir",
                status="configured_not_executed",
                summary="Skipped function inventory because source bundle is missing.",
            )
            self._record(
                step="build_ir",
                path=privilege_path,
                kind="ir",
                status="configured_not_executed",
                summary="Skipped privilege matrix because source bundle is missing.",
            )
            return "source_bundle_missing"

        bundle_payload = load_bundle_payload(bundle_path.read_text())
        if bundle_payload.get("status") != "fetched":
            contracts_path = self.workspace.write_json(
                "ir/contracts.json",
                {
                    "target": {"address": address, "chain": chain},
                    "contracts": [],
                    "status": "source_not_fetched",
                    "note": "Source fetching did not complete successfully.",
                },
            )
            functions_path = self.workspace.write_json(
                "ir/functions.json",
                {"functions": [], "status": "source_not_fetched"},
            )
            privilege_path = self.workspace.write_json(
                "ir/privilege_matrix.json",
                {
                    "roles": [],
                    "privileged_functions": [],
                    "status": "source_not_fetched",
                },
            )
            self._record(
                step="build_ir",
                path=contracts_path,
                kind="ir",
                status="configured_not_executed",
                summary="Skipped contract inventory because source fetch did not succeed.",
            )
            self._record(
                step="build_ir",
                path=functions_path,
                kind="ir",
                status="configured_not_executed",
                summary="Skipped function inventory because source fetch did not succeed.",
            )
            self._record(
                step="build_ir",
                path=privilege_path,
                kind="ir",
                status="configured_not_executed",
                summary="Skipped privilege matrix because source fetch did not succeed.",
            )
            return "source_not_fetched"

        source_map: Dict[str, str] = {}
        for source_file in self._all_bundle_files(bundle_payload):
            relative_path = source_file.get("path")
            if not isinstance(relative_path, str) or not relative_path:
                continue
            file_path = self.workspace.root / "sources" / relative_path
            if file_path.exists():
                source_map[relative_path] = file_path.read_text()

        if not source_map:
            contracts_path = self.workspace.write_json(
                "ir/contracts.json",
                {
                    "target": {"address": address, "chain": chain},
                    "contracts": [],
                    "status": "source_files_missing",
                },
            )
            functions_path = self.workspace.write_json(
                "ir/functions.json",
                {"functions": [], "status": "source_files_missing"},
            )
            privilege_path = self.workspace.write_json(
                "ir/privilege_matrix.json",
                {
                    "roles": [],
                    "privileged_functions": [],
                    "status": "source_files_missing",
                },
            )
            self._record(
                step="build_ir",
                path=contracts_path,
                kind="ir",
                status="executed_with_error",
                summary="Source bundle exists but no local source files were found.",
            )
            self._record(
                step="build_ir",
                path=functions_path,
                kind="ir",
                status="executed_with_error",
                summary="Source bundle exists but no local source files were found.",
            )
            self._record(
                step="build_ir",
                path=privilege_path,
                kind="ir",
                status="executed_with_error",
                summary="Source bundle exists but no local source files were found.",
            )
            return "source_files_missing"

        try:
            contracts_payload, functions_payload, privilege_payload = build_lightweight_ir(
                bundle_payload=bundle_payload,
                sources=source_map,
            )
        except Exception as exc:
            error_payload = {
                "target": {"address": address, "chain": chain},
                "status": "ir_generation_failed",
                "error": str(exc),
            }
            contracts_path = self.workspace.write_json("ir/contracts.json", error_payload)
            functions_path = self.workspace.write_json("ir/functions.json", error_payload)
            privilege_path = self.workspace.write_json("ir/privilege_matrix.json", error_payload)
            self._record(
                step="build_ir",
                path=contracts_path,
                kind="ir",
                status="executed_with_error",
                summary="Failed to generate contract inventory from source.",
            )
            self._record(
                step="build_ir",
                path=functions_path,
                kind="ir",
                status="executed_with_error",
                summary="Failed to generate function inventory from source.",
            )
            self._record(
                step="build_ir",
                path=privilege_path,
                kind="ir",
                status="executed_with_error",
                summary="Failed to generate privilege matrix from source.",
            )
            return "ir_generation_failed"

        contracts_path = self.workspace.write_json(
            "ir/contracts.json", contracts_payload
        )
        functions_path = self.workspace.write_json(
            "ir/functions.json", functions_payload
        )
        privilege_path = self.workspace.write_json(
            "ir/privilege_matrix.json", privilege_payload
        )
        self._record(
            step="build_ir",
            path=contracts_path,
            kind="ir",
            status="executed",
            summary="Generated a lightweight contract inventory from source text.",
        )
        self._record(
            step="build_ir",
            path=functions_path,
            kind="ir",
            status="executed",
            summary="Generated a lightweight function inventory from source text.",
        )
        self._record(
            step="build_ir",
            path=privilege_path,
            kind="ir",
            status="executed",
            summary="Generated a lightweight privilege matrix from source text.",
        )
        return "ir_generated"

    def run_dependency_analysis(self, address: str, chain: str) -> str:
        bundle_payload = self._load_source_bundle_payload()
        if not bundle_payload or bundle_payload.get("status") != "fetched":
            findings_path = self.workspace.write_json(
                "artifacts/dependency_findings.json",
                {
                    "target": {"address": address, "chain": chain},
                    "status": "source_not_fetched",
                    "findings": [],
                },
            )
            self._record(
                step="run_dependency_analysis",
                path=findings_path,
                kind="artifact",
                status="configured_not_executed",
                summary="Skipped dependency analysis because source fetching did not complete.",
            )
            return "source_not_fetched"

        findings = analyze_dependencies(bundle_payload, self.workspace.root)
        status = "executed" if findings else "executed"
        findings_path = self.workspace.write_json(
            "artifacts/dependency_findings.json",
            {
                "target": {"address": address, "chain": chain},
                "status": status,
                "findings": findings,
            },
        )
        self._record(
            step="run_dependency_analysis",
            path=findings_path,
            kind="artifact",
            status=status,
            summary="Analyzed fetched dependencies for high-signal role-specific findings.",
        )
        return status

    def aggregate_materials(self, address: str, chain: str) -> str:
        manifest_path = self.workspace.write_json(
            "reports/materials_manifest.json",
            {
                "target": {"address": address, "chain": chain},
                "run_id": self.workspace.run_id,
                "statuses": self.material_status_snapshot(),
                "inputs": self._existing_paths(
                    [
                        "input/request.json",
                        "input/source_request.json",
                    ]
                ),
                "core_materials": self._existing_paths(
                    [
                        "artifacts/source_bundle.json",
                        "artifacts/dependency_findings.json",
                        "ir/contracts.json",
                        "ir/functions.json",
                        "ir/privilege_matrix.json",
                    ]
                ),
                "optional_tool_artifacts": self._existing_paths(
                    [
                        "artifacts/chain_checks_plan.json",
                        "artifacts/chain_checks_output.txt",
                        "artifacts/chain_checks_findings.json",
                        "artifacts/static_plan.json",
                        "artifacts/slither_raw.json",
                        "artifacts/static_findings.json",
                    ]
                ),
                "artifact_records": [asdict(item) for item in self.artifacts],
                "notes": [
                    "This manifest is a neutral map of prepared review materials.",
                    "Use it to locate evidence; do not treat it as an audit conclusion.",
                    "Repository-side findings, when present, live in artifacts/dependency_findings.json.",
                    "Directly-invoked tools may leave optional artifacts under runs/<run_id>/artifacts/ that are not produced by the CLI itself.",
                ],
            },
        )
        self._record(
            step="aggregate_materials",
            path=manifest_path,
            kind="report",
            status="executed",
            summary="Stored a neutral manifest of prepared review materials.",
        )
        return manifest_path

    def read_workspace_file(self, relative_path: str) -> str:
        text = self.workspace.read_text(relative_path)
        return text[:4000]

    def artifact_index_payload(self) -> Dict[str, Any]:
        return {
            "run_id": self.workspace.run_id,
            "artifacts": [asdict(item) for item in self.artifacts],
        }

    def _record(self, step: str, path: str, kind: str, status: str, summary: str) -> None:
        self.artifacts = [
            item
            for item in self.artifacts
            if not (item.path == path and item.step == step and item.kind == kind)
        ]
        self.artifacts.append(
            ArtifactRecord(
                step=step,
                path=path,
                kind=kind,
                status=status,
                summary=summary,
            )
        )

    def _write_artifact_index(self) -> str:
        return self.workspace.write_json(
            "artifacts/artifact_index.json",
            self.artifact_index_payload(),
        )

    def write_artifact_index(self) -> str:
        return self._write_artifact_index()

    def material_status_snapshot(self) -> Dict[str, str]:
        source_payload = self._read_json_if_exists("artifacts/source_bundle.json")
        contracts_payload = self._read_json_if_exists("ir/contracts.json")
        dependency_payload = self._read_json_if_exists("artifacts/dependency_findings.json")

        source_status = str(source_payload.get("status") or "not_prepared")
        if source_status == "fetched":
            source_status = "source_fetched"

        ir_status = str(contracts_payload.get("status") or "")
        if not ir_status and contracts_payload.get("contracts") is not None:
            ir_status = "ir_generated"

        return {
            "source_fetch_status": source_status,
            "ir_status": ir_status or "not_prepared",
            "dependency_analysis_status": str(dependency_payload.get("status") or "not_prepared"),
        }

    def _load_source_bundle_payload(self) -> Dict[str, Any]:
        path = self.workspace.root / "artifacts" / "source_bundle.json"
        if not path.exists():
            return {}
        return load_bundle_payload(path.read_text())

    def _read_json_if_exists(self, relative_path: str) -> Dict[str, Any]:
        path = self.workspace.root / relative_path
        if not path.exists():
            return {}
        try:
            return json.loads(path.read_text())
        except json.JSONDecodeError:
            return {}

    def _existing_paths(self, relative_paths: List[str]) -> List[str]:
        existing: List[str] = []
        for relative_path in relative_paths:
            if (self.workspace.root / relative_path).exists():
                existing.append(relative_path)
        return existing

    def _all_bundle_files(self, bundle_payload: Dict[str, Any]) -> List[Dict[str, Any]]:
        files: List[Dict[str, Any]] = []
        primary_files = bundle_payload.get("files")
        if isinstance(primary_files, list):
            files.extend(item for item in primary_files if isinstance(item, dict))

        dependencies = bundle_payload.get("dependencies")
        if isinstance(dependencies, list):
            for dependency in dependencies:
                if not isinstance(dependency, dict):
                    continue
                dependency_files = dependency.get("files")
                if isinstance(dependency_files, list):
                    files.extend(item for item in dependency_files if isinstance(item, dict))
                nested_related = dependency.get("related_contracts")
                if isinstance(nested_related, list):
                    for related in nested_related:
                        if not isinstance(related, dict):
                            continue
                        related_files = related.get("files")
                        if isinstance(related_files, list):
                            files.extend(item for item in related_files if isinstance(item, dict))

        related_contracts = bundle_payload.get("related_contracts")
        if isinstance(related_contracts, list):
            for related in related_contracts:
                if not isinstance(related, dict):
                    continue
                related_files = related.get("files")
                if isinstance(related_files, list):
                    files.extend(item for item in related_files if isinstance(item, dict))
        return files

    def _analysis_target_from_bundle(
        self,
        *,
        address: str,
        primary_contract: Dict[str, Any],
        primary_files: List[Dict[str, Any]],
        related_contracts: List[Dict[str, Any]],
    ) -> Dict[str, Any]:
        for related in related_contracts:
            if (
                isinstance(related, dict)
                and related.get("role") == "implementation"
                and related.get("status") == "fetched"
            ):
                files = related.get("files")
                contract = related.get("contract")
                if isinstance(files, list) and files:
                    first_path = files[0].get("path")
                    if isinstance(first_path, str) and first_path:
                        return {
                            "address": related.get("address") or address,
                            "contract_name": contract.get("name") if isinstance(contract, dict) else "",
                            "path": first_path,
                            "role": "implementation",
                        }

        first_primary_path = ""
        if primary_files:
            first_primary_path = str(primary_files[0].get("path") or "")
        return {
            "address": address,
            "contract_name": primary_contract.get("name") if isinstance(primary_contract, dict) else "",
            "path": first_primary_path,
            "role": "target",
        }

    def _write_fetched_source_files(
        self,
        files: List[Any],
        *,
        prefix: str,
        summary_prefix: str,
    ) -> List[Dict[str, Any]]:
        written: List[Dict[str, Any]] = []
        for source_file in files:
            relative_path = getattr(source_file, "path", "")
            content = getattr(source_file, "content", None)
            if not isinstance(relative_path, str) or not isinstance(content, str):
                continue
            final_path = f"{prefix}/{relative_path}" if prefix else relative_path
            self.workspace.write_text(f"sources/{final_path}", content)
            entry = {
                "path": final_path,
                "length": len(content),
                "original_path": relative_path,
            }
            written.append(entry)
            self._record(
                step="fetch_contract_source",
                path=f"sources/{final_path}",
                kind="source",
                status="executed",
                summary=summary_prefix,
            )
        return written

    def _fetch_dependency_bundle_record(
        self,
        *,
        address: str,
        chain: str,
        role: str,
        name: str,
        prefix: str,
    ) -> Dict[str, Any]:
        try:
            bundle = fetch_verified_source(
                base_url=self.config.source_api_base,
                api_key=self.config.source_api_key,
                headers=self.config.source_api_headers,
                address=address,
                chain=chain,
            )
        except Exception as exc:
            return {
                "role": role,
                "name": name,
                "address": address,
                "status": "fetch_failed",
                "error": str(exc),
            }

        response_artifact = self.workspace.write_json(
            f"artifacts/source_provider_response_{prefix.replace('/', '_')}.json",
            bundle.provider_payload,
        )
        written_files = self._write_fetched_source_files(
            bundle.files,
            prefix=prefix,
            summary_prefix="Stored a fetched dependency source file.",
        )

        record = {
            "role": role,
            "name": name,
            "address": address,
            "provider": bundle.normalized_payload.get("provider", {}),
            "contract": bundle.normalized_payload.get("contract", {}),
            "compiler": bundle.normalized_payload.get("compiler", {}),
            "abi": bundle.normalized_payload.get("abi"),
            "source_layout": bundle.normalized_payload.get("source_layout"),
            "source_meta": bundle.normalized_payload.get("source_meta", {}),
            "files": written_files,
            "provider_response_artifact": response_artifact,
            "status": "fetched",
            "related_contracts": [],
        }
        self._record(
            step="fetch_contract_source",
            path=response_artifact,
            kind="artifact",
            status="executed",
            summary="Stored the raw dependency provider response.",
        )

        contract = bundle.normalized_payload.get("contract", {})
        implementation_address = (
            str(contract.get("implementation") or "").strip()
            if isinstance(contract, dict)
            else ""
        )
        if (
            isinstance(contract, dict)
            and contract.get("proxy")
            and implementation_address
            and implementation_address.lower() != address.lower()
        ):
            nested_prefix = f"{prefix}/implementation"
            nested = self._fetch_dependency_bundle_record(
                address=implementation_address,
                chain=chain,
                role="implementation",
                name=f"{name or role}-implementation",
                prefix=nested_prefix,
            )
            record["related_contracts"].append(nested)
        return record

    def _fetch_discovered_dependencies(
        self,
        candidates: List[Any],
        *,
        target_address: str,
        chain: str,
        skip_addresses: set[str],
    ) -> List[Dict[str, Any]]:
        records: List[Dict[str, Any]] = []
        seen = {target_address.lower(), *skip_addresses}
        for item in candidates:
            if not isinstance(item, dict):
                continue
            address = str(item.get("address") or "").lower()
            if not address or address in seen:
                continue
            seen.add(address)
            role = str(item.get("role") or "dependency")
            name = str(item.get("name") or role or address)
            safe_name = self._safe_dependency_name(name)
            prefix = f"dependencies/{role}/{safe_name}_{address}"
            record = self._fetch_dependency_bundle_record(
                address=address,
                chain=chain,
                role=role,
                name=name,
                prefix=prefix,
            )
            record["discovery"] = {
                "sources": list(item.get("sources") or []),
                "internal_type": item.get("internal_type", ""),
                "solidity_type": item.get("solidity_type", ""),
                "file": item.get("file", ""),
            }
            records.append(record)
        return records

    def _safe_dependency_name(self, name: str) -> str:
        cleaned = []
        for char in name.lower():
            if char.isalnum():
                cleaned.append(char)
            else:
                cleaned.append("_")
        return "".join(cleaned).strip("_") or "dependency"

    def _load_existing_artifacts(self) -> List[ArtifactRecord]:
        path = self.workspace.root / "artifacts" / "artifact_index.json"
        if not path.exists():
            return []
        try:
            payload = json.loads(path.read_text())
        except json.JSONDecodeError:
            return []
        items = payload.get("artifacts")
        if not isinstance(items, list):
            return []
        artifacts: List[ArtifactRecord] = []
        for item in items:
            if not isinstance(item, dict):
                continue
            try:
                artifacts.append(ArtifactRecord(**item))
            except TypeError:
                continue
        return artifacts
