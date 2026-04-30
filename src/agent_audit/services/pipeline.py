from __future__ import annotations

from dataclasses import asdict
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
from typing import Any, Dict, List

from agent_audit.config import AppConfig
from agent_audit.dependency_analyzers import analyze_dependencies
from agent_audit.dependency_discovery import discover_dependencies
from agent_audit.schemas import ArtifactRecord
from agent_audit.source_fetch import fetch_verified_source
from agent_audit.source_fetch import parse_json_string
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

    def prepare_slither_project(self, address: str, chain: str) -> str:
        slither_root = self.workspace.root / "slither_project"
        bundle_payload = self._load_source_bundle_payload()
        if not bundle_payload or bundle_payload.get("status") != "fetched":
            if slither_root.exists():
                shutil.rmtree(slither_root)
            slither_root.mkdir(parents=True, exist_ok=True)
            manifest = {
                "target": {"address": address, "chain": chain},
                "run_id": self.workspace.run_id,
                "status": "source_not_fetched",
                "note": "Fetch verified source before preparing a Slither project.",
            }
            manifest_path = self.workspace.write_json("slither_project/build_manifest.json", manifest)
            self._record(
                step="prepare_slither_project",
                path=manifest_path,
                kind="prep",
                status="configured_not_executed",
                summary="Skipped Slither project preparation because source fetching did not complete.",
            )
            return "source_not_fetched"

        sources_root = self.workspace.root / "sources"
        if not sources_root.exists():
            if slither_root.exists():
                shutil.rmtree(slither_root)
            slither_root.mkdir(parents=True, exist_ok=True)
            manifest = {
                "target": {"address": address, "chain": chain},
                "run_id": self.workspace.run_id,
                "status": "source_files_missing",
                "note": "Source bundle exists but sources/ is missing.",
            }
            manifest_path = self.workspace.write_json("slither_project/build_manifest.json", manifest)
            self._record(
                step="prepare_slither_project",
                path=manifest_path,
                kind="prep",
                status="executed_with_error",
                summary="Failed Slither project preparation because source files are missing.",
            )
            return "source_files_missing"

        if slither_root.exists():
            shutil.rmtree(slither_root)
        slither_root.mkdir(parents=True, exist_ok=True)

        linked_entries = self._link_slither_source_entries(sources_root, slither_root)
        node_modules_links = self._create_slither_node_modules(sources_root / "npm", slither_root / "node_modules")

        analysis_target = self._analysis_target_payload(bundle_payload)
        preferred_settings = self._slither_target_settings(
            bundle_payload=bundle_payload,
            linked_entries=linked_entries,
            node_modules_links=node_modules_links,
            target_path=str(analysis_target.get("path") or ""),
        )
        analysis_target["prepared_path"] = preferred_settings["prepared_target"]
        analysis_target["prepared_root"] = preferred_settings["prepared_root"]

        remappings_path = self.workspace.write_text(
            "slither_project/remappings.txt",
            "".join(f"{entry}\n" for entry in preferred_settings["remappings"]),
        )
        config_path = self.workspace.write_json(
            "slither_project/slither_inputs.json",
            {
                "status": "prepared",
                "working_dir": preferred_settings["working_dir_token"],
                "base_path": ".",
                "include_paths": preferred_settings["include_paths"],
                "remappings_file": preferred_settings["remappings_file"],
                "remappings": preferred_settings["remappings"],
                "solc_args": preferred_settings["solc_args"],
                "target_path": preferred_settings["target_path"],
                "prepared_target": preferred_settings["prepared_target"],
            },
        )
        manifest_path = self.workspace.write_json(
            "slither_project/build_manifest.json",
            {
                "target": {"address": address, "chain": chain},
                "run_id": self.workspace.run_id,
                "status": "prepared",
                "slither_project_root": "slither_project",
                "analysis_target": analysis_target,
                "compiler_version": preferred_settings["compiler_version"],
                "solc_version": preferred_settings["solc_version"],
                "solc_select": preferred_settings["solc_select"],
                "linked_source_entries": linked_entries,
                "node_modules_links": node_modules_links,
                "remappings": preferred_settings["remappings"],
                "solc_args": preferred_settings["solc_args"],
                "config_path": config_path,
                "preferred_target": preferred_settings["prepared_target"],
                "preferred_working_dir": preferred_settings["working_dir"],
                "preferred_source_root": preferred_settings["source_root"],
            },
        )

        for path, summary in [
            (remappings_path, "Prepared Slither remappings."),
            (config_path, "Prepared Slither config metadata."),
            (manifest_path, "Prepared a deterministic Slither project manifest."),
        ]:
            self._record(
                step="prepare_slither_project",
                path=path,
                kind="prep",
                status="executed",
                summary=summary,
            )
        return "prepared"

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
                    ]
                ),
                "optional_tool_artifacts": self._existing_paths(
                    [
                        "artifacts/chain_checks_plan.json",
                        "artifacts/chain_checks_output.txt",
                        "artifacts/chain_checks_findings.json",
                        "artifacts/chain_index.json",
                        "artifacts/static_plan.json",
                        "artifacts/slither_raw.json",
                        "artifacts/static_findings.json",
                        "artifacts/analyzer_index.json",
                        "slither_project/build_manifest.json",
                        "slither_project/remappings.txt",
                        "slither_project/slither_inputs.json",
                    ]
                )
                + self._existing_tree(
                    [
                        "artifacts/analyzer",
                        "artifacts/chain",
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
        dependency_payload = self._read_json_if_exists("artifacts/dependency_findings.json")

        source_status = str(source_payload.get("status") or "not_prepared")
        if source_status == "fetched":
            source_status = "source_fetched"

        return {
            "source_fetch_status": source_status,
            "dependency_analysis_status": str(dependency_payload.get("status") or "not_prepared"),
        }

    def _load_source_bundle_payload(self) -> Dict[str, Any]:
        path = self.workspace.root / "artifacts" / "source_bundle.json"
        if not path.exists():
            return {}
        payload = parse_json_string(path.read_text())
        if not isinstance(payload, dict):
            raise ValueError("bundle payload must be a JSON object")
        return payload

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

    def _existing_tree(self, relative_roots: List[str]) -> List[str]:
        existing: List[str] = []
        seen = set()
        for relative_root in relative_roots:
            root = self.workspace.root / relative_root
            if root.is_file():
                if relative_root not in seen:
                    existing.append(relative_root)
                    seen.add(relative_root)
                continue
            if not root.exists():
                continue
            for path in sorted(root.rglob("*")):
                if not path.is_file():
                    continue
                relative_path = self.workspace.relative(path)
                if relative_path in seen:
                    continue
                existing.append(relative_path)
                seen.add(relative_path)
        return existing

    def _link_slither_source_entries(self, sources_root: Path, slither_root: Path) -> List[Dict[str, Any]]:
        linked: List[Dict[str, Any]] = []
        for entry in sorted(sources_root.iterdir(), key=lambda item: item.name):
            link_path = slither_root / entry.name
            self._recreate_symlink(link_path, entry)
            linked.append(
                {
                    "path": entry.name,
                    "target": self.workspace.relative(entry),
                    "kind": "directory" if entry.is_dir() else "file",
                }
            )
        return linked

    def _create_slither_node_modules(self, npm_root: Path, node_modules_root: Path) -> List[Dict[str, Any]]:
        links: List[Dict[str, Any]] = []
        if not npm_root.exists():
            return links

        for entry in sorted(npm_root.iterdir(), key=lambda item: item.name):
            if not entry.is_dir():
                continue
            if entry.name.startswith("@"):
                for package_dir in sorted(entry.iterdir(), key=lambda item: item.name):
                    if not package_dir.is_dir():
                        continue
                    alias_name, version = self._split_versioned_package_name(package_dir.name)
                    link_path = node_modules_root / entry.name / alias_name
                    self._recreate_symlink(link_path, package_dir)
                    links.append(
                        {
                            "alias": f"{entry.name}/{alias_name}",
                            "version": version,
                            "link_path": self.workspace.relative(link_path),
                            "target": self.workspace.relative(package_dir),
                        }
                    )
            else:
                alias_name, version = self._split_versioned_package_name(entry.name)
                link_path = node_modules_root / alias_name
                self._recreate_symlink(link_path, entry)
                links.append(
                    {
                        "alias": alias_name,
                        "version": version,
                        "link_path": self.workspace.relative(link_path),
                        "target": self.workspace.relative(entry),
                    }
                )
        return links

    def _recreate_symlink(self, link_path: Path, target_path: Path) -> None:
        if link_path.exists() or link_path.is_symlink():
            if link_path.is_dir() and not link_path.is_symlink():
                shutil.rmtree(link_path)
            else:
                link_path.unlink()
        link_path.parent.mkdir(parents=True, exist_ok=True)
        relative_target = os.path.relpath(target_path, start=link_path.parent)
        link_path.symlink_to(relative_target, target_is_directory=target_path.is_dir())

    def _split_versioned_package_name(self, name: str) -> tuple[str, str]:
        match = re.match(r"^(?P<package>.+)@(?P<version>\d[\w.+-]*)$", name)
        if not match:
            return name, ""
        return match.group("package"), match.group("version")

    def _analysis_target_payload(self, bundle_payload: Dict[str, Any]) -> Dict[str, Any]:
        preferred_path = str(bundle_payload.get("contract", {}).get("file_name") or "")
        if preferred_path and self._record_for_path(bundle_payload, preferred_path):
            return {
                "address": str(bundle_payload.get("target", {}).get("address") or ""),
                "contract_name": str(bundle_payload.get("contract", {}).get("name") or ""),
                "path": preferred_path,
                "role": "target",
                "prepared_path": preferred_path,
            }

        analysis_target = bundle_payload.get("analysis_target")
        if isinstance(analysis_target, dict) and str(analysis_target.get("path") or ""):
            return {
                "address": str(analysis_target.get("address") or bundle_payload.get("target", {}).get("address") or ""),
                "contract_name": str(analysis_target.get("contract_name") or ""),
                "path": str(analysis_target.get("path") or ""),
                "role": str(analysis_target.get("role") or ""),
                "prepared_path": str(analysis_target.get("path") or ""),
            }

        primary_files = bundle_payload.get("files")
        first_path = ""
        if isinstance(primary_files, list) and primary_files:
            first_path = str(primary_files[0].get("path") or "")
        return {
            "address": str(bundle_payload.get("target", {}).get("address") or ""),
            "contract_name": str(bundle_payload.get("contract", {}).get("name") or ""),
            "path": first_path,
            "role": "target",
            "prepared_path": first_path,
        }

    def _collect_bundle_records(self, bundle_payload: Dict[str, Any]) -> List[Dict[str, Any]]:
        records: List[Dict[str, Any]] = [
            {
                "files": bundle_payload.get("files", []),
                "compiler": bundle_payload.get("compiler", {}),
                "source_meta": bundle_payload.get("source_meta", {}),
                "contract": bundle_payload.get("contract", {}),
                "role": "target",
                "address": bundle_payload.get("target", {}).get("address", ""),
            }
        ]
        for key in ("dependencies", "related_contracts"):
            entries = bundle_payload.get(key)
            if not isinstance(entries, list):
                continue
            for entry in entries:
                if not isinstance(entry, dict):
                    continue
                records.extend(self._collect_record_tree(entry))
        return records

    def _collect_record_tree(self, record: Dict[str, Any]) -> List[Dict[str, Any]]:
        records = [record]
        related = record.get("related_contracts")
        if isinstance(related, list):
            for nested in related:
                if isinstance(nested, dict):
                    records.extend(self._collect_record_tree(nested))
        return records

    def _record_for_path(self, bundle_payload: Dict[str, Any], relative_path: str) -> Dict[str, Any]:
        for record in self._collect_bundle_records(bundle_payload):
            files = record.get("files")
            if not isinstance(files, list):
                continue
            for item in files:
                if isinstance(item, dict) and str(item.get("path") or "") == relative_path:
                    return record
        return {}

    def _compiler_version_for_path(self, bundle_payload: Dict[str, Any], relative_path: str) -> str:
        record = self._record_for_path(bundle_payload, relative_path)
        compiler = record.get("compiler")
        if isinstance(compiler, dict):
            return str(compiler.get("version") or "")
        return ""

    def _source_meta_for_path(self, bundle_payload: Dict[str, Any], relative_path: str) -> Dict[str, Any]:
        record = self._record_for_path(bundle_payload, relative_path)
        source_meta = record.get("source_meta")
        return source_meta if isinstance(source_meta, dict) else {}

    def _provider_remappings(self, source_meta: Dict[str, Any]) -> List[str]:
        settings = source_meta.get("settings")
        if not isinstance(settings, dict):
            return []
        remappings = settings.get("remappings")
        if not isinstance(remappings, list):
            return []
        return [str(item) for item in remappings if isinstance(item, str) and item]

    def _node_modules_remappings(self, node_modules_links: List[Dict[str, Any]]) -> List[str]:
        remappings: List[str] = []
        for item in node_modules_links:
            alias = str(item.get("alias") or "").strip("/")
            if not alias:
                continue
            remappings.append(f"{alias}/=node_modules/{alias}/")
        return remappings

    def _merge_remapping_lists(self, *groups: List[str]) -> List[str]:
        merged: List[str] = []
        seen = set()
        for group in groups:
            for entry in group:
                if entry in seen:
                    continue
                seen.add(entry)
                merged.append(entry)
        return merged

    def _slither_target_settings(
        self,
        *,
        bundle_payload: Dict[str, Any],
        linked_entries: List[Dict[str, Any]],
        node_modules_links: List[Dict[str, Any]],
        target_path: str,
    ) -> Dict[str, Any]:
        normalized_target_path = str(target_path or ".").lstrip("./") or "."
        source_root = self._slither_source_root_for_target(normalized_target_path, linked_entries)
        prepared_target = self._slither_relative_target_path(normalized_target_path, source_root)
        compiler_version = self._compiler_version_for_path(bundle_payload, normalized_target_path)
        solc_version = self._extract_semver(compiler_version)
        source_meta = self._source_meta_for_path(bundle_payload, normalized_target_path)
        provider_remappings = self._provider_remappings(source_meta)
        generated_remappings = self._node_modules_remappings(node_modules_links)
        remappings = self._merge_remapping_lists(provider_remappings, generated_remappings)
        use_project_root = bool(remappings)
        working_root = "" if use_project_root else source_root
        prepared_root = "." if use_project_root else (source_root or ".")
        prepared_target = (
            normalized_target_path
            if use_project_root
            else self._slither_relative_target_path(normalized_target_path, source_root)
        )
        include_paths = self._slither_include_paths(working_root, bool(node_modules_links))
        return {
            "target_path": normalized_target_path,
            "source_root": source_root,
            "prepared_root": prepared_root,
            "prepared_target": prepared_target,
            "working_dir": f"slither_project/{working_root}" if working_root else "slither_project",
            "working_dir_token": working_root or ".",
            "compiler_version": compiler_version,
            "solc_version": solc_version,
            "solc_select": self._solc_select_status(solc_version),
            "include_paths": include_paths,
            "remappings": remappings,
            "remappings_file": self._slither_relative_from_working_dir(working_root, "remappings.txt"),
            "solc_args": self._slither_solc_args(include_paths),
        }

    def _slither_source_root_for_target(
        self,
        target_path: str,
        linked_entries: List[Dict[str, Any]],
    ) -> str:
        normalized_target_path = str(target_path or "").lstrip("./")
        matches: List[str] = []
        for entry in linked_entries:
            source_root = str(entry.get("path") or "").strip("/")
            if not source_root:
                continue
            if normalized_target_path == source_root or normalized_target_path.startswith(f"{source_root}/"):
                matches.append(source_root)
        if not matches:
            return ""
        return max(matches, key=len)

    def _slither_relative_target_path(self, target_path: str, source_root: str) -> str:
        normalized_target_path = str(target_path or ".").lstrip("./") or "."
        if not source_root:
            return normalized_target_path
        if normalized_target_path == source_root:
            return "."
        prefix = f"{source_root}/"
        if normalized_target_path.startswith(prefix):
            return normalized_target_path[len(prefix) :] or "."
        return normalized_target_path

    def _slither_relative_from_working_dir(self, source_root: str, path_in_slither_root: str) -> str:
        start = source_root or "."
        return os.path.relpath(path_in_slither_root, start=start)

    def _slither_include_paths(self, source_root: str, has_node_modules: bool) -> List[str]:
        include_paths = ["."]
        if has_node_modules:
            node_modules_path = self._slither_relative_from_working_dir(source_root, "node_modules")
            if node_modules_path not in include_paths:
                include_paths.append(node_modules_path)
        return include_paths

    def _slither_solc_args(self, include_paths: List[str]) -> str:
        args = ["--base-path", "."]
        allow_paths: List[str] = ["."]
        for entry in include_paths:
            if entry == ".":
                continue
            args.extend(["--include-path", entry])
            allow_paths.append(entry)
        args.extend(["--allow-paths", ",".join(dict.fromkeys(allow_paths))])
        return " ".join(args)

    def _extract_semver(self, compiler_version: str) -> str:
        match = re.search(r"(\d+\.\d+\.\d+)", compiler_version or "")
        return match.group(1) if match else ""

    def _solc_select_status(self, requested_version: str) -> Dict[str, Any]:
        if not requested_version:
            return {
                "requested_version": "",
                "is_installed": False,
                "current_version": "",
                "available_versions": [],
                "recommended_action": "No semantic compiler version could be extracted from source metadata.",
            }

        try:
            result = subprocess.run(
                ["nix", "develop", ".#default", "-c", "solc-select", "versions"],
                cwd=self.workspace.project_root,
                capture_output=True,
                text=True,
                timeout=60,
                check=False,
            )
        except Exception as exc:
            return {
                "requested_version": requested_version,
                "is_installed": False,
                "current_version": "",
                "available_versions": [],
                "recommended_action": f"Could not query solc-select versions: {exc}",
            }

        available_versions: List[str] = []
        current_version = ""
        for raw_line in result.stdout.splitlines():
            line = raw_line.strip()
            if not line:
                continue
            match = re.match(r"(?P<version>\d+\.\d+\.\d+)(?:\s+\(current.*\))?$", line)
            if not match:
                continue
            version = match.group("version")
            available_versions.append(version)
            if "(current" in line:
                current_version = version

        is_installed = requested_version in available_versions
        if is_installed:
            recommended_action = (
                f"Run `solc-select use {requested_version}` inside the devShell before invoking Slither."
            )
        else:
            recommended_action = (
                f"`{requested_version}` is not installed in solc-select. Install or select it before Slither, "
                f"for example with `solc-select install {requested_version} && solc-select use {requested_version}`."
            )

        return {
            "requested_version": requested_version,
            "is_installed": is_installed,
            "current_version": current_version,
            "available_versions": available_versions,
            "recommended_action": recommended_action,
            "command_status": "ok" if result.returncode == 0 else "error",
            "stderr_preview": result.stderr[:1000],
        }

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
        preferred_path = str(primary_contract.get("file_name") or "") if isinstance(primary_contract, dict) else ""
        if preferred_path and any(str(item.get("path") or "") == preferred_path for item in primary_files):
            first_primary_path = preferred_path
        elif primary_files:
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
