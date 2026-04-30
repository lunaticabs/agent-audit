use std::path::{Path, PathBuf};
use std::process::Command;

use regex::Regex;
use serde_json::Value;

use crate::error::AppResult;
use crate::models::run::RunTarget;
use crate::models::source::{SourceBundleArtifact, SourceMetadata};
use crate::models::tooling::{
    EchidnaBuildManifest, FoundryBuildManifest, NodeModuleLink, RunArtifactHeader,
    SlitherBuildManifest, SlitherInputsArtifact, SolcSelectStatus, SourceLink, SourceLinkKind,
    ToolWorkspaceManifest, ToolWorkspaceManifestSet, ToolingManifest,
};
use crate::services::source_provider::{extract_semver, merge_unique_lists};
use crate::workspace::RunWorkspace;

use super::AuditPipelineService;
use super::source::{
    analysis_target_for_prepared, compiler_version_for_path, source_meta_for_path,
};
use super::support::{
    format_path_for_json, path_parent_string, recreate_dir, recreate_symlink, render_line_list,
};

impl AuditPipelineService {
    pub fn prepare_slither_project(&mut self, address: &str, chain: &str) -> AppResult<String> {
        let slither_root = self.workspace.root.join("slither_project");
        let bundle_payload = self.load_source_bundle_payload()?;
        if !bundle_payload.is_fetched() {
            recreate_dir(&slither_root)?;
            let manifest_path = self.workspace.write_json(
                "slither_project/build_manifest.json",
                &SlitherBuildManifest {
                    header: build_header(
                        address,
                        chain,
                        &self.workspace.run_id,
                        "source_not_fetched",
                    ),
                    note: Some(
                        "Fetch verified source before preparing a Slither project.".to_string(),
                    ),
                    ..SlitherBuildManifest::default()
                },
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
                &SlitherBuildManifest {
                    header: build_header(
                        address,
                        chain,
                        &self.workspace.run_id,
                        "source_files_missing",
                    ),
                    note: Some("Source bundle exists but sources/ is missing.".to_string()),
                    ..SlitherBuildManifest::default()
                },
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
        let analysis_target = analysis_target_for_prepared(&bundle_payload);
        let preferred_settings = slither_target_settings(
            &self.workspace,
            &bundle_payload,
            &linked_entries,
            &node_modules_links,
            &analysis_target.path,
        );
        let prepared_analysis_target = analysis_target.with_prepared(
            preferred_settings.prepared_target.clone(),
            preferred_settings.prepared_root.clone(),
        );

        let remappings_path = self.workspace.write_text(
            "slither_project/remappings.txt",
            &render_line_list(&preferred_settings.remappings),
        )?;
        let config_path = self.workspace.write_json(
            "slither_project/slither_inputs.json",
            &SlitherInputsArtifact {
                status: "prepared".to_string(),
                working_dir: preferred_settings.working_dir_token.clone(),
                base_path: ".".to_string(),
                include_paths: preferred_settings.include_paths.clone(),
                remappings_file: preferred_settings.remappings_file.clone(),
                remappings: preferred_settings.remappings.clone(),
                solc_args: preferred_settings.solc_args.clone(),
                target_path: preferred_settings.target_path.clone(),
                prepared_target: preferred_settings.prepared_target.clone(),
            },
        )?;
        let manifest_path = self.workspace.write_json(
            "slither_project/build_manifest.json",
            &SlitherBuildManifest {
                header: build_header(address, chain, &self.workspace.run_id, "prepared"),
                slither_project_root: "slither_project".to_string(),
                analysis_target: Some(prepared_analysis_target),
                compiler_version: preferred_settings.compiler_version,
                solc_version: preferred_settings.solc_version,
                solc_select: Some(preferred_settings.solc_select),
                linked_source_entries: linked_entries,
                node_modules_links,
                remappings: preferred_settings.remappings,
                solc_args: preferred_settings.solc_args,
                config_path: config_path.clone(),
                preferred_target: preferred_settings.prepared_target,
                preferred_working_dir: preferred_settings.working_dir,
                preferred_source_root: preferred_settings.source_root,
                ..SlitherBuildManifest::default()
            },
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
        let source_status = if bundle_payload.is_fetched() {
            "fetched".to_string()
        } else if bundle_payload.status.is_empty() {
            "source_not_fetched".to_string()
        } else {
            bundle_payload.status.clone()
        };
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
            &ToolingManifest {
                header: build_header(address, chain, &self.workspace.run_id, &status),
                source_fetch_status: source_status,
                workspaces: ToolWorkspaceManifestSet {
                    slither: ToolWorkspaceManifest {
                        status: slither_status,
                        manifest_path: "slither_project/build_manifest.json".to_string(),
                    },
                    foundry: ToolWorkspaceManifest {
                        status: foundry_status,
                        manifest_path: "foundry_project/build_manifest.json".to_string(),
                    },
                    echidna: ToolWorkspaceManifest {
                        status: echidna_status,
                        manifest_path: "echidna_project/build_manifest.json".to_string(),
                    },
                },
            },
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

    fn link_slither_source_entries(
        &self,
        sources_root: &Path,
        slither_root: &Path,
    ) -> AppResult<Vec<SourceLink>> {
        let mut linked = Vec::new();
        let mut entries = std::fs::read_dir(sources_root)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let file_name = entry.file_name().to_string_lossy().to_string();
            let link_path = slither_root.join(&file_name);
            recreate_symlink(&link_path, &path)?;
            linked.push(SourceLink {
                path: file_name,
                target: self.workspace.relative(&path)?,
                kind: Some(if path.is_dir() {
                    SourceLinkKind::Directory
                } else {
                    SourceLinkKind::File
                }),
            });
        }
        Ok(linked)
    }

    fn create_slither_node_modules(
        &self,
        npm_root: &Path,
        node_modules_root: &Path,
    ) -> AppResult<Vec<NodeModuleLink>> {
        let mut links = Vec::new();
        if !npm_root.exists() {
            return Ok(links);
        }
        let mut entries = std::fs::read_dir(npm_root)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('@') {
                let mut scoped = std::fs::read_dir(&path)?.collect::<Result<Vec<_>, _>>()?;
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
                    links.push(NodeModuleLink {
                        alias: format!("{name}/{alias_name}"),
                        version,
                        link_path: self.workspace.relative(&link_path)?,
                        target: self.workspace.relative(&package_path)?,
                    });
                }
            } else {
                let (alias_name, version) = split_versioned_package_name(&name);
                let link_path = node_modules_root.join(&alias_name);
                recreate_symlink(&link_path, &path)?;
                links.push(NodeModuleLink {
                    alias: alias_name,
                    version,
                    link_path: self.workspace.relative(&link_path)?,
                    target: self.workspace.relative(&path)?,
                });
            }
        }
        Ok(links)
    }

    fn prepare_foundry_project(
        &mut self,
        address: &str,
        chain: &str,
        bundle_payload: &SourceBundleArtifact,
    ) -> AppResult<String> {
        let foundry_root = self.workspace.root.join("foundry_project");
        if !bundle_payload.is_fetched() {
            recreate_dir(&foundry_root)?;
            let manifest_path = self.workspace.write_json(
                "foundry_project/build_manifest.json",
                &FoundryBuildManifest {
                    header: build_header(
                        address,
                        chain,
                        &self.workspace.run_id,
                        "source_not_fetched",
                    ),
                    note: Some(
                        "Fetch verified source before preparing a Foundry project.".to_string(),
                    ),
                    ..FoundryBuildManifest::default()
                },
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
                &FoundryBuildManifest {
                    header: build_header(
                        address,
                        chain,
                        &self.workspace.run_id,
                        "source_files_missing",
                    ),
                    note: Some("Source bundle exists but sources/ is missing.".to_string()),
                    ..FoundryBuildManifest::default()
                },
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
            &FoundryBuildManifest {
                header: build_header(address, chain, &self.workspace.run_id, "prepared"),
                project_root: "foundry_project".to_string(),
                analysis_target: Some(analysis_target_for_prepared(bundle_payload)),
                source_links,
                node_modules_links,
                compiler_version: settings.compiler_version,
                solc_version: settings.solc_version,
                optimizer_enabled: settings.optimizer_enabled,
                optimizer_runs: settings.optimizer_runs,
                evm_version: settings.evm_version,
                remappings,
                remappings_path: remappings_path.clone(),
                foundry_toml_path: foundry_toml_path.clone(),
                preferred_working_dir: "foundry_project".to_string(),
                preferred_target: settings.prepared_target,
                preferred_source_root: settings.source_root,
                test_dir: "foundry_project/test".to_string(),
                script_dir: "foundry_project/script".to_string(),
                ..FoundryBuildManifest::default()
            },
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
        bundle_payload: &SourceBundleArtifact,
    ) -> AppResult<String> {
        let echidna_root = self.workspace.root.join("echidna_project");
        if !bundle_payload.is_fetched() {
            recreate_dir(&echidna_root)?;
            let manifest_path = self.workspace.write_json(
                "echidna_project/build_manifest.json",
                &EchidnaBuildManifest {
                    header: build_header(
                        address,
                        chain,
                        &self.workspace.run_id,
                        "source_not_fetched",
                    ),
                    note: Some(
                        "Fetch verified source before preparing an Echidna project.".to_string(),
                    ),
                    ..EchidnaBuildManifest::default()
                },
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
                &EchidnaBuildManifest {
                    header: build_header(
                        address,
                        chain,
                        &self.workspace.run_id,
                        "source_files_missing",
                    ),
                    note: Some("Source bundle exists but sources/ is missing.".to_string()),
                    ..EchidnaBuildManifest::default()
                },
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
            &EchidnaBuildManifest {
                header: build_header(address, chain, &self.workspace.run_id, "prepared"),
                project_root: "echidna_project".to_string(),
                analysis_target: Some(analysis_target_for_prepared(bundle_payload)),
                source_links,
                node_modules_links,
                compiler_version: settings.compiler_version,
                solc_version: settings.solc_version,
                optimizer_enabled: settings.optimizer_enabled,
                optimizer_runs: settings.optimizer_runs,
                evm_version: settings.evm_version,
                remappings,
                config_path: config_path.clone(),
                preferred_working_dir: "echidna_project".to_string(),
                preferred_target: settings.prepared_target,
                preferred_source_root: settings.source_root,
                harness_dir: "echidna_project/test".to_string(),
                ..EchidnaBuildManifest::default()
            },
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
    ) -> AppResult<Vec<SourceLink>> {
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
            linked.push(SourceLink {
                path: format_path_for_json(&display_path),
                target: relative,
                kind: None,
            });
        }
        Ok(linked)
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
    solc_select: SolcSelectStatus,
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

fn build_header(address: &str, chain: &str, run_id: &str, status: &str) -> RunArtifactHeader {
    RunArtifactHeader {
        target: RunTarget::new(address, chain),
        run_id: run_id.to_string(),
        status: status.to_string(),
    }
}

fn provider_remappings(source_meta: Option<&SourceMetadata>) -> Vec<String> {
    source_meta
        .and_then(|meta| meta.settings.get("remappings"))
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

fn node_modules_remappings(node_modules_links: &[NodeModuleLink]) -> Vec<String> {
    node_modules_links
        .iter()
        .map(|item| item.alias.trim_matches('/').to_string())
        .filter(|alias| !alias.is_empty())
        .map(|alias| format!("{alias}/=node_modules/{alias}/"))
        .collect()
}

fn tool_project_settings(bundle_payload: &SourceBundleArtifact) -> ToolProjectSettings {
    let analysis_target = analysis_target_for_prepared(bundle_payload);
    let target_path = analysis_target.path.trim_start_matches("./").to_string();
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
    let optimizer_enabled = compiler_optimizer_enabled(bundle_payload, source_meta);
    let optimizer_runs = compiler_optimizer_runs(bundle_payload, source_meta);
    let evm_version = compiler_evm_version(bundle_payload, source_meta);
    let remappings = provider_remappings(source_meta);
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
    bundle_payload: &SourceBundleArtifact,
    linked_entries: &[SourceLink],
    node_modules_links: &[NodeModuleLink],
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
    let provider_remappings = provider_remappings(source_meta);
    let generated_remappings = node_modules_remappings(node_modules_links);
    let remappings = merge_unique_lists(&[provider_remappings, generated_remappings]);
    let use_project_root = !remappings.is_empty();
    let working_root = if use_project_root {
        String::new()
    } else {
        source_root.clone()
    };
    let prepared_root = if use_project_root || source_root.is_empty() {
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

fn compiler_optimizer_enabled(
    bundle_payload: &SourceBundleArtifact,
    source_meta: Option<&SourceMetadata>,
) -> bool {
    source_meta
        .and_then(|meta| meta.settings.get("optimizer"))
        .and_then(Value::as_object)
        .and_then(|obj| obj.get("enabled"))
        .and_then(Value::as_bool)
        .unwrap_or_else(|| {
            bundle_payload
                .compiler
                .as_ref()
                .map(|compiler| {
                    compiler.optimization_used == "1"
                        || compiler.optimization_used.eq_ignore_ascii_case("true")
                })
                .unwrap_or(false)
        })
}

fn compiler_optimizer_runs(
    bundle_payload: &SourceBundleArtifact,
    source_meta: Option<&SourceMetadata>,
) -> u64 {
    source_meta
        .and_then(|meta| meta.settings.get("optimizer"))
        .and_then(Value::as_object)
        .and_then(|obj| obj.get("runs"))
        .and_then(Value::as_u64)
        .or_else(|| {
            bundle_payload
                .compiler
                .as_ref()
                .and_then(|compiler| compiler.runs.parse::<u64>().ok())
        })
        .unwrap_or(200)
}

fn compiler_evm_version(
    bundle_payload: &SourceBundleArtifact,
    source_meta: Option<&SourceMetadata>,
) -> String {
    let meta_value = source_meta
        .and_then(|meta| meta.settings.get("evmVersion"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if !meta_value.is_empty() && meta_value != "Default" {
        return meta_value;
    }
    let compiler_value = bundle_payload
        .compiler
        .as_ref()
        .map(|compiler| compiler.evm_version.trim().to_string())
        .unwrap_or_default();
    if compiler_value.is_empty() || compiler_value == "Default" {
        String::new()
    } else {
        compiler_value
    }
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
        "testMode: \"property\"".to_string(),
        "format: \"text\"".to_string(),
        "corpusDir: \"corpus\"".to_string(),
        "srcDir: \"src\"".to_string(),
        "testDir: \"test\"".to_string(),
    ];
    if !settings.prepared_target.is_empty() {
        lines.push(format!("prefix: \"{}\"", settings.prepared_target));
    }
    lines.push(String::new());
    lines.join("\n")
}

fn slither_source_root_for_target(target_path: &str, linked_entries: &[SourceLink]) -> String {
    let normalized_target_path = target_path.trim_start_matches("./");
    linked_entries
        .iter()
        .map(|entry| entry.path.trim_matches('/').to_string())
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

fn solc_select_status(workspace: &RunWorkspace, requested_version: &str) -> SolcSelectStatus {
    if requested_version.is_empty() {
        return SolcSelectStatus {
            requested_version: String::new(),
            is_installed: false,
            current_version: String::new(),
            available_versions: Vec::new(),
            recommended_action:
                "No semantic compiler version could be extracted from source metadata.".to_string(),
            command_status: String::new(),
            stderr_preview: String::new(),
        };
    }

    let output = Command::new("nix")
        .args(["develop", ".#default", "-c", "solc-select", "versions"])
        .current_dir(&workspace.project_root)
        .output();
    let output = match output {
        Ok(output) => output,
        Err(error) => {
            return SolcSelectStatus {
                requested_version: requested_version.to_string(),
                is_installed: false,
                current_version: String::new(),
                available_versions: Vec::new(),
                recommended_action: format!("Could not query solc-select versions: {error}"),
                command_status: "error".to_string(),
                stderr_preview: String::new(),
            };
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

    SolcSelectStatus {
        requested_version: requested_version.to_string(),
        is_installed,
        current_version,
        available_versions,
        recommended_action,
        command_status: if output.status.success() {
            "ok".to_string()
        } else {
            "error".to_string()
        },
        stderr_preview: stderr.chars().take(1000).collect(),
    }
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
