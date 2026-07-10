use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use regex::Regex;
use serde_json::Value;

use crate::error::AppResult;
use crate::models::artifact::{ArtifactKind, ArtifactStatus, ArtifactStep};
use crate::models::bytecode::{BytecodeAuditTarget, BytecodeTargetsArtifact};
use crate::models::identity::{ChainAlias, EvmAddress, RunId};
use crate::models::path::{RelativePath, WorkspaceRelPath};
use crate::models::run::RunTarget;
use crate::models::source::{SourceBundleArtifact, SourceMetadata};
use crate::models::step::StepStatus;
use crate::models::tooling::{
    EchidnaBuildManifest, FoundryBuildManifest, HeimdallAction, HeimdallBuildManifest,
    HeimdallCommandWorkspace, HeimdallTargetWorkspace, NodeModuleLink, RunArtifactHeader,
    SlitherBuildManifest, SlitherInputsArtifact, SolcSelectStatus, SourceLink, SourceLinkKind,
    ToolCommandStatus, ToolWorkspaceManifest, ToolWorkspaceManifestSet, ToolingManifest,
};
use crate::services::source_provider::{extract_semver, merge_unique_lists};
use crate::workspace::{RunWorkspace, paths};

use super::AuditPipelineService;
use super::source::{
    analysis_target_for_prepared, compiler_version_for_path, source_meta_for_path,
};
use super::support::{
    format_path_for_json, path_parent, recreate_dir, recreate_symlink, render_line_list,
};

const HEIMDALL_ROOT: &str = "artifacts/heimdall";

