use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use regex::Regex;
use serde_json::{Map, Value, json};

use super::config::AppConfig;
use super::dependency_analyzers::analyze_dependencies;
use super::dependency_discovery::discover_dependencies;
use super::errors::AppResult;
use super::schemas::ArtifactRecord;
use super::source_fetch::{
    extract_semver, fetch_verified_source, merge_unique_lists, sanitize_dependency_name,
};
use super::workspace::RunWorkspace;

pub struct AuditPipelineService {
    pub config: AppConfig,
    pub workspace: RunWorkspace,
    pub artifacts: Vec<ArtifactRecord>,
}

impl AuditPipelineService {
    pub fn new(config: AppConfig, workspace: RunWorkspace) -> Self {
        let artifacts = load_existing_artifacts(&workspace);
        Self {
            config,
            workspace,
            artifacts,
        }
    }

    pub fn fetch_contract_source(&mut self, address: &str, chain: &str) -> AppResult<String> {
        let request_payload = json!({
            "address": address,
            "chain": chain,
            "source_api_base": self.config.source_api_base,
            "source_api_configured": self.config.source_api_base.is_some(),
            "source_api_header_names": self.config.source_api_headers.keys().cloned().collect::<Vec<_>>(),
            "rpc_url_configured": self.config.rpc_url.is_some(),
        });
        let request_path = self
            .workspace
            .write_json("input/source_request.json", &request_payload)?;

        let Some(base_url) = self.config.source_api_base.clone() else {
            let bundle_path = self.workspace.write_json(
                "artifacts/source_bundle.json",
                &json!({
                    "target": {"address": address, "chain": chain},
                    "contracts": [],
                    "proxy_resolution": {"status": "not_attempted"},
                    "status": "source_api_not_configured",
                    "note": "Configure AGENT_AUDIT_SOURCE_API_BASE to enable verified source fetching.",
                }),
            )?;
            self.record(
                "fetch_contract_source",
                &request_path,
                "request",
                "configured_not_executed",
                "Persisted source fetch request metadata.",
            );
            self.record(
                "fetch_contract_source",
                &bundle_path,
                "artifact",
                "configured_not_executed",
                "Skipped source fetch because the source API is not configured.",
            );
            return Ok("source_api_not_configured".to_string());
        };

        let bundle = match fetch_verified_source(
            &base_url,
            self.config.source_api_key.as_deref(),
            &self.config.source_api_headers,
            address,
            chain,
        ) {
            Ok(bundle) => bundle,
            Err(error) => {
                let bundle_path = self.workspace.write_json(
                    "artifacts/source_bundle.json",
                    &json!({
                        "target": {"address": address, "chain": chain},
                        "contracts": [],
                        "proxy_resolution": {"status": "not_attempted"},
                        "status": "source_fetch_failed",
                        "error": error.to_string(),
                        "error_debug": format!("{error:?}"),
                    }),
                )?;
                self.record(
                    "fetch_contract_source",
                    &request_path,
                    "request",
                    "executed_with_error",
                    "Persisted source fetch request metadata.",
                );
                self.record(
                    "fetch_contract_source",
                    &bundle_path,
                    "artifact",
                    "executed_with_error",
                    "Source fetch failed; inspect the stored error payload.",
                );
                return Ok("source_fetch_failed".to_string());
            }
        };

        let proxy_contract = bundle
            .normalized_payload
            .get("contract")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let implementation_address = proxy_contract
            .get("implementation")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();

        let raw_response_path = self.workspace.write_json(
            "artifacts/source_provider_response.json",
            &bundle.provider_payload,
        )?;
        let primary_sources =
            self.write_fetched_source_files(&bundle.files, "", "Stored a fetched source file.")?;

        let mut related_contracts = Vec::new();
        if proxy_contract.get("proxy").and_then(Value::as_bool) == Some(true)
            && !implementation_address.is_empty()
            && implementation_address.to_lowercase() != address.to_lowercase()
        {
            related_contracts.push(self.fetch_dependency_bundle_record(
                &implementation_address,
                chain,
                "implementation",
                "implementation",
                "implementation",
            )?);
        }

        let mut source_map_for_discovery = BTreeMap::new();
        for item in &primary_sources {
            if let Some(relative_path) = item.get("path").and_then(Value::as_str) {
                let file_path = self.workspace.root.join("sources").join(relative_path);
                if file_path.exists() {
                    source_map_for_discovery
                        .insert(relative_path.to_string(), fs::read_to_string(file_path)?);
                }
            }
        }

        let dependency_discovery =
            discover_dependencies(&bundle.normalized_payload, &source_map_for_discovery);
        let dependencies = self.fetch_discovered_dependencies(
            dependency_discovery
                .get("merged_candidates")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            address,
            chain,
            if implementation_address.is_empty() {
                BTreeSet::new()
            } else {
                BTreeSet::from([implementation_address.to_lowercase()])
            },
        )?;

        let analysis_target = analysis_target_from_bundle(
            address,
            &proxy_contract,
            &primary_sources,
            &related_contracts,
        );

        let mut bundle_payload = bundle
            .normalized_payload
            .as_object()
            .cloned()
            .unwrap_or_default();
        bundle_payload.insert("status".to_string(), Value::String("fetched".to_string()));
        bundle_payload.insert(
            "proxy_resolution".to_string(),
            json!({
                "status": "provider_flag_only",
                "proxy": proxy_contract.get("proxy").cloned().unwrap_or(Value::Bool(false)),
                "implementation": proxy_contract.get("implementation").cloned().unwrap_or(Value::String(String::new())),
            }),
        );
        bundle_payload.insert("dependency_discovery".to_string(), dependency_discovery);
        bundle_payload.insert("dependencies".to_string(), Value::Array(dependencies));
        bundle_payload.insert(
            "related_contracts".to_string(),
            Value::Array(related_contracts),
        );
        bundle_payload.insert("analysis_target".to_string(), analysis_target);

        let bundle_path = self.workspace.write_json(
            "artifacts/source_bundle.json",
            &Value::Object(bundle_payload),
        )?;

        self.record(
            "fetch_contract_source",
            &request_path,
            "request",
            "executed",
            "Persisted source fetch request metadata.",
        );
        self.record(
            "fetch_contract_source",
            &raw_response_path,
            "artifact",
            "executed",
            "Stored the raw source provider response.",
        );
        self.record(
            "fetch_contract_source",
            &bundle_path,
            "artifact",
            "executed",
            "Fetched and normalized verified source metadata.",
        );
        Ok("source_fetched".to_string())
    }

    pub fn run_dependency_analysis(&mut self, address: &str, chain: &str) -> AppResult<String> {
        let bundle_payload = self.load_source_bundle_payload()?;
        if bundle_payload.get("status").and_then(Value::as_str) != Some("fetched") {
            let findings_path = self.workspace.write_json(
                "artifacts/dependency_findings.json",
                &json!({
                    "target": {"address": address, "chain": chain},
                    "status": "source_not_fetched",
                    "findings": [],
                }),
            )?;
            self.record(
                "run_dependency_analysis",
                &findings_path,
                "artifact",
                "configured_not_executed",
                "Skipped dependency analysis because source fetching did not complete.",
            );
            return Ok("source_not_fetched".to_string());
        }

        let findings = analyze_dependencies(&bundle_payload, &self.workspace.root);
        let status = "executed";
        let findings_path = self.workspace.write_json(
            "artifacts/dependency_findings.json",
            &json!({
                "target": {"address": address, "chain": chain},
                "status": status,
                "findings": findings,
            }),
        )?;
        self.record(
            "run_dependency_analysis",
            &findings_path,
            "artifact",
            status,
            "Analyzed fetched dependencies for high-signal role-specific findings.",
        );
        Ok(status.to_string())
    }

    pub fn prepare_slither_project(&mut self, address: &str, chain: &str) -> AppResult<String> {
        let slither_root = self.workspace.root.join("slither_project");
        let bundle_payload = self.load_source_bundle_payload()?;
        if bundle_payload.get("status").and_then(Value::as_str) != Some("fetched") {
            recreate_dir(&slither_root)?;
            let manifest_path = self.workspace.write_json(
                "slither_project/build_manifest.json",
                &json!({
                    "target": {"address": address, "chain": chain},
                    "run_id": self.workspace.run_id,
                    "status": "source_not_fetched",
                    "note": "Fetch verified source before preparing a Slither project.",
                }),
            )?;
            self.record(
                "prepare_slither_project",
                &manifest_path,
                "prep",
                "configured_not_executed",
                "Skipped Slither project preparation because source fetching did not complete.",
            );
            return Ok("source_not_fetched".to_string());
        }

        let sources_root = self.workspace.root.join("sources");
        if !sources_root.exists() {
            recreate_dir(&slither_root)?;
            let manifest_path = self.workspace.write_json(
                "slither_project/build_manifest.json",
                &json!({
                    "target": {"address": address, "chain": chain},
                    "run_id": self.workspace.run_id,
                    "status": "source_files_missing",
                    "note": "Source bundle exists but sources/ is missing.",
                }),
            )?;
            self.record(
                "prepare_slither_project",
                &manifest_path,
                "prep",
                "executed_with_error",
                "Failed Slither project preparation because source files are missing.",
            );
            return Ok("source_files_missing".to_string());
        }

        recreate_dir(&slither_root)?;
        let linked_entries = self.link_slither_source_entries(&sources_root, &slither_root)?;
        let node_modules_links = self.create_slither_node_modules(
            &sources_root.join("npm"),
            &slither_root.join("node_modules"),
        )?;
        let mut analysis_target = analysis_target_payload(&bundle_payload);
        let preferred_settings = slither_target_settings(
            &self.workspace,
            &bundle_payload,
            &linked_entries,
            &node_modules_links,
            analysis_target
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        );

        if let Some(object) = analysis_target.as_object_mut() {
            object.insert(
                "prepared_path".to_string(),
                Value::String(preferred_settings.prepared_target.clone()),
            );
            object.insert(
                "prepared_root".to_string(),
                Value::String(preferred_settings.prepared_root.clone()),
            );
        }

        let remappings_path = self.workspace.write_text(
            "slither_project/remappings.txt",
            &preferred_settings
                .remappings
                .iter()
                .map(|entry| format!("{entry}\n"))
                .collect::<String>(),
        )?;
        let config_path = self.workspace.write_json(
            "slither_project/slither_inputs.json",
            &json!({
                "status": "prepared",
                "working_dir": preferred_settings.working_dir_token,
                "base_path": ".",
                "include_paths": preferred_settings.include_paths,
                "remappings_file": preferred_settings.remappings_file,
                "remappings": preferred_settings.remappings,
                "solc_args": preferred_settings.solc_args,
                "target_path": preferred_settings.target_path,
                "prepared_target": preferred_settings.prepared_target,
            }),
        )?;
        let manifest_path = self.workspace.write_json(
            "slither_project/build_manifest.json",
            &json!({
                "target": {"address": address, "chain": chain},
                "run_id": self.workspace.run_id,
                "status": "prepared",
                "slither_project_root": "slither_project",
                "analysis_target": analysis_target,
                "compiler_version": preferred_settings.compiler_version,
                "solc_version": preferred_settings.solc_version,
                "solc_select": preferred_settings.solc_select,
                "linked_source_entries": linked_entries,
                "node_modules_links": node_modules_links,
                "remappings": preferred_settings.remappings,
                "solc_args": preferred_settings.solc_args,
                "config_path": config_path,
                "preferred_target": preferred_settings.prepared_target,
                "preferred_working_dir": preferred_settings.working_dir,
                "preferred_source_root": preferred_settings.source_root,
            }),
        )?;

        self.record(
            "prepare_slither_project",
            &remappings_path,
            "prep",
            "executed",
            "Prepared Slither remappings.",
        );
        self.record(
            "prepare_slither_project",
            &config_path,
            "prep",
            "executed",
            "Prepared Slither config metadata.",
        );
        self.record(
            "prepare_slither_project",
            &manifest_path,
            "prep",
            "executed",
            "Prepared a deterministic Slither project manifest.",
        );
        Ok("prepared".to_string())
    }

    pub fn prepare_tooling_workspaces(&mut self, address: &str, chain: &str) -> AppResult<String> {
        let bundle_payload = self.load_source_bundle_payload()?;
        let source_status = bundle_payload
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("source_not_fetched")
            .to_string();
        let slither_status = self.prepare_slither_project(address, chain)?;
        let foundry_status = self.prepare_foundry_project(address, chain, &bundle_payload)?;
        let echidna_status = self.prepare_echidna_project(address, chain, &bundle_payload)?;
        let status = if source_status == "fetched"
            && slither_status == "prepared"
            && foundry_status == "prepared"
            && echidna_status == "prepared"
        {
            "prepared".to_string()
        } else {
            source_status.clone()
        };
        let manifest_path = self.workspace.write_json(
            "artifacts/tooling_manifest.json",
            &json!({
                "target": {"address": address, "chain": chain},
                "run_id": self.workspace.run_id,
                "status": status,
                "source_fetch_status": source_status,
                "workspaces": {
                    "slither": {
                        "status": slither_status,
                        "manifest_path": "slither_project/build_manifest.json",
                    },
                    "foundry": {
                        "status": foundry_status,
                        "manifest_path": "foundry_project/build_manifest.json",
                    },
                    "echidna": {
                        "status": echidna_status,
                        "manifest_path": "echidna_project/build_manifest.json",
                    }
                }
            }),
        )?;
        self.record(
            "prepare_tooling_workspaces",
            &manifest_path,
            "prep",
            &status,
            "Prepared standard working directories for supported analysis tools.",
        );
        Ok(status)
    }