impl AuditPipelineService {
    pub fn prepare_slither_project(
        &mut self,
        address: &EvmAddress,
        chain: &ChainAlias,
    ) -> AppResult<StepStatus> {
        let slither_root = self.workspace.root().join("slither_project");
        let bundle_payload = self.load_source_bundle_payload()?;
        if !bundle_payload.is_fetched() {
            let (status, note, summary) = source_unavailable_tooling_precondition(
                &bundle_payload,
                "Slither",
                "Skipped Slither project preparation because source fetching did not complete.",
            );
            return self.prepare_slither_precondition(
                address,
                chain,
                &slither_root,
                PreconditionSpec {
                    status,
                    note,
                    artifact_status: StepStatus::ConfiguredNotExecuted,
                    summary,
                },
            );
        }

        let sources_root = self.workspace.root().join("sources");
        if !sources_root.exists() {
            return self.prepare_slither_precondition(
                address,
                chain,
                &slither_root,
                PreconditionSpec {
                    status: StepStatus::SourceFilesMissing,
                    note: "Source bundle exists but sources/ is missing.",
                    artifact_status: StepStatus::ExecutedWithError,
                    summary: "Failed Slither project preparation because source files are missing.",
                },
            );
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

        let remappings_path = self.workspace.store().write_text(
            "slither_project/remappings.txt",
            &render_line_list(&preferred_settings.remappings),
        )?;
        let config_path = self.workspace.store().write_json(
            "slither_project/slither_inputs.json",
            &SlitherInputsArtifact {
                status: StepStatus::Prepared,
                working_dir: preferred_settings.working_dir_token.clone(),
                base_path: RelativePath::dot(),
                include_paths: preferred_settings.include_paths.clone(),
                remappings_file: preferred_settings.remappings_file.clone(),
                remappings: preferred_settings.remappings.clone(),
                solc_args: preferred_settings.solc_args.clone(),
                target_path: preferred_settings.target_path.clone(),
                prepared_target: preferred_settings.prepared_target.clone(),
            },
        )?;
        self.record(
            ArtifactStep::PrepareSlitherProject,
            &remappings_path,
            ArtifactKind::Prep,
            ArtifactStatus::Executed,
            "Prepared Slither remappings.",
        );
        self.record(
            ArtifactStep::PrepareSlitherProject,
            &config_path,
            ArtifactKind::Prep,
            ArtifactStatus::Executed,
            "Prepared Slither config metadata.",
        );
        let manifest_path = self.workspace.store().write_json(
            paths::SLITHER_BUILD_MANIFEST,
            &SlitherBuildManifest {
                header: build_header(
                    address,
                    chain,
                    self.workspace.run_id(),
                    StepStatus::Prepared,
                ),
                slither_project_root: Some(WorkspaceRelPath::new("slither_project")),
                analysis_target: Some(prepared_analysis_target),
                compiler_version: preferred_settings.compiler_version,
                solc_version: preferred_settings.solc_version,
                solc_select: Some(preferred_settings.solc_select),
                linked_source_entries: linked_entries,
                node_modules_links,
                remappings: preferred_settings.remappings,
                solc_args: preferred_settings.solc_args,
                config_path: Some(config_path),
                preferred_target: Some(preferred_settings.prepared_target),
                preferred_working_dir: Some(preferred_settings.working_dir),
                preferred_source_root: preferred_settings.source_root,
                ..SlitherBuildManifest::default()
            },
        )?;
        self.record(
            ArtifactStep::PrepareSlitherProject,
            &manifest_path,
            ArtifactKind::Prep,
            ArtifactStatus::Executed,
            "Prepared a deterministic Slither project manifest.",
        );
        Ok(StepStatus::Prepared)
    }

    fn prepare_slither_precondition(
        &mut self,
        address: &EvmAddress,
        chain: &ChainAlias,
        slither_root: &Path,
        spec: PreconditionSpec<'_>,
    ) -> AppResult<StepStatus> {
        recreate_dir(slither_root)?;
        let manifest_path = self.workspace.store().write_json(
            paths::SLITHER_BUILD_MANIFEST,
            &SlitherBuildManifest {
                header: build_header(address, chain, self.workspace.run_id(), spec.status),
                note: Some(spec.note.to_string()),
                ..SlitherBuildManifest::default()
            },
        )?;
        self.record(
            ArtifactStep::PrepareSlitherProject,
            &manifest_path,
            ArtifactKind::Prep,
            spec.artifact_status,
            spec.summary,
        );
        Ok(spec.status)
    }

    pub fn prepare_tooling_workspaces(
        &mut self,
        address: &EvmAddress,
        chain: &ChainAlias,
    ) -> AppResult<StepStatus> {
        let bundle_payload = self.load_source_bundle_payload()?;
        let source_status = bundle_source_step_status(&bundle_payload);
        let slither_status = self.prepare_slither_project(address, chain)?;
        let foundry_status = self.prepare_foundry_project(address, chain, &bundle_payload)?;
        let echidna_status = self.prepare_echidna_project(address, chain, &bundle_payload)?;
        let heimdall_status = self.prepare_heimdall_workspace(
            address,
            chain,
            bundle_payload.is_fetched() || bundle_payload.is_source_unavailable(),
        )?;
        let status = aggregate_tooling_status(
            source_status,
            slither_status,
            foundry_status,
            echidna_status,
            heimdall_status,
        );
        let manifest_path = self.workspace.store().write_json(
            paths::TOOLING_MANIFEST,
            &ToolingManifest {
                header: build_header(address, chain, self.workspace.run_id(), status),
                source_fetch_status: source_status,
                workspaces: ToolWorkspaceManifestSet {
                    slither: ToolWorkspaceManifest {
                        status: slither_status,
                        manifest_path: WorkspaceRelPath::new(paths::SLITHER_BUILD_MANIFEST),
                    },
                    foundry: ToolWorkspaceManifest {
                        status: foundry_status,
                        manifest_path: WorkspaceRelPath::new(paths::FOUNDRY_BUILD_MANIFEST),
                    },
                    echidna: ToolWorkspaceManifest {
                        status: echidna_status,
                        manifest_path: WorkspaceRelPath::new(paths::ECHIDNA_BUILD_MANIFEST),
                    },
                    heimdall: ToolWorkspaceManifest {
                        status: heimdall_status,
                        manifest_path: WorkspaceRelPath::new(paths::HEIMDALL_BUILD_MANIFEST),
                    },
                },
            },
        )?;
        self.record(
            ArtifactStep::PrepareToolingWorkspaces,
            &manifest_path,
            ArtifactKind::Prep,
            status,
            "Prepared standard working directories for supported analysis tools.",
        );
        Ok(status)
    }

    fn prepare_heimdall_workspace(
        &mut self,
        address: &EvmAddress,
        chain: &ChainAlias,
        source_material_ready: bool,
    ) -> AppResult<StepStatus> {
        let targets_path = self.workspace.paths().resolve(paths::BYTECODE_TARGETS);
        let targets_exist = targets_path.exists();
        let bytecode_targets: BytecodeTargetsArtifact =
            super::support::read_json_if_exists(&targets_path)?;
        fs::create_dir_all(self.workspace.root().join(HEIMDALL_ROOT))?;

        let mut target_workspaces = Vec::new();
        let command_status = if bytecode_targets.rpc_url_configured {
            StepStatus::Prepared
        } else {
            StepStatus::ConfiguredNotExecuted
        };
        for target in &bytecode_targets.targets {
            let target_root = heimdall_target_root(&target.chain, &target.address);
            fs::create_dir_all(self.workspace.root().join(target_root.as_str()))?;
            let mut commands = Vec::new();
            for action in [
                HeimdallAction::Decompile,
                HeimdallAction::Disassemble,
                HeimdallAction::Cfg,
            ] {
                let command = heimdall_command_workspace(target, action, command_status);
                self.write_heimdall_command_files(&command)?;
                commands.push(command);
            }
            target_workspaces.push(HeimdallTargetWorkspace {
                address: target.address.clone(),
                chain: target.chain.clone(),
                role: target.role.clone(),
                name: target.name.clone(),
                source_availability: target.source_availability,
                source_unavailable_reason: target.source_unavailable_reason.clone(),
                bytecode_status: target.bytecode_status,
                bytecode_artifact: target.bytecode_artifact.clone(),
                target_root,
                commands,
                note: if bytecode_targets.rpc_url_configured {
                    None
                } else {
                    Some(
                        "AGENT_AUDIT_RPC_URL is required before running prepared Heimdall commands."
                            .to_string(),
                    )
                },
            });
        }

        let status =
            heimdall_workspace_status(&bytecode_targets, targets_exist, source_material_ready);
        let note = heimdall_workspace_note(&bytecode_targets, targets_exist, source_material_ready);
        let manifest_path = self.workspace.store().write_json(
            paths::HEIMDALL_BUILD_MANIFEST,
            &HeimdallBuildManifest {
                header: build_header(address, chain, self.workspace.run_id(), status),
                bytecode_targets_path: targets_exist
                    .then(|| WorkspaceRelPath::new(paths::BYTECODE_TARGETS)),
                heimdall_root: Some(WorkspaceRelPath::new(HEIMDALL_ROOT)),
                targets: target_workspaces,
                note,
            },
        )?;
        self.record(
            ArtifactStep::PrepareHeimdallWorkspace,
            &manifest_path,
            ArtifactKind::Prep,
            status,
            "Prepared Heimdall command workspaces for bytecode audit targets.",
        );
        Ok(status)
    }

    fn write_heimdall_command_files(
        &mut self,
        command: &HeimdallCommandWorkspace,
    ) -> AppResult<()> {
        self.workspace
            .store()
            .write_json(command.command_json_path.as_str(), command)?;
        self.workspace.store().write_text(
            command.command_text_path.as_str(),
            &heimdall_command_text(command),
        )?;
        self.workspace.store().write_text(
            command.run_script_path.as_str(),
            &render_heimdall_run_script(command),
        )?;
        set_executable(
            self.workspace
                .paths()
                .resolve(command.run_script_path.as_str()),
        )?;
        self.record(
            ArtifactStep::PrepareHeimdallWorkspace,
            &command.run_script_path,
            ArtifactKind::Prep,
            command.status,
            "Prepared a Heimdall command runner that captures stdout, stderr, exit code, and failure text.",
        );
        Ok(())
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
                path: RelativePath::new(file_name),
                target: self.workspace.paths().relative(&path)?,
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
                        link_path: self.workspace.paths().relative(&link_path)?,
                        target: self.workspace.paths().relative(&package_path)?,
                    });
                }
            } else {
                let (alias_name, version) = split_versioned_package_name(&name);
                let link_path = node_modules_root.join(&alias_name);
                recreate_symlink(&link_path, &path)?;
                links.push(NodeModuleLink {
                    alias: alias_name,
                    version,
                    link_path: self.workspace.paths().relative(&link_path)?,
                    target: self.workspace.paths().relative(&path)?,
                });
            }
        }
        Ok(links)
    }

    fn prepare_foundry_project(
        &mut self,
        address: &EvmAddress,
        chain: &ChainAlias,
        bundle_payload: &SourceBundleArtifact,
    ) -> AppResult<StepStatus> {
        let foundry_root = self.workspace.root().join("foundry_project");
        if !bundle_payload.is_fetched() {
            let (status, note, summary) = source_unavailable_tooling_precondition(
                bundle_payload,
                "Foundry",
                "Skipped Foundry project preparation because source fetching did not complete.",
            );
            return self.prepare_foundry_precondition(
                address,
                chain,
                &foundry_root,
                PreconditionSpec {
                    status,
                    note,
                    artifact_status: StepStatus::ConfiguredNotExecuted,
                    summary,
                },
            );
        }

        let sources_root = self.workspace.root().join("sources");
        if !sources_root.exists() {
            return self.prepare_foundry_precondition(
                address,
                chain,
                &foundry_root,
                PreconditionSpec {
                    status: StepStatus::SourceFilesMissing,
                    note: "Source bundle exists but sources/ is missing.",
                    artifact_status: StepStatus::ExecutedWithError,
                    summary: "Failed Foundry project preparation because source files are missing.",
                },
            );
        }

        let settings = tool_project_settings(bundle_payload);
        recreate_dir(&foundry_root)?;
        let source_links = self.link_tool_project_sources(
            &sources_root,
            &foundry_root.join("src"),
            settings.source_root.as_ref(),
        )?;
        let node_modules_links = self.create_slither_node_modules(
            &sources_root.join("npm"),
            &foundry_root.join("node_modules"),
        )?;
        let generated_remappings = node_modules_remappings(&node_modules_links);
        let remappings = merge_unique_lists(&[
            settings.remappings.as_slice(),
            generated_remappings.as_slice(),
        ]);
        let remappings_path = self.workspace.store().write_text(
            "foundry_project/remappings.txt",
            &render_line_list(&remappings),
        )?;
        self.workspace
            .store()
            .write_text("foundry_project/test/.gitkeep", "")?;
        self.workspace
            .store()
            .write_text("foundry_project/script/.gitkeep", "")?;
        self.workspace
            .store()
            .write_text("foundry_project/lib/.gitkeep", "")?;
        let foundry_toml_path = self.workspace.store().write_text(
            "foundry_project/foundry.toml",
            &render_foundry_toml(&settings, &remappings),
        )?;
        self.record(
            ArtifactStep::PrepareFoundryProject,
            &remappings_path,
            ArtifactKind::Prep,
            ArtifactStatus::Executed,
            "Prepared Foundry remappings.",
        );
        self.record(
            ArtifactStep::PrepareFoundryProject,
            &foundry_toml_path,
            ArtifactKind::Prep,
            ArtifactStatus::Executed,
            "Prepared a deterministic Foundry config.",
        );
        let manifest_path = self.workspace.store().write_json(
            paths::FOUNDRY_BUILD_MANIFEST,
            &FoundryBuildManifest {
                header: build_header(
                    address,
                    chain,
                    self.workspace.run_id(),
                    StepStatus::Prepared,
                ),
                project_root: Some(WorkspaceRelPath::new("foundry_project")),
                analysis_target: Some(analysis_target_for_prepared(bundle_payload)),
                source_links,
                node_modules_links,
                compiler_version: settings.compiler_version,
                solc_version: settings.solc_version,
                optimizer_enabled: settings.optimizer_enabled,
                optimizer_runs: settings.optimizer_runs,
                evm_version: settings.evm_version,
                remappings,
                remappings_path: Some(remappings_path),
                foundry_toml_path: Some(foundry_toml_path),
                preferred_working_dir: Some(WorkspaceRelPath::new("foundry_project")),
                preferred_target: Some(settings.prepared_target),
                preferred_source_root: settings.source_root,
                test_dir: Some(WorkspaceRelPath::new("foundry_project/test")),
                script_dir: Some(WorkspaceRelPath::new("foundry_project/script")),
                ..FoundryBuildManifest::default()
            },
        )?;
        self.record(
            ArtifactStep::PrepareFoundryProject,
            &manifest_path,
            ArtifactKind::Prep,
            ArtifactStatus::Executed,
            "Prepared a deterministic Foundry project manifest.",
        );
        Ok(StepStatus::Prepared)
    }

    fn prepare_foundry_precondition(
        &mut self,
        address: &EvmAddress,
        chain: &ChainAlias,
        foundry_root: &Path,
        spec: PreconditionSpec<'_>,
    ) -> AppResult<StepStatus> {
        recreate_dir(foundry_root)?;
        let manifest_path = self.workspace.store().write_json(
            paths::FOUNDRY_BUILD_MANIFEST,
            &FoundryBuildManifest {
                header: build_header(address, chain, self.workspace.run_id(), spec.status),
                note: Some(spec.note.to_string()),
                ..FoundryBuildManifest::default()
            },
        )?;
        self.record(
            ArtifactStep::PrepareFoundryProject,
            &manifest_path,
            ArtifactKind::Prep,
            spec.artifact_status,
            spec.summary,
        );
        Ok(spec.status)
    }

    fn prepare_echidna_project(
        &mut self,
        address: &EvmAddress,
        chain: &ChainAlias,
        bundle_payload: &SourceBundleArtifact,
    ) -> AppResult<StepStatus> {
        let echidna_root = self.workspace.root().join("echidna_project");
        if !bundle_payload.is_fetched() {
            let (status, note, summary) = source_unavailable_tooling_precondition(
                bundle_payload,
                "Echidna",
                "Skipped Echidna project preparation because source fetching did not complete.",
            );
            return self.prepare_echidna_precondition(
                address,
                chain,
                &echidna_root,
                PreconditionSpec {
                    status,
                    note,
                    artifact_status: StepStatus::ConfiguredNotExecuted,
                    summary,
                },
            );
        }

        let sources_root = self.workspace.root().join("sources");
        if !sources_root.exists() {
            return self.prepare_echidna_precondition(
                address,
                chain,
                &echidna_root,
                PreconditionSpec {
                    status: StepStatus::SourceFilesMissing,
                    note: "Source bundle exists but sources/ is missing.",
                    artifact_status: StepStatus::ExecutedWithError,
                    summary: "Failed Echidna project preparation because source files are missing.",
                },
            );
        }

        let settings = tool_project_settings(bundle_payload);
        recreate_dir(&echidna_root)?;
        let source_links = self.link_tool_project_sources(
            &sources_root,
            &echidna_root.join("src"),
            settings.source_root.as_ref(),
        )?;
        let node_modules_links = self.create_slither_node_modules(
            &sources_root.join("npm"),
            &echidna_root.join("node_modules"),
        )?;
        let generated_remappings = node_modules_remappings(&node_modules_links);
        let remappings = merge_unique_lists(&[
            settings.remappings.as_slice(),
            generated_remappings.as_slice(),
        ]);
        self.workspace
            .store()
            .write_text("echidna_project/test/.gitkeep", "")?;
        self.workspace
            .store()
            .write_text("echidna_project/lib/.gitkeep", "")?;
        let config_path = self.workspace.store().write_text(
            "echidna_project/echidna.yaml",
            &render_echidna_yaml(&settings),
        )?;
        self.record(
            ArtifactStep::PrepareEchidnaProject,
            &config_path,
            ArtifactKind::Prep,
            ArtifactStatus::Executed,
            "Prepared an Echidna config scaffold.",
        );
        let manifest_path = self.workspace.store().write_json(
            paths::ECHIDNA_BUILD_MANIFEST,
            &EchidnaBuildManifest {
                header: build_header(
                    address,
                    chain,
                    self.workspace.run_id(),
                    StepStatus::Prepared,
                ),
                project_root: Some(WorkspaceRelPath::new("echidna_project")),
                analysis_target: Some(analysis_target_for_prepared(bundle_payload)),
                source_links,
                node_modules_links,
                compiler_version: settings.compiler_version,
                solc_version: settings.solc_version,
                optimizer_enabled: settings.optimizer_enabled,
                optimizer_runs: settings.optimizer_runs,
                evm_version: settings.evm_version,
                remappings,
                config_path: Some(config_path),
                preferred_working_dir: Some(WorkspaceRelPath::new("echidna_project")),
                preferred_target: Some(settings.prepared_target),
                preferred_source_root: settings.source_root,
                harness_dir: Some(WorkspaceRelPath::new("echidna_project/test")),
                ..EchidnaBuildManifest::default()
            },
        )?;
        self.record(
            ArtifactStep::PrepareEchidnaProject,
            &manifest_path,
            ArtifactKind::Prep,
            ArtifactStatus::Executed,
            "Prepared a deterministic Echidna project manifest.",
        );
        Ok(StepStatus::Prepared)
    }

    fn prepare_echidna_precondition(
        &mut self,
        address: &EvmAddress,
        chain: &ChainAlias,
        echidna_root: &Path,
        spec: PreconditionSpec<'_>,
    ) -> AppResult<StepStatus> {
        recreate_dir(echidna_root)?;
        let manifest_path = self.workspace.store().write_json(
            paths::ECHIDNA_BUILD_MANIFEST,
            &EchidnaBuildManifest {
                header: build_header(address, chain, self.workspace.run_id(), spec.status),
                note: Some(spec.note.to_string()),
                ..EchidnaBuildManifest::default()
            },
        )?;
        self.record(
            ArtifactStep::PrepareEchidnaProject,
            &manifest_path,
            ArtifactKind::Prep,
            spec.artifact_status,
            spec.summary,
        );
        Ok(spec.status)
    }

    fn link_tool_project_sources(
        &self,
        sources_root: &Path,
        tool_src_root: &Path,
        source_root_filter: Option<&RelativePath>,
    ) -> AppResult<Vec<SourceLink>> {
        let source_root_filter = source_root_filter
            .map(RelativePath::as_str)
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
            let relative = self.workspace.paths().relative(entry.path())?;
            let source_relative = relative.as_str().trim_start_matches("sources/").to_string();
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

fn heimdall_target_root(chain: &ChainAlias, address: &EvmAddress) -> WorkspaceRelPath {
    WorkspaceRelPath::new(format!(
        "{}/{}/{}",
        HEIMDALL_ROOT,
        chain.as_str(),
        address.as_lowercase()
    ))
}

fn heimdall_command_workspace(
    target: &BytecodeAuditTarget,
    action: HeimdallAction,
    status: StepStatus,
) -> HeimdallCommandWorkspace {
    let action_root = WorkspaceRelPath::new(format!(
        "{}/{}/{}/{}",
        HEIMDALL_ROOT,
        target.chain.as_str(),
        target.address.as_lowercase(),
        action.as_str()
    ));
    let output_dir = action_root.join("output");
    let command = heimdall_command_vector(target, action, &output_dir);
    HeimdallCommandWorkspace {
        action,
        status,
        run_script_path: action_root.join("run.sh"),
        command_json_path: action_root.join("command.json"),
        command_text_path: action_root.join("command.txt"),
        stdout_path: action_root.join("stdout.txt"),
        stderr_path: action_root.join("stderr.txt"),
        exit_code_path: action_root.join("exit_code.txt"),
        failure_path: action_root.join("failure.txt"),
        output_dir,
        command,
        note: if status == StepStatus::Prepared {
            None
        } else {
            Some("AGENT_AUDIT_RPC_URL must be configured before this command can run.".to_string())
        },
    }
}

fn heimdall_command_vector(
    target: &BytecodeAuditTarget,
    action: HeimdallAction,
    output_dir: &WorkspaceRelPath,
) -> Vec<String> {
    let mut command = vec![
        "heimdall".to_string(),
        action.as_str().to_string(),
        target.address.to_string(),
        "--rpc-url".to_string(),
        "$AGENT_AUDIT_RPC_URL".to_string(),
    ];
    if action == HeimdallAction::Decompile {
        command.extend([
            "--default".to_string(),
            "--include-sol".to_string(),
            "--include-yul".to_string(),
        ]);
    }
    command.extend(["--output".to_string(), output_dir.to_string()]);
    if action == HeimdallAction::Decompile {
        command.extend([
            "--name".to_string(),
            heimdall_target_name(&target.role, &target.name),
        ]);
    }
    command
}

fn heimdall_target_name(role: &str, name: &str) -> String {
    if !role.trim().is_empty() {
        role.to_string()
    } else if !name.trim().is_empty() {
        name.to_string()
    } else {
        "target".to_string()
    }
}

fn heimdall_workspace_status(
    targets: &BytecodeTargetsArtifact,
    targets_exist: bool,
    source_material_ready: bool,
) -> StepStatus {
    if !targets_exist {
        return if source_material_ready {
            StepStatus::Prepared
        } else {
            StepStatus::SourceNotFetched
        };
    }
    if targets.targets.is_empty() {
        return StepStatus::Prepared;
    }
    if targets.rpc_url_configured {
        StepStatus::Prepared
    } else {
        StepStatus::ConfiguredNotExecuted
    }
}

fn heimdall_workspace_note(
    targets: &BytecodeTargetsArtifact,
    targets_exist: bool,
    source_material_ready: bool,
) -> Option<String> {
    if !targets_exist {
        return Some(if source_material_ready {
            "No bytecode target artifact was found; no Heimdall workspace is required for this run."
                .to_string()
        } else {
            "Fetch source before preparing Heimdall workspaces; artifacts/bytecode_targets.json is missing."
                .to_string()
        });
    }
    if targets.targets.is_empty() {
        return Some("No source-unavailable bytecode audit targets were identified.".to_string());
    }
    if !targets.rpc_url_configured {
        return Some(
            "AGENT_AUDIT_RPC_URL is not configured; Heimdall command workspaces were written but cannot run yet."
                .to_string(),
        );
    }
    None
}

fn render_heimdall_run_script(command: &HeimdallCommandWorkspace) -> String {
    let command_line = heimdall_shell_command(command);
    let command_text = heimdall_command_text(command);
    format!(
        r#"#!/usr/bin/env bash
set -u

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
RUN_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/../../../../.." && pwd)
cd "$RUN_ROOT"

mkdir -p {output_dir}
: > {stdout_path}
: > {stderr_path}
rm -f {failure_path}

if [ -z "${{AGENT_AUDIT_RPC_URL:-}}" ]; then
  printf '%s\n' 'AGENT_AUDIT_RPC_URL is not configured.' > {stderr_path}
  printf '%s\n' '2' > {exit_code_path}
  printf '%s\n' 'AGENT_AUDIT_RPC_URL is not configured.' > {failure_path}
  exit 2
fi

printf '%s\n' {command_text} > {command_text_path}
{command_line} > {stdout_path} 2> {stderr_path}
status=$?
printf '%s\n' "$status" > {exit_code_path}
if [ "$status" -ne 0 ]; then
  printf 'heimdall {action} failed with exit code %s\n' "$status" > {failure_path}
else
  rm -f {failure_path}
fi
exit "$status"
"#,
        output_dir = shell_quote(command.output_dir.as_str()),
        stdout_path = shell_quote(command.stdout_path.as_str()),
        stderr_path = shell_quote(command.stderr_path.as_str()),
        exit_code_path = shell_quote(command.exit_code_path.as_str()),
        failure_path = shell_quote(command.failure_path.as_str()),
        command_text_path = shell_quote(command.command_text_path.as_str()),
        command_text = shell_quote(&command_text),
        command_line = command_line,
        action = command.action.as_str(),
    )
}

fn heimdall_shell_command(command: &HeimdallCommandWorkspace) -> String {
    let address = command
        .command
        .get(2)
        .map(String::as_str)
        .unwrap_or_default();
    let mut line = format!(
        "heimdall {} {} --rpc-url \"$AGENT_AUDIT_RPC_URL\"",
        command.action.as_str(),
        shell_quote(address)
    );
    if command.action == HeimdallAction::Decompile {
        line.push_str(" --default --include-sol --include-yul");
    }
    line.push_str(&format!(
        " --output {}",
        shell_quote(command.output_dir.as_str())
    ));
    if command.action == HeimdallAction::Decompile
        && let Some(name) = command.command.last()
    {
        line.push_str(&format!(" --name {}", shell_quote(name)));
    }
    line
}

fn heimdall_command_text(command: &HeimdallCommandWorkspace) -> String {
    command
        .command
        .iter()
        .map(|arg| {
            if arg == "$AGENT_AUDIT_RPC_URL" {
                "\"$AGENT_AUDIT_RPC_URL\"".to_string()
            } else {
                shell_quote(arg)
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn set_executable(path: PathBuf) -> AppResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&path)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions)?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

#[derive(Clone)]
struct SlitherSettings {
    target_path: RelativePath,
    source_root: Option<RelativePath>,
    prepared_root: RelativePath,
    prepared_target: RelativePath,
    working_dir: WorkspaceRelPath,
    working_dir_token: RelativePath,
    compiler_version: String,
    solc_version: String,
    solc_select: SolcSelectStatus,
    include_paths: Vec<RelativePath>,
    remappings: Vec<String>,
    remappings_file: RelativePath,
    solc_args: String,
}

#[derive(Clone)]
struct ToolProjectSettings {
    source_root: Option<RelativePath>,
    prepared_target: RelativePath,
    compiler_version: String,
    solc_version: String,
    optimizer_enabled: bool,
    optimizer_runs: u64,
    evm_version: String,
    remappings: Vec<String>,
}

struct PreconditionSpec<'a> {
    status: StepStatus,
    note: &'a str,
    artifact_status: StepStatus,
    summary: &'a str,
}

fn source_unavailable_tooling_precondition<'a>(
    bundle_payload: &SourceBundleArtifact,
    tool_name: &'a str,
    default_summary: &'a str,
) -> (StepStatus, &'a str, &'a str) {
    if bundle_payload.is_source_unavailable() {
        (
            StepStatus::SourceUnavailable,
            "Verified source is unavailable; Solidity project preparation is skipped. Use artifacts/bytecode_targets.json for bytecode review.",
            match tool_name {
                "Slither" => {
                    "Skipped Slither project preparation because verified source is unavailable."
                }
                "Foundry" => {
                    "Skipped Foundry project preparation because verified source is unavailable."
                }
                "Echidna" => {
                    "Skipped Echidna project preparation because verified source is unavailable."
                }
                _ => "Skipped Solidity project preparation because verified source is unavailable.",
            },
        )
    } else {
        (
            StepStatus::SourceNotFetched,
            match tool_name {
                "Slither" => "Fetch verified source before preparing a Slither project.",
                "Foundry" => "Fetch verified source before preparing a Foundry project.",
                "Echidna" => "Fetch verified source before preparing an Echidna project.",
                _ => "Fetch verified source before preparing a Solidity project.",
            },
            default_summary,
        )
    }
}

fn build_header(
    address: &EvmAddress,
    chain: &ChainAlias,
    run_id: &RunId,
    status: StepStatus,
) -> RunArtifactHeader {
    RunArtifactHeader {
        target: RunTarget::new(address.clone(), chain.clone()),
        run_id: run_id.clone(),
        status,
    }
}

fn bundle_source_step_status(bundle_payload: &SourceBundleArtifact) -> StepStatus {
    if bundle_payload.is_fetched() {
        StepStatus::SourceFetched
    } else {
        bundle_payload.status
    }
}

fn aggregate_tooling_status(
    source_status: StepStatus,
    slither_status: StepStatus,
    foundry_status: StepStatus,
    echidna_status: StepStatus,
    heimdall_status: StepStatus,
) -> StepStatus {
    if source_status != StepStatus::SourceFetched {
        return source_status;
    }
    for status in [
        slither_status,
        foundry_status,
        echidna_status,
        heimdall_status,
    ] {
        if status != StepStatus::Prepared {
            return status;
        }
    }
    StepStatus::Prepared
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
    let target_path = analysis_target.path;
    let source_root = path_parent(&target_path);
    let prepared_target = if let Some(source_root) = source_root.as_ref() {
        let prefix = format!("{}/", source_root.as_str());
        RelativePath::new(
            target_path
                .as_str()
                .strip_prefix(&prefix)
                .unwrap_or(target_path.as_str()),
        )
    } else {
        target_path.clone()
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
    target_path: &RelativePath,
) -> SlitherSettings {
    let normalized_target_path = target_path.clone();
    let source_root = slither_source_root_for_target(&normalized_target_path, linked_entries);
    let compiler_version = compiler_version_for_path(bundle_payload, &normalized_target_path);
    let solc_version = extract_semver(&compiler_version);
    let source_meta = source_meta_for_path(bundle_payload, &normalized_target_path);
    let provider_remappings = provider_remappings(source_meta);
    let generated_remappings = node_modules_remappings(node_modules_links);
    let remappings = merge_unique_lists(&[
        provider_remappings.as_slice(),
        generated_remappings.as_slice(),
    ]);
    let use_project_root = !remappings.is_empty();
    let working_root = if use_project_root {
        None
    } else {
        source_root.clone()
    };
    let prepared_root = if use_project_root || source_root.is_none() {
        RelativePath::dot()
    } else {
        source_root.clone().unwrap_or_default()
    };
    let prepared_target = if use_project_root {
        normalized_target_path.clone()
    } else {
        slither_relative_target_path(&normalized_target_path, source_root.as_ref())
    };
    let include_paths =
        slither_include_paths(working_root.as_ref(), !node_modules_links.is_empty());
    let working_dir = if let Some(working_root) = working_root.as_ref() {
        WorkspaceRelPath::new(format!("slither_project/{working_root}"))
    } else {
        WorkspaceRelPath::new("slither_project")
    };
    let remappings_file =
        slither_relative_from_working_dir(working_root.as_ref(), "remappings.txt");
    let solc_args = slither_solc_args(&include_paths);
    let solc_select = solc_select_status(workspace, &solc_version);
    let working_dir_token = working_root.unwrap_or_default();
    SlitherSettings {
        target_path: normalized_target_path,
        source_root,
        prepared_root,
        prepared_target,
        working_dir,
        working_dir_token,
        compiler_version,
        solc_version,
        solc_select,
        include_paths,
        remappings_file,
        remappings,
        solc_args,
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
    lines.push(format!("prefix: \"{}\"", settings.prepared_target));
    lines.push(String::new());
    lines.join("\n")
}

fn slither_source_root_for_target(
    target_path: &RelativePath,
    linked_entries: &[SourceLink],
) -> Option<RelativePath> {
    linked_entries
        .iter()
        .map(|entry| entry.path.clone())
        .filter(|source_root| {
            !source_root.is_dot()
                && (target_path == source_root
                    || target_path
                        .as_str()
                        .starts_with(&format!("{}/", source_root.as_str())))
        })
        .max_by_key(|item| item.as_str().len())
}

fn slither_relative_target_path(
    target_path: &RelativePath,
    source_root: Option<&RelativePath>,
) -> RelativePath {
    let Some(source_root) = source_root else {
        return target_path.clone();
    };
    if target_path == source_root {
        return RelativePath::dot();
    }
    let prefix = format!("{}/", source_root.as_str());
    if let Some(stripped) = target_path.as_str().strip_prefix(&prefix) {
        RelativePath::new(stripped)
    } else {
        target_path.clone()
    }
}

fn slither_relative_from_working_dir(
    source_root: Option<&RelativePath>,
    path_in_slither_root: &str,
) -> RelativePath {
    if let Some(source_root) = source_root {
        RelativePath::new(
            pathdiff::diff_paths(path_in_slither_root, source_root.as_str())
                .unwrap_or_else(|| PathBuf::from(path_in_slither_root))
                .to_string_lossy(),
        )
    } else {
        RelativePath::new(path_in_slither_root)
    }
}

fn slither_include_paths(
    source_root: Option<&RelativePath>,
    has_node_modules: bool,
) -> Vec<RelativePath> {
    let mut include_paths = vec![RelativePath::dot()];
    if has_node_modules {
        let node_modules_path = slither_relative_from_working_dir(source_root, "node_modules");
        if !include_paths.contains(&node_modules_path) {
            include_paths.push(node_modules_path);
        }
    }
    include_paths
}

fn slither_solc_args(include_paths: &[RelativePath]) -> String {
    let mut args = vec!["--base-path".to_string(), ".".to_string()];
    let mut allow_paths = vec![".".to_string()];
    for entry in include_paths {
        if entry.is_dot() {
            continue;
        }
        args.push("--include-path".to_string());
        args.push(entry.as_str().to_string());
        allow_paths.push(entry.as_str().to_string());
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
            command_status: ToolCommandStatus::Error,
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
                command_status: ToolCommandStatus::Error,
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
            ToolCommandStatus::Ok
        } else {
            ToolCommandStatus::Error
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::bytecode::{BytecodeAuditTarget, BytecodeFetchStatus};
    use crate::models::identity::RunId;
    use crate::models::run::RunTarget;
    use crate::models::source::SourceAvailabilityStatus;
    use crate::workspace::RunWorkspace;
    use tempfile::TempDir;

    fn test_workspace() -> (TempDir, RunWorkspace, RunTarget) {
        let temp = TempDir::new().expect("temp dir");
        std::fs::write(
            temp.path().join(".env"),
            "AGENT_AUDIT_DEFAULT_CHAIN=eth\nAGENT_AUDIT_RUNS_DIR=runs\n",
        )
        .expect("write env");
        let target = RunTarget::new(
            EvmAddress::new("0x1234567890abcdef1234567890abcdef12345678").expect("address"),
            ChainAlias::new("eth").expect("chain"),
        );
        let workspace = RunWorkspace::create_at_root(
            temp.path(),
            &temp.path().join("runs/run-1"),
            &RunId::new("run-1").expect("run id"),
            &target.address,
            &target.chain,
        )
        .expect("workspace");
        (temp, workspace, target)
    }

    #[test]
    fn aggregate_tooling_status_returns_first_tooling_failure() {
        let status = aggregate_tooling_status(
            StepStatus::SourceFetched,
            StepStatus::Prepared,
            StepStatus::SourceFilesMissing,
            StepStatus::Prepared,
            StepStatus::Prepared,
        );

        assert_eq!(status, StepStatus::SourceFilesMissing);
    }

    #[test]
    fn aggregate_tooling_status_preserves_source_failure() {
        let status = aggregate_tooling_status(
            StepStatus::SourceApiNotConfigured,
            StepStatus::Prepared,
            StepStatus::Prepared,
            StepStatus::Prepared,
            StepStatus::Prepared,
        );

        assert_eq!(status, StepStatus::SourceApiNotConfigured);
    }

    #[test]
    fn prepare_tooling_creates_managed_heimdall_workspace() {
        let (_temp, workspace, target) = test_workspace();
        workspace
            .store()
            .write_json(
                paths::SOURCE_BUNDLE,
                &SourceBundleArtifact {
                    target: target.clone(),
                    status: StepStatus::SourceUnavailable,
                    source_availability: SourceAvailabilityStatus::Unavailable,
                    source_unavailable_reason: Some("not verified".into()),
                    ..SourceBundleArtifact::default()
                },
            )
            .expect("source bundle");
        workspace
            .store()
            .write_json(
                paths::BYTECODE_TARGETS,
                &BytecodeTargetsArtifact::new(
                    target.clone(),
                    true,
                    vec![BytecodeAuditTarget {
                        address: target.address.clone(),
                        chain: target.chain.clone(),
                        role: "target".into(),
                        name: "target".into(),
                        source_availability: SourceAvailabilityStatus::Unavailable,
                        source_unavailable_reason: Some("not verified".into()),
                        origin: "source_provider_unavailable".into(),
                        bytecode_status: BytecodeFetchStatus::Fetched,
                        bytecode_artifact: Some(WorkspaceRelPath::new(
                            "artifacts/bytecode/eth/0x1234567890abcdef1234567890abcdef12345678.hex",
                        )),
                        ..BytecodeAuditTarget::default()
                    }],
                ),
            )
            .expect("bytecode targets");
        let config = crate::config::AppConfig {
            project_root: workspace.project_root.clone(),
            runs_dir: workspace.project_root.join("runs"),
            default_chain: target.chain.clone(),
            source_api_base: None,
            source_api_key: None,
            source_api_headers: std::collections::BTreeMap::new(),
            rpc_url: None,
            mongo_uri: None,
            mongo_db: "agent_audit".to_string(),
            mongo_runs_meta_collection: "runs_meta".to_string(),
            mongo_runs_files_collection: "runs_files".to_string(),
            mongo_max_inline_file_bytes: 8 * 1024 * 1024,
        };
        let mut service = AuditPipelineService::new(config, workspace);

        let status = service
            .prepare_tooling_workspaces(&target.address, &target.chain)
            .expect("prepare tooling");

        assert_eq!(status, StepStatus::SourceUnavailable);
        let manifest: HeimdallBuildManifest = super::super::support::read_json_if_exists(
            &service
                .workspace
                .paths()
                .resolve(paths::HEIMDALL_BUILD_MANIFEST),
        )
        .expect("heimdall manifest");
        assert_eq!(manifest.header.status, StepStatus::Prepared);
        assert_eq!(manifest.targets.len(), 1);
        let target_manifest = &manifest.targets[0];
        assert_eq!(target_manifest.commands.len(), 3);
        let decompile = target_manifest
            .commands
            .iter()
            .find(|command| command.action == HeimdallAction::Decompile)
            .expect("decompile command");
        assert_eq!(decompile.status, StepStatus::Prepared);

        let run_script_path = service
            .workspace
            .paths()
            .resolve(decompile.run_script_path.as_str());
        let command_json_path = service
            .workspace
            .paths()
            .resolve(decompile.command_json_path.as_str());
        assert!(run_script_path.exists());
        assert!(command_json_path.exists());
        let script = std::fs::read_to_string(run_script_path).expect("read run script");
        assert!(script.contains("stdout.txt"));
        assert!(script.contains("exit_code.txt"));
        assert!(script.contains("AGENT_AUDIT_RPC_URL"));

        let tooling_manifest: ToolingManifest = super::super::support::read_json_if_exists(
            &service.workspace.paths().resolve(paths::TOOLING_MANIFEST),
        )
        .expect("tooling manifest");
        assert_eq!(
            tooling_manifest.workspaces.heimdall.status,
            StepStatus::Prepared
        );
        assert_eq!(
            tooling_manifest.workspaces.heimdall.manifest_path.as_str(),
            paths::HEIMDALL_BUILD_MANIFEST
        );
    }
}