    pub fn aggregate_materials(&mut self, address: &str, chain: &str) -> AppResult<String> {
        let mut optional_tool_artifacts = self.existing_paths(&[
            "artifacts/chain_checks_plan.json",
            "artifacts/chain_checks_output.txt",
            "artifacts/chain_checks_findings.json",
            "artifacts/chain_index.json",
            "artifacts/static_plan.json",
            "artifacts/slither_raw.json",
            "artifacts/static_findings.json",
            "artifacts/analyzer_index.json",
            "artifacts/tooling_manifest.json",
            "slither_project/build_manifest.json",
            "slither_project/remappings.txt",
            "slither_project/slither_inputs.json",
            "foundry_project/build_manifest.json",
            "foundry_project/foundry.toml",
            "foundry_project/remappings.txt",
            "echidna_project/build_manifest.json",
            "echidna_project/echidna.yaml",
        ]);
        optional_tool_artifacts.extend(self.existing_tree(&[
            "artifacts/analyzer",
            "artifacts/chain",
            "foundry_project/src",
            "foundry_project/test",
            "foundry_project/script",
            "foundry_project/lib",
            "foundry_project/node_modules",
            "echidna_project/src",
            "echidna_project/test",
            "echidna_project/lib",
            "echidna_project/node_modules",
        ])?);
        let manifest_path = self.workspace.write_json(
            "reports/materials_manifest.json",
            &json!({
                "target": {"address": address, "chain": chain},
                "run_id": self.workspace.run_id,
                "statuses": self.material_status_snapshot()?,
                "inputs": self.existing_paths(&["input/request.json", "input/source_request.json"]),
                "core_materials": self.existing_paths(&["artifacts/source_bundle.json", "artifacts/dependency_findings.json"]),
                "optional_tool_artifacts": optional_tool_artifacts,
                "artifact_records": self.artifacts,
                "notes": [
                    "This manifest is a neutral map of prepared review materials.",
                    "Use it to locate evidence; do not treat it as an audit conclusion.",
                    "Repository-side findings, when present, live in artifacts/dependency_findings.json.",
                    "Directly-invoked tools may leave optional artifacts under runs/<run_id>/artifacts/ that are not produced by the CLI itself.",
                ],
            }),
        )?;
        self.record(
            "aggregate_materials",
            &manifest_path,
            "report",
            "executed",
            "Stored a neutral manifest of prepared review materials.",
        );
        Ok(manifest_path)
    }

    pub fn material_status_snapshot(&self) -> AppResult<Value> {
        let source_payload =
            read_json_if_exists(&self.workspace.root.join("artifacts/source_bundle.json"))?;
        let dependency_payload = read_json_if_exists(
            &self
                .workspace
                .root
                .join("artifacts/dependency_findings.json"),
        )?;
        let mut source_status = source_payload
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("not_prepared")
            .to_string();
        if source_status == "fetched" {
            source_status = "source_fetched".to_string();
        }
        Ok(json!({
            "source_fetch_status": source_status,
            "dependency_analysis_status": dependency_payload.get("status").and_then(Value::as_str).unwrap_or("not_prepared"),
        }))
    }

    pub fn load_source_bundle_payload(&self) -> AppResult<Value> {
        let path = self.workspace.root.join("artifacts/source_bundle.json");
        if !path.exists() {
            return Ok(Value::Object(Map::new()));
        }
        let text = fs::read_to_string(path)?;
        let payload = serde_json::from_str::<Value>(&text)?;
        if !payload.is_object() {
            return Ok(Value::Object(Map::new()));
        }
        Ok(payload)
    }

    pub fn write_artifact_index(&self) -> AppResult<String> {
        self.workspace.write_json(
            "artifacts/artifact_index.json",
            &json!({
                "run_id": self.workspace.run_id,
                "artifacts": self.artifacts,
            }),
        )
    }

    fn record(&mut self, step: &str, path: &str, kind: &str, status: &str, summary: &str) {
        self.artifacts
            .retain(|item| !(item.path == path && item.step == step && item.kind == kind));
        self.artifacts.push(ArtifactRecord {
            step: step.to_string(),
            path: path.to_string(),
            kind: kind.to_string(),
            status: status.to_string(),
            summary: summary.to_string(),
        });
    }

    fn write_fetched_source_files(
        &mut self,
        files: &[super::source_fetch::SourceFile],
        prefix: &str,
        summary_prefix: &str,
    ) -> AppResult<Vec<Value>> {
        let mut written = Vec::new();
        for source_file in files {
            let final_path = if prefix.is_empty() {
                source_file.path.clone()
            } else {
                format!("{prefix}/{}", source_file.path)
            };
            self.workspace
                .write_text(&format!("sources/{final_path}"), &source_file.content)?;
            written.push(json!({
                "path": final_path,
                "length": source_file.content.len(),
                "original_path": source_file.path,
            }));
            self.record(
                "fetch_contract_source",
                &format!("sources/{final_path}"),
                "source",
                "executed",
                summary_prefix,
            );
        }
        Ok(written)
    }

    fn fetch_dependency_bundle_record(
        &mut self,
        address: &str,
        chain: &str,
        role: &str,
        name: &str,
        prefix: &str,
    ) -> AppResult<Value> {
        let Some(base_url) = self.config.source_api_base.clone() else {
            return Ok(json!({
                "role": role,
                "name": name,
                "address": address,
                "status": "fetch_failed",
                "error": "missing source API base",
            }));
        };

        let bundle = match fetch_verified_source(
            &base_url,
            self.config.source_api_key.as_deref(),
            &self.config.source_api_headers,
            address,
            chain,
        ) {
            Ok(bundle) => bundle,
            Err(error) => {
                return Ok(json!({
                    "role": role,
                    "name": name,
                    "address": address,
                    "status": "fetch_failed",
                    "error": error.to_string(),
                }));
            }
        };

        let response_artifact = self.workspace.write_json(
            &format!(
                "artifacts/source_provider_response_{}.json",
                prefix.replace('/', "_")
            ),
            &bundle.provider_payload,
        )?;
        let written_files = self.write_fetched_source_files(
            &bundle.files,
            prefix,
            "Stored a fetched dependency source file.",
        )?;

        let mut record = json!({
            "role": role,
            "name": name,
            "address": address,
            "provider": bundle.normalized_payload.get("provider").cloned().unwrap_or(Value::Object(Map::new())),
            "contract": bundle.normalized_payload.get("contract").cloned().unwrap_or(Value::Object(Map::new())),
            "compiler": bundle.normalized_payload.get("compiler").cloned().unwrap_or(Value::Object(Map::new())),
            "abi": bundle.normalized_payload.get("abi").cloned().unwrap_or(Value::Null),
            "source_layout": bundle.normalized_payload.get("source_layout").cloned().unwrap_or(Value::Null),
            "source_meta": bundle.normalized_payload.get("source_meta").cloned().unwrap_or(Value::Object(Map::new())),
            "files": written_files,
            "provider_response_artifact": response_artifact,
            "status": "fetched",
            "related_contracts": [],
        });
        self.record(
            "fetch_contract_source",
            &response_artifact,
            "artifact",
            "executed",
            "Stored the raw dependency provider response.",
        );

        let contract = bundle
            .normalized_payload
            .get("contract")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let implementation_address = contract
            .get("implementation")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        if contract.get("proxy").and_then(Value::as_bool) == Some(true)
            && !implementation_address.is_empty()
            && implementation_address.to_lowercase() != address.to_lowercase()
        {
            let nested = self.fetch_dependency_bundle_record(
                &implementation_address,
                chain,
                "implementation",
                &format!("{name}-implementation"),
                &format!("{prefix}/implementation"),
            )?;
            if let Some(array) = record
                .get_mut("related_contracts")
                .and_then(Value::as_array_mut)
            {
                array.push(nested);
            }
        }
        Ok(record)
    }

    fn fetch_discovered_dependencies(
        &mut self,
        candidates: Vec<Value>,
        target_address: &str,
        chain: &str,
        skip_addresses: BTreeSet<String>,
    ) -> AppResult<Vec<Value>> {
        let mut records = Vec::new();
        let mut seen = BTreeSet::new();
        seen.insert(target_address.to_lowercase());
        seen.extend(skip_addresses);
        for item in candidates {
            let Some(address) = item
                .get("address")
                .and_then(Value::as_str)
                .map(|s| s.to_lowercase())
            else {
                continue;
            };
            if address.is_empty() || seen.contains(&address) {
                continue;
            }
            seen.insert(address.clone());
            let role = item
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or("dependency");
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .unwrap_or(role);
            let safe_name = sanitize_dependency_name(name);
            let prefix = format!("dependencies/{role}/{safe_name}_{address}");
            let mut record =
                self.fetch_dependency_bundle_record(&address, chain, role, name, &prefix)?;
            if let Some(object) = record.as_object_mut() {
                object.insert(
                    "discovery".to_string(),
                    json!({
                        "sources": item.get("sources").cloned().unwrap_or(Value::Array(Vec::new())),
                        "internal_type": item.get("internal_type").cloned().unwrap_or(Value::String(String::new())),
                        "solidity_type": item.get("solidity_type").cloned().unwrap_or(Value::String(String::new())),
                        "file": item.get("file").cloned().unwrap_or(Value::String(String::new())),
                    }),
                );
            }
            records.push(record);
        }
        Ok(records)
    }

    fn link_slither_source_entries(
        &self,
        sources_root: &Path,
        slither_root: &Path,
    ) -> AppResult<Vec<Value>> {
        let mut linked = Vec::new();
        let mut entries = fs::read_dir(sources_root)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let file_name = entry.file_name().to_string_lossy().to_string();
            let link_path = slither_root.join(&file_name);
            recreate_symlink(&link_path, &path)?;
            linked.push(json!({
                "path": file_name,
                "target": self.workspace.relative(&path)?,
                "kind": if path.is_dir() { "directory" } else { "file" },
            }));
        }
        Ok(linked)
    }

    fn create_slither_node_modules(
        &self,
        npm_root: &Path,
        node_modules_root: &Path,
    ) -> AppResult<Vec<Value>> {
        let mut links = Vec::new();
        if !npm_root.exists() {
            return Ok(links);
        }
        let mut entries = fs::read_dir(npm_root)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('@') {
                let mut scoped = fs::read_dir(&path)?.collect::<Result<Vec<_>, _>>()?;
                scoped.sort_by_key(|entry| entry.file_name());
                for package_dir in scoped {
                    let package_path = package_dir.path();
                    if !package_path.is_dir() {
                        continue;
                    }
                    let package_name = package_dir.file_name().to_string_lossy().to_string();
                    let (alias_name, version) = split_versioned_package_name(&package_name);
                    let link_path = node_modules_root.join(&name).join(&alias_name);
                    recreate_symlink(&link_path, &package_path)?;
                    links.push(json!({
                        "alias": format!("{name}/{alias_name}"),
                        "version": version,
                        "link_path": self.workspace.relative(&link_path)?,
                        "target": self.workspace.relative(&package_path)?,
                    }));
                }
            } else {
                let (alias_name, version) = split_versioned_package_name(&name);
                let link_path = node_modules_root.join(&alias_name);
                recreate_symlink(&link_path, &path)?;
                links.push(json!({
                    "alias": alias_name,
                    "version": version,
                    "link_path": self.workspace.relative(&link_path)?,
                    "target": self.workspace.relative(&path)?,
                }));
            }
        }
        Ok(links)
    }

    fn prepare_foundry_project(
        &mut self,
        address: &str,
        chain: &str,
        bundle_payload: &Value,
    ) -> AppResult<String> {
        let foundry_root = self.workspace.root.join("foundry_project");
        if bundle_payload.get("status").and_then(Value::as_str) != Some("fetched") {
            recreate_dir(&foundry_root)?;
            let manifest_path = self.workspace.write_json(
                "foundry_project/build_manifest.json",
                &json!({
                    "target": {"address": address, "chain": chain},
                    "run_id": self.workspace.run_id,
                    "status": "source_not_fetched",
                    "note": "Fetch verified source before preparing a Foundry project.",
                }),
            )?;
            self.record(
                "prepare_foundry_project",
                &manifest_path,
                "prep",
                "configured_not_executed",
                "Skipped Foundry project preparation because source fetching did not complete.",
            );
            return Ok("source_not_fetched".to_string());
        }

        let sources_root = self.workspace.root.join("sources");
        if !sources_root.exists() {
            recreate_dir(&foundry_root)?;
            let manifest_path = self.workspace.write_json(
                "foundry_project/build_manifest.json",
                &json!({
                    "target": {"address": address, "chain": chain},
                    "run_id": self.workspace.run_id,
                    "status": "source_files_missing",
                    "note": "Source bundle exists but sources/ is missing.",
                }),
            )?;
            self.record(
                "prepare_foundry_project",
                &manifest_path,
                "prep",
                "executed_with_error",
                "Failed Foundry project preparation because source files are missing.",
            );
            return Ok("source_files_missing".to_string());
        }

        let settings = tool_project_settings(bundle_payload);
        recreate_dir(&foundry_root)?;
        let source_links = self.link_tool_project_sources(
            &sources_root,
            &foundry_root.join("src"),
            Some(&settings.source_root),
        )?;
        let node_modules_links = self.create_slither_node_modules(
            &sources_root.join("npm"),
            &foundry_root.join("node_modules"),
        )?;
        let remappings = merge_unique_lists(&[
            settings.remappings.clone(),
            node_modules_remappings(&node_modules_links),
        ]);
        let remappings_path = self.workspace.write_text(
            "foundry_project/remappings.txt",
            &render_line_list(&remappings),
        )?;
        self.workspace
            .write_text("foundry_project/test/.gitkeep", "")?;
        self.workspace
            .write_text("foundry_project/script/.gitkeep", "")?;
        self.workspace
            .write_text("foundry_project/lib/.gitkeep", "")?;
        let foundry_toml_path = self.workspace.write_text(
            "foundry_project/foundry.toml",
            &render_foundry_toml(&settings, &remappings),
        )?;
        let manifest_path = self.workspace.write_json(
            "foundry_project/build_manifest.json",
            &json!({
                "target": {"address": address, "chain": chain},
                "run_id": self.workspace.run_id,
                "status": "prepared",
                "project_root": "foundry_project",
                "analysis_target": analysis_target_payload(bundle_payload),
                "source_links": source_links,
                "node_modules_links": node_modules_links,
                "compiler_version": settings.compiler_version,
                "solc_version": settings.solc_version,
                "optimizer_enabled": settings.optimizer_enabled,
                "optimizer_runs": settings.optimizer_runs,
                "evm_version": settings.evm_version,
                "remappings": remappings,
                "remappings_path": remappings_path,
                "foundry_toml_path": foundry_toml_path,
                "preferred_working_dir": "foundry_project",
                "preferred_target": settings.prepared_target,
                "preferred_source_root": settings.source_root,
                "test_dir": "foundry_project/test",
                "script_dir": "foundry_project/script",
            }),
        )?;
        self.record(
            "prepare_foundry_project",
            &remappings_path,
            "prep",
            "executed",
            "Prepared Foundry remappings.",
        );
        self.record(
            "prepare_foundry_project",
            &foundry_toml_path,
            "prep",
            "executed",
            "Prepared a deterministic Foundry config.",
        );
        self.record(
            "prepare_foundry_project",
            &manifest_path,
            "prep",
            "executed",
            "Prepared a deterministic Foundry project manifest.",
        );
        Ok("prepared".to_string())
    }

    fn prepare_echidna_project(
        &mut self,
        address: &str,
        chain: &str,
        bundle_payload: &Value,
    ) -> AppResult<String> {
        let echidna_root = self.workspace.root.join("echidna_project");
        if bundle_payload.get("status").and_then(Value::as_str) != Some("fetched") {
            recreate_dir(&echidna_root)?;
            let manifest_path = self.workspace.write_json(
                "echidna_project/build_manifest.json",
                &json!({
                    "target": {"address": address, "chain": chain},
                    "run_id": self.workspace.run_id,
                    "status": "source_not_fetched",
                    "note": "Fetch verified source before preparing an Echidna project.",
                }),
            )?;
            self.record(
                "prepare_echidna_project",
                &manifest_path,
                "prep",
                "configured_not_executed",
                "Skipped Echidna project preparation because source fetching did not complete.",
            );
            return Ok("source_not_fetched".to_string());
        }

        let sources_root = self.workspace.root.join("sources");
        if !sources_root.exists() {
            recreate_dir(&echidna_root)?;
            let manifest_path = self.workspace.write_json(
                "echidna_project/build_manifest.json",
                &json!({
                    "target": {"address": address, "chain": chain},
                    "run_id": self.workspace.run_id,
                    "status": "source_files_missing",
                    "note": "Source bundle exists but sources/ is missing.",
                }),
            )?;
            self.record(
                "prepare_echidna_project",
                &manifest_path,
                "prep",
                "executed_with_error",
                "Failed Echidna project preparation because source files are missing.",
            );
            return Ok("source_files_missing".to_string());
        }

        let settings = tool_project_settings(bundle_payload);
        recreate_dir(&echidna_root)?;
        let source_links = self.link_tool_project_sources(
            &sources_root,
            &echidna_root.join("src"),
            Some(&settings.source_root),
        )?;
        let node_modules_links = self.create_slither_node_modules(
            &sources_root.join("npm"),
            &echidna_root.join("node_modules"),
        )?;
        let remappings = merge_unique_lists(&[
            settings.remappings.clone(),
            node_modules_remappings(&node_modules_links),
        ]);
        self.workspace
            .write_text("echidna_project/test/.gitkeep", "")?;
        self.workspace
            .write_text("echidna_project/lib/.gitkeep", "")?;
        let config_path = self.workspace.write_text(
            "echidna_project/echidna.yaml",
            &render_echidna_yaml(&settings),
        )?;
        let manifest_path = self.workspace.write_json(
            "echidna_project/build_manifest.json",
            &json!({
                "target": {"address": address, "chain": chain},
                "run_id": self.workspace.run_id,
                "status": "prepared",
                "project_root": "echidna_project",
                "analysis_target": analysis_target_payload(bundle_payload),
                "source_links": source_links,
                "node_modules_links": node_modules_links,
                "compiler_version": settings.compiler_version,
                "solc_version": settings.solc_version,
                "optimizer_enabled": settings.optimizer_enabled,
                "optimizer_runs": settings.optimizer_runs,
                "evm_version": settings.evm_version,
                "remappings": remappings,
                "config_path": config_path,
                "preferred_working_dir": "echidna_project",
                "preferred_target": settings.prepared_target,
                "preferred_source_root": settings.source_root,
                "harness_dir": "echidna_project/test",
            }),
        )?;
        self.record(
            "prepare_echidna_project",
            &config_path,
            "prep",
            "executed",
            "Prepared an Echidna config scaffold.",
        );
        self.record(
            "prepare_echidna_project",
            &manifest_path,
            "prep",
            "executed",
            "Prepared a deterministic Echidna project manifest.",
        );
        Ok("prepared".to_string())
    }

    fn link_tool_project_sources(
        &self,
        sources_root: &Path,
        tool_src_root: &Path,
        source_root_filter: Option<&str>,
    ) -> AppResult<Vec<Value>> {
        let source_root_filter = source_root_filter
            .unwrap_or_default()
            .trim_matches('/')
            .to_string();
        let source_root_prefix = if source_root_filter.is_empty() {
            None
        } else {
            Some(format!("{source_root_filter}/"))
        };
        let mut linked = Vec::new();
        for entry in walkdir::WalkDir::new(sources_root).sort_by_file_name() {
            let entry = entry?;
            if !entry.file_type().is_file() {
                continue;
            }
            let relative = self.workspace.relative(entry.path())?;
            let source_relative = relative.trim_start_matches("sources/").to_string();
            if source_relative.starts_with("dependencies/") || source_relative.starts_with("npm/") {
                continue;
            }
            let mapped_path = if let Some(prefix) = &source_root_prefix {
                if source_relative == source_root_filter {
                    PathBuf::from(".")
                } else if let Some(stripped) = source_relative.strip_prefix(prefix) {
                    PathBuf::from(stripped)
                } else {
                    continue;
                }
            } else {
                PathBuf::from(&source_relative)
            };
            let link_path = tool_src_root.join(&mapped_path);
            recreate_symlink(&link_path, entry.path())?;
            let display_path = link_path
                .strip_prefix(tool_src_root)
                .unwrap_or(&mapped_path)
                .to_path_buf();
            linked.push(json!({
                "path": format_path_for_json(&display_path),
                "target": relative,
            }));
        }
        Ok(linked)
    }

    fn existing_paths(&self, relative_paths: &[&str]) -> Vec<String> {
        relative_paths
            .iter()
            .filter(|path| self.workspace.root.join(path).exists())
            .map(|path| (*path).to_string())
            .collect()
    }

    fn existing_tree(&self, relative_roots: &[&str]) -> AppResult<Vec<String>> {
        let mut existing = Vec::new();
        let mut seen = BTreeSet::new();
        for root in relative_roots {
            let path = self.workspace.root.join(root);
            if path.is_file() {
                if seen.insert((*root).to_string()) {
                    existing.push((*root).to_string());
                }
                continue;
            }
            if !path.exists() {
                continue;
            }
            for entry in walkdir::WalkDir::new(&path).sort_by_file_name() {
                let entry = entry?;
                if !entry.file_type().is_file() {
                    continue;
                }
                let relative = self.workspace.relative(entry.path())?;
                if seen.insert(relative.clone()) {
                    existing.push(relative);
                }
            }
        }
        Ok(existing)
    }
}

#[derive(Clone)]
struct SlitherSettings {
    target_path: String,
    source_root: String,
    prepared_root: String,
    prepared_target: String,
    working_dir: String,
    working_dir_token: String,
    compiler_version: String,
    solc_version: String,
    solc_select: Value,
    include_paths: Vec<String>,
    remappings: Vec<String>,
    remappings_file: String,
    solc_args: String,
}

#[derive(Clone)]
struct ToolProjectSettings {
    source_root: String,
    prepared_target: String,
    compiler_version: String,
    solc_version: String,
    optimizer_enabled: bool,
    optimizer_runs: u64,
    evm_version: String,
    remappings: Vec<String>,
}

fn analysis_target_from_bundle(
    address: &str,
    primary_contract: &Map<String, Value>,
    primary_files: &[Value],
    related_contracts: &[Value],
) -> Value {
    for related in related_contracts {
        if related.get("role").and_then(Value::as_str) == Some("implementation")
            && related.get("status").and_then(Value::as_str) == Some("fetched")
        {
            if let Some(first_path) = related
                .get("files")
                .and_then(Value::as_array)
                .and_then(|files| files.first())
                .and_then(|item| item.get("path"))
                .and_then(Value::as_str)
            {
                return json!({
                    "address": related.get("address").cloned().unwrap_or(Value::String(address.to_string())),
                    "contract_name": related.get("contract").and_then(Value::as_object).and_then(|obj| obj.get("name")).cloned().unwrap_or(Value::String(String::new())),
                    "path": first_path,
                    "role": "implementation",
                });
            }
        }
    }

    let preferred_path = primary_contract
        .get("file_name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let first_primary_path = if !preferred_path.is_empty()
        && primary_files
            .iter()
            .any(|item| item.get("path").and_then(Value::as_str) == Some(preferred_path.as_str()))
    {
        preferred_path
    } else {
        primary_files
            .first()
            .and_then(|item| item.get("path"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    json!({
        "address": address,
        "contract_name": primary_contract.get("name").cloned().unwrap_or(Value::String(String::new())),
        "path": first_primary_path,
        "role": "target",
    })
}

fn analysis_target_payload(bundle_payload: &Value) -> Value {
    let preferred_path = bundle_payload
        .get("contract")
        .and_then(Value::as_object)
        .and_then(|obj| obj.get("file_name"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if !preferred_path.is_empty() && record_for_path(bundle_payload, &preferred_path).is_some() {
        return json!({
            "address": bundle_payload.get("target").and_then(Value::as_object).and_then(|obj| obj.get("address")).cloned().unwrap_or(Value::String(String::new())),
            "contract_name": bundle_payload.get("contract").and_then(Value::as_object).and_then(|obj| obj.get("name")).cloned().unwrap_or(Value::String(String::new())),
            "path": preferred_path,
            "role": "target",
            "prepared_path": preferred_path,
        });
    }

    if let Some(analysis_target) = bundle_payload
        .get("analysis_target")
        .and_then(Value::as_object)
    {
        if !analysis_target
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .is_empty()
        {
            return json!({
                "address": analysis_target.get("address").cloned().unwrap_or_else(|| bundle_payload.get("target").and_then(Value::as_object).and_then(|obj| obj.get("address")).cloned().unwrap_or(Value::String(String::new()))),
                "contract_name": analysis_target.get("contract_name").cloned().unwrap_or(Value::String(String::new())),
                "path": analysis_target.get("path").cloned().unwrap_or(Value::String(String::new())),
                "role": analysis_target.get("role").cloned().unwrap_or(Value::String(String::new())),
                "prepared_path": analysis_target.get("path").cloned().unwrap_or(Value::String(String::new())),
            });
        }
    }

    let first_path = bundle_payload
        .get("files")
        .and_then(Value::as_array)
        .and_then(|files| files.first())
        .and_then(|item| item.get("path"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    json!({
        "address": bundle_payload.get("target").and_then(Value::as_object).and_then(|obj| obj.get("address")).cloned().unwrap_or(Value::String(String::new())),
        "contract_name": bundle_payload.get("contract").and_then(Value::as_object).and_then(|obj| obj.get("name")).cloned().unwrap_or(Value::String(String::new())),
        "path": first_path,
        "role": "target",
        "prepared_path": first_path,
    })
}

fn collect_bundle_records(bundle_payload: &Value) -> Vec<Value> {
    let mut records = vec![json!({
        "files": bundle_payload.get("files").cloned().unwrap_or(Value::Array(Vec::new())),
        "compiler": bundle_payload.get("compiler").cloned().unwrap_or(Value::Object(Map::new())),
        "source_meta": bundle_payload.get("source_meta").cloned().unwrap_or(Value::Object(Map::new())),
        "contract": bundle_payload.get("contract").cloned().unwrap_or(Value::Object(Map::new())),
        "role": "target",
        "address": bundle_payload.get("target").and_then(Value::as_object).and_then(|obj| obj.get("address")).cloned().unwrap_or(Value::String(String::new())),
    })];
    for key in ["dependencies", "related_contracts"] {
        if let Some(entries) = bundle_payload.get(key).and_then(Value::as_array) {
            for entry in entries {
                records.extend(collect_record_tree(entry));
            }
        }
    }
    records
}

fn collect_record_tree(record: &Value) -> Vec<Value> {
    let mut records = vec![record.clone()];
    if let Some(related) = record.get("related_contracts").and_then(Value::as_array) {
        for nested in related {
            records.extend(collect_record_tree(nested));
        }
    }
    records
}

fn record_for_path(bundle_payload: &Value, relative_path: &str) -> Option<Value> {
    for record in collect_bundle_records(bundle_payload) {
        if let Some(files) = record.get("files").and_then(Value::as_array) {
            for item in files {
                if item.get("path").and_then(Value::as_str) == Some(relative_path) {
                    return Some(record);
                }
            }
        }
    }
    None
}

fn compiler_version_for_path(bundle_payload: &Value, relative_path: &str) -> String {
    record_for_path(bundle_payload, relative_path)
        .and_then(|record| record.get("compiler").cloned())
        .and_then(|compiler| compiler.get("version").cloned())
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .unwrap_or_default()
}

fn source_meta_for_path(bundle_payload: &Value, relative_path: &str) -> Value {
    record_for_path(bundle_payload, relative_path)
        .and_then(|record| record.get("source_meta").cloned())
        .unwrap_or(Value::Object(Map::new()))
}

fn provider_remappings(source_meta: &Value) -> Vec<String> {
    source_meta
        .get("settings")
        .and_then(Value::as_object)
        .and_then(|obj| obj.get("remappings"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .filter(|entry| !entry.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn node_modules_remappings(node_modules_links: &[Value]) -> Vec<String> {
    node_modules_links
        .iter()
        .filter_map(|item| item.get("alias").and_then(Value::as_str))
        .map(|alias| alias.trim_matches('/').to_string())
        .filter(|alias| !alias.is_empty())
        .map(|alias| format!("{alias}/=node_modules/{alias}/"))
        .collect()
}

fn tool_project_settings(bundle_payload: &Value) -> ToolProjectSettings {
    let analysis_target = analysis_target_payload(bundle_payload);
    let target_path = analysis_target
        .get("path")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim_start_matches("./")
        .to_string();
    let source_root = path_parent_string(&target_path);
    let prepared_target = if source_root.is_empty() {
        target_path.clone()
    } else {
        target_path
            .strip_prefix(&format!("{source_root}/"))
            .unwrap_or(target_path.as_str())
            .to_string()
    };
    let compiler_version = compiler_version_for_path(bundle_payload, &target_path);
    let solc_version = extract_semver(&compiler_version);
    let source_meta = source_meta_for_path(bundle_payload, &target_path);
    let optimizer_enabled = compiler_optimizer_enabled(bundle_payload, &source_meta);
    let optimizer_runs = compiler_optimizer_runs(bundle_payload, &source_meta);
    let evm_version = compiler_evm_version(bundle_payload, &source_meta);
    let remappings = provider_remappings(&source_meta);
    ToolProjectSettings {
        source_root,
        prepared_target,
        compiler_version,
        solc_version,
        optimizer_enabled,
        optimizer_runs,
        evm_version,
        remappings,
    }
}

fn slither_target_settings(
    workspace: &RunWorkspace,
    bundle_payload: &Value,
    linked_entries: &[Value],
    node_modules_links: &[Value],
    target_path: &str,
) -> SlitherSettings {
    let normalized_target_path = target_path.trim_start_matches("./");
    let normalized_target_path = if normalized_target_path.is_empty() {
        ".".to_string()
    } else {
        normalized_target_path.to_string()
    };
    let source_root = slither_source_root_for_target(&normalized_target_path, linked_entries);
    let compiler_version = compiler_version_for_path(bundle_payload, &normalized_target_path);
    let solc_version = extract_semver(&compiler_version);
    let source_meta = source_meta_for_path(bundle_payload, &normalized_target_path);
    let provider_remappings = provider_remappings(&source_meta);
    let generated_remappings = node_modules_remappings(node_modules_links);
    let remappings = merge_unique_lists(&[provider_remappings, generated_remappings]);
    let use_project_root = !remappings.is_empty();
    let working_root = if use_project_root {
        String::new()
    } else {
        source_root.clone()
    };
    let prepared_root = if use_project_root {
        ".".to_string()
    } else if source_root.is_empty() {
        ".".to_string()
    } else {
        source_root.clone()
    };
    let prepared_target = if use_project_root {
        normalized_target_path.clone()
    } else {
        slither_relative_target_path(&normalized_target_path, &source_root)
    };
    let include_paths = slither_include_paths(&working_root, !node_modules_links.is_empty());
    let working_dir = if working_root.is_empty() {
        "slither_project".to_string()
    } else {
        format!("slither_project/{working_root}")
    };
    SlitherSettings {
        target_path: normalized_target_path.clone(),
        source_root: source_root.clone(),
        prepared_root,
        prepared_target: prepared_target.clone(),
        working_dir,
        working_dir_token: if working_root.is_empty() {
            ".".to_string()
        } else {
            working_root.clone()
        },
        compiler_version,
        solc_version: solc_version.clone(),
        solc_select: solc_select_status(workspace, &solc_version),
        include_paths: include_paths.clone(),
        remappings_file: slither_relative_from_working_dir(&working_root, "remappings.txt"),
        remappings,
        solc_args: slither_solc_args(&include_paths),
    }
}

fn compiler_optimizer_enabled(bundle_payload: &Value, source_meta: &Value) -> bool {
    source_meta
        .get("settings")
        .and_then(Value::as_object)
        .and_then(|obj| obj.get("optimizer"))
        .and_then(Value::as_object)
        .and_then(|obj| obj.get("enabled"))
        .and_then(Value::as_bool)
        .unwrap_or_else(|| {
            bundle_payload
                .get("compiler")
                .and_then(Value::as_object)
                .and_then(|obj| obj.get("optimization_used"))
                .and_then(Value::as_str)
                .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
                .unwrap_or(false)
        })
}

fn compiler_optimizer_runs(bundle_payload: &Value, source_meta: &Value) -> u64 {
    source_meta
        .get("settings")
        .and_then(Value::as_object)
        .and_then(|obj| obj.get("optimizer"))
        .and_then(Value::as_object)
        .and_then(|obj| obj.get("runs"))
        .and_then(Value::as_u64)
        .or_else(|| {
            bundle_payload
                .get("compiler")
                .and_then(Value::as_object)
                .and_then(|obj| obj.get("runs"))
                .and_then(Value::as_str)
                .and_then(|value| value.parse::<u64>().ok())
        })
        .unwrap_or(200)
}

fn compiler_evm_version(bundle_payload: &Value, source_meta: &Value) -> String {
    let meta_value = source_meta
        .get("settings")
        .and_then(Value::as_object)
        .and_then(|obj| obj.get("evmVersion"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if !meta_value.is_empty() && meta_value != "Default" {
        return meta_value;
    }
    let compiler_value = bundle_payload
        .get("compiler")
        .and_then(Value::as_object)
        .and_then(|obj| obj.get("evm_version"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if compiler_value.is_empty() || compiler_value == "Default" {
        String::new()
    } else {
        compiler_value
    }
}

fn path_parent_string(path: &str) -> String {
    Path::new(path)
        .parent()
        .map(|parent| {
            let rendered = parent.to_string_lossy().replace('\\', "/");
            if rendered == "." {
                String::new()
            } else {
                rendered
            }
        })
        .unwrap_or_default()
}

fn render_foundry_toml(settings: &ToolProjectSettings, remappings: &[String]) -> String {
    let mut lines = vec![
        "[profile.default]".to_string(),
        "src = \"src\"".to_string(),
        "test = \"test\"".to_string(),
        "script = \"script\"".to_string(),
        "out = \"out\"".to_string(),
        "libs = [\"lib\", \"node_modules\"]".to_string(),
    ];
    if !settings.solc_version.is_empty() {
        lines.push(format!("solc = \"{}\"", settings.solc_version));
    }
    lines.push(format!("optimizer = {}", settings.optimizer_enabled));
    lines.push(format!("optimizer_runs = {}", settings.optimizer_runs));
    if !settings.evm_version.is_empty() {
        lines.push(format!("evm_version = \"{}\"", settings.evm_version));
    }
    if !remappings.is_empty() {
        let rendered = remappings
            .iter()
            .map(|entry| format!("\"{}\"", entry.replace('\\', "\\\\").replace('\"', "\\\"")))
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!("remappings = [{rendered}]"));
    }
    lines.push(String::new());
    lines.join("\n")
}

fn render_echidna_yaml(settings: &ToolProjectSettings) -> String {
    let mut lines = vec![
        format!("testMode: \"property\""),
        format!("format: \"text\""),
        format!("corpusDir: \"corpus\""),
        format!("srcDir: \"src\""),
        format!("testDir: \"test\""),
    ];
    if !settings.prepared_target.is_empty() {
        lines.push(format!("prefix: \"{}\"", settings.prepared_target));
    }
    lines.push(String::new());
    lines.join("\n")
}

fn render_line_list(items: &[String]) -> String {
    if items.is_empty() {
        String::new()
    } else {
        format!("{}\n", items.join("\n"))
    }
}

fn format_path_for_json(path: &Path) -> String {
    let rendered = path.to_string_lossy().replace('\\', "/");
    if rendered.is_empty() || rendered == "." {
        ".".to_string()
    } else {
        rendered
    }
}

fn slither_source_root_for_target(target_path: &str, linked_entries: &[Value]) -> String {
    let normalized_target_path = target_path.trim_start_matches("./");
    linked_entries
        .iter()
        .filter_map(|entry| entry.get("path").and_then(Value::as_str))
        .map(|path| path.trim_matches('/').to_string())
        .filter(|source_root| {
            !source_root.is_empty()
                && (normalized_target_path == source_root
                    || normalized_target_path.starts_with(&format!("{source_root}/")))
        })
        .max_by_key(|item| item.len())
        .unwrap_or_default()
}

fn slither_relative_target_path(target_path: &str, source_root: &str) -> String {
    let normalized_target_path = target_path.trim_start_matches("./");
    let normalized_target_path = if normalized_target_path.is_empty() {
        "."
    } else {
        normalized_target_path
    };
    if source_root.is_empty() {
        return normalized_target_path.to_string();
    }
    if normalized_target_path == source_root {
        return ".".to_string();
    }
    let prefix = format!("{source_root}/");
    if let Some(stripped) = normalized_target_path.strip_prefix(&prefix) {
        if stripped.is_empty() {
            ".".to_string()
        } else {
            stripped.to_string()
        }
    } else {
        normalized_target_path.to_string()
    }
}

fn slither_relative_from_working_dir(source_root: &str, path_in_slither_root: &str) -> String {
    if source_root.is_empty() {
        path_in_slither_root.to_string()
    } else {
        pathdiff::diff_paths(path_in_slither_root, source_root)
            .unwrap_or_else(|| PathBuf::from(path_in_slither_root))
            .to_string_lossy()
            .replace('\\', "/")
    }
}

fn slither_include_paths(source_root: &str, has_node_modules: bool) -> Vec<String> {
    let mut include_paths = vec![".".to_string()];
    if has_node_modules {
        let node_modules_path = slither_relative_from_working_dir(source_root, "node_modules");
        if !include_paths.contains(&node_modules_path) {
            include_paths.push(node_modules_path);
        }
    }
    include_paths
}

fn slither_solc_args(include_paths: &[String]) -> String {
    let mut args = vec!["--base-path".to_string(), ".".to_string()];
    let mut allow_paths = vec![".".to_string()];
    for entry in include_paths {
        if entry == "." {
            continue;
        }
        args.push("--include-path".to_string());
        args.push(entry.clone());
        allow_paths.push(entry.clone());
    }
    args.push("--allow-paths".to_string());
    args.push(allow_paths.join(","));
    args.join(" ")
}

fn solc_select_status(workspace: &RunWorkspace, requested_version: &str) -> Value {
    if requested_version.is_empty() {
        return json!({
            "requested_version": "",
            "is_installed": false,
            "current_version": "",
            "available_versions": [],
            "recommended_action": "No semantic compiler version could be extracted from source metadata.",
        });
    }

    let output = Command::new("nix")
        .args(["develop", ".#default", "-c", "solc-select", "versions"])
        .current_dir(&workspace.project_root)
        .output();
    let output = match output {
        Ok(output) => output,
        Err(error) => {
            return json!({
                "requested_version": requested_version,
                "is_installed": false,
                "current_version": "",
                "available_versions": [],
                "recommended_action": format!("Could not query solc-select versions: {error}"),
            });
        }
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let pattern = Regex::new(r"(?P<version>\d+\.\d+\.\d+)(?:\s+\(current.*\))?$")
        .expect("valid solc-select regex");
    let mut available_versions = Vec::new();
    let mut current_version = String::new();
    for raw_line in stdout.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(capture) = pattern.captures(line) {
            let version = capture
                .name("version")
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();
            available_versions.push(version.clone());
            if line.contains("(current") {
                current_version = version;
            }
        }
    }
    let is_installed = available_versions
        .iter()
        .any(|version| version == requested_version);
    let recommended_action = if is_installed {
        format!(
            "Run `solc-select use {requested_version}` inside the devShell before invoking Slither."
        )
    } else {
        format!(
            "`{requested_version}` is not installed in solc-select. Install or select it before Slither, for example with `solc-select install {requested_version} && solc-select use {requested_version}`."
        )
    };
    json!({
        "requested_version": requested_version,
        "is_installed": is_installed,
        "current_version": current_version,
        "available_versions": available_versions,
        "recommended_action": recommended_action,
        "command_status": if output.status.success() { "ok" } else { "error" },
        "stderr_preview": stderr.chars().take(1000).collect::<String>(),
    })
}

fn split_versioned_package_name(name: &str) -> (String, String) {
    let pattern =
        Regex::new(r"^(?P<package>.+)@(?P<version>\d[\w.+-]*)$").expect("valid package regex");
    if let Some(capture) = pattern.captures(name) {
        (
            capture
                .name("package")
                .map(|m| m.as_str().to_string())
                .unwrap_or_else(|| name.to_string()),
            capture
                .name("version")
                .map(|m| m.as_str().to_string())
                .unwrap_or_default(),
        )
    } else {
        (name.to_string(), String::new())
    }
}

fn recreate_dir(path: &Path) -> AppResult<()> {
    if path.exists() {
        fs::remove_dir_all(path)?;
    }
    fs::create_dir_all(path)?;
    Ok(())
}

fn recreate_symlink(link_path: &Path, target_path: &Path) -> AppResult<()> {
    if link_path.exists() || link_path.symlink_metadata().is_ok() {
        let metadata = link_path.symlink_metadata()?;
        if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
            fs::remove_dir_all(link_path)?;
        } else {
            fs::remove_file(link_path)?;
        }
    }
    if let Some(parent) = link_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let relative_target = pathdiff::diff_paths(
        target_path,
        link_path.parent().unwrap_or_else(|| Path::new(".")),
    )
    .unwrap_or_else(|| target_path.to_path_buf());
    #[cfg(unix)]
    std::os::unix::fs::symlink(relative_target, link_path)?;
    #[cfg(windows)]
    {
        if target_path.is_dir() {
            std::os::windows::fs::symlink_dir(relative_target, link_path)?;
        } else {
            std::os::windows::fs::symlink_file(relative_target, link_path)?;
        }
    }
    Ok(())
}

fn load_existing_artifacts(workspace: &RunWorkspace) -> Vec<ArtifactRecord> {
    let path = workspace.root.join("artifacts/artifact_index.json");
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(payload) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    payload
        .get("artifacts")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| serde_json::from_value::<ArtifactRecord>(item.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

fn read_json_if_exists(path: &Path) -> AppResult<Value> {
    if !path.exists() {
        return Ok(Value::Object(Map::new()));
    }
    let text = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&text).unwrap_or(Value::Object(Map::new())))
}
