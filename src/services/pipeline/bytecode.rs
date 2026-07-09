use std::collections::BTreeMap;
use std::process::Command;
use std::time::Duration;

use reqwest::blocking::Client;
use serde_json::{Value, json};
use sha3::{Digest, Keccak256};
use url::Url;

use crate::error::AppResult;
use crate::models::artifact::{ArtifactKind, ArtifactStatus, ArtifactStep};
use crate::models::bytecode::{
    BytecodeArtifact, HeimdallCommandRecord, HeimdallManifest, SelectorEntry,
    SelectorIndexArtifact, StorageProbe, StorageProbePlanArtifact,
};
use crate::models::identity::{ChainAlias, EvmAddress};
use crate::models::path::WorkspaceRelPath;
use crate::models::run::{RunTarget, SourceKind};
use crate::models::step::StepStatus;
use crate::models::tooling::RunArtifactHeader;
use crate::workspace::paths;

use super::AuditPipelineService;
use super::support::read_json_if_exists;

const BYTECODE_BLOCK_TAG: &str = "latest";
const PREVIEW_CHARS: usize = 600;

impl AuditPipelineService {
    pub fn fetch_contract_bytecode(
        &mut self,
        address: &EvmAddress,
        chain: &ChainAlias,
        source_kind: SourceKind,
    ) -> AppResult<StepStatus> {
        if source_kind == SourceKind::ClosedSource {
            self.write_closed_source_bundle(address, chain)?;
        }
        let target = target_for_kind(address, chain, source_kind);

        let Some(rpc_url) = self.config.rpc_url.clone() else {
            let path = self.workspace.store().write_json(
                paths::BYTECODE,
                &BytecodeArtifact {
                    header: self.header(target, StepStatus::ConfiguredNotExecuted),
                    rpc_url_configured: false,
                    block_tag: BYTECODE_BLOCK_TAG.to_string(),
                    note: Some(
                        "Configure AGENT_AUDIT_RPC_URL to fetch deployed runtime bytecode."
                            .to_string(),
                    ),
                    ..BytecodeArtifact::default()
                },
            )?;
            self.record(
                ArtifactStep::FetchContractBytecode,
                &path,
                ArtifactKind::Artifact,
                StepStatus::ConfiguredNotExecuted,
                "Skipped bytecode fetch because RPC is not configured.",
            );
            return Ok(StepStatus::ConfiguredNotExecuted);
        };

        let result = fetch_runtime_bytecode(&rpc_url, address);
        match result {
            Ok(runtime_bytecode) => {
                self.write_successful_bytecode_artifacts(target, runtime_bytecode, true)
            }
            Err(error) => {
                let path = self.workspace.store().write_json(
                    paths::BYTECODE,
                    &BytecodeArtifact {
                        header: self.header(target, StepStatus::BytecodeFetchFailed),
                        rpc_url_configured: true,
                        block_tag: BYTECODE_BLOCK_TAG.to_string(),
                        error: Some(error),
                        ..BytecodeArtifact::default()
                    },
                )?;
                self.record(
                    ArtifactStep::FetchContractBytecode,
                    &path,
                    ArtifactKind::Artifact,
                    StepStatus::BytecodeFetchFailed,
                    "Bytecode fetch failed; inspect the stored error payload.",
                );
                Ok(StepStatus::BytecodeFetchFailed)
            }
        }
    }

    pub fn prepare_heimdall_artifacts(
        &mut self,
        address: &EvmAddress,
        chain: &ChainAlias,
        source_kind: SourceKind,
    ) -> AppResult<StepStatus> {
        let bytecode: BytecodeArtifact =
            read_json_if_exists(&self.workspace.paths().resolve(paths::BYTECODE))?;
        if bytecode.header.status != StepStatus::BytecodeFetched {
            return self.write_skipped_heimdall_manifest(
                address,
                chain,
                source_kind,
                "Fetch runtime bytecode before preparing Heimdall artifacts.",
                None,
            );
        }

        let Some(input_bytecode_file) = bytecode.runtime_bytecode_file.clone() else {
            return self.write_skipped_heimdall_manifest(
                address,
                chain,
                source_kind,
                "Bytecode artifact is missing runtime_bytecode_file.",
                None,
            );
        };

        let version = match heimdall_version() {
            Ok(version) => Some(version),
            Err(error) => {
                return self.write_skipped_heimdall_manifest(
                    address,
                    chain,
                    source_kind,
                    "Heimdall is not available in PATH.",
                    Some(error),
                );
            }
        };

        let input_path = self
            .workspace
            .paths()
            .resolve(input_bytecode_file.as_str())
            .to_string_lossy()
            .to_string();
        let command_specs = [
            HeimdallCommandSpec {
                kind: "decompile",
                args: vec![
                    "decompile".to_string(),
                    input_path.clone(),
                    "--output".to_string(),
                    "print".to_string(),
                    "--include-sol".to_string(),
                    "--skip-resolving".to_string(),
                    "--default".to_string(),
                ],
                stdout_path: paths::HEIMDALL_DECOMPILED,
                stderr_path: "artifacts/heimdall_decompile_stderr.txt",
            },
            HeimdallCommandSpec {
                kind: "disassemble",
                args: vec![
                    "disassemble".to_string(),
                    input_path.clone(),
                    "--output".to_string(),
                    "print".to_string(),
                    "--default".to_string(),
                ],
                stdout_path: paths::HEIMDALL_DISASSEMBLY,
                stderr_path: "artifacts/heimdall_disassemble_stderr.txt",
            },
            HeimdallCommandSpec {
                kind: "cfg",
                args: vec![
                    "cfg".to_string(),
                    input_path,
                    "--output".to_string(),
                    "print".to_string(),
                    "--default".to_string(),
                ],
                stdout_path: paths::HEIMDALL_CFG,
                stderr_path: "artifacts/heimdall_cfg_stderr.txt",
            },
        ];

        let mut commands = Vec::new();
        let mut output_artifacts = Vec::new();
        let mut all_ok = true;
        for spec in command_specs {
            let record = self.run_heimdall_command(spec)?;
            if record.exit_status != Some(0) {
                all_ok = false;
            }
            if let Some(path) = record.stdout_path.clone() {
                output_artifacts.push(path);
            }
            commands.push(record);
        }

        let status = if all_ok {
            StepStatus::HeimdallPrepared
        } else {
            StepStatus::HeimdallFailed
        };
        let manifest_path = self.workspace.store().write_json(
            paths::HEIMDALL_MANIFEST,
            &HeimdallManifest {
                header: self.header(target_for_kind(address, chain, source_kind), status),
                version,
                input_bytecode_file: Some(input_bytecode_file),
                input_bytecode_artifact: Some(WorkspaceRelPath::new(paths::BYTECODE)),
                commands,
                output_artifacts: output_artifacts.clone(),
                note: Some(
                    "Heimdall output is review material derived from bytecode, not verified Solidity source."
                        .to_string(),
                ),
                ..HeimdallManifest::default()
            },
        )?;
        self.record(
            ArtifactStep::PrepareHeimdallArtifacts,
            &manifest_path,
            ArtifactKind::Artifact,
            status,
            "Prepared Heimdall bytecode analysis manifest.",
        );
        Ok(status)
    }

    fn write_successful_bytecode_artifacts(
        &mut self,
        target: RunTarget,
        runtime_bytecode: String,
        rpc_url_configured: bool,
    ) -> AppResult<StepStatus> {
        let bytecode_bytes = decode_hex_bytes(&runtime_bytecode).unwrap_or_default();
        if bytecode_bytes.is_empty() {
            let path = self.workspace.store().write_json(
                paths::BYTECODE,
                &BytecodeArtifact {
                    header: self.header(target, StepStatus::BytecodeFetchFailed),
                    rpc_url_configured,
                    block_tag: BYTECODE_BLOCK_TAG.to_string(),
                    runtime_bytecode: Some(runtime_bytecode),
                    byte_length: Some(0),
                    error: Some("address has no deployed runtime bytecode at latest block".into()),
                    ..BytecodeArtifact::default()
                },
            )?;
            self.record(
                ArtifactStep::FetchContractBytecode,
                &path,
                ArtifactKind::Artifact,
                StepStatus::BytecodeFetchFailed,
                "Fetched an empty runtime bytecode payload.",
            );
            return Ok(StepStatus::BytecodeFetchFailed);
        }

        let bytecode_file = self
            .workspace
            .store()
            .write_text(paths::RUNTIME_BYTECODE, &format!("{runtime_bytecode}\n"))?;
        self.record(
            ArtifactStep::FetchContractBytecode,
            &bytecode_file,
            ArtifactKind::Artifact,
            ArtifactStatus::Executed,
            "Stored deployed runtime bytecode as a hex file.",
        );

        let bytecode_path = self.workspace.store().write_json(
            paths::BYTECODE,
            &BytecodeArtifact {
                header: self.header(target.clone(), StepStatus::BytecodeFetched),
                rpc_url_configured,
                block_tag: BYTECODE_BLOCK_TAG.to_string(),
                runtime_bytecode: Some(runtime_bytecode.clone()),
                runtime_bytecode_file: Some(bytecode_file.clone()),
                code_hash: Some(format!(
                    "0x{}",
                    hex_lower(&Keccak256::digest(&bytecode_bytes))
                )),
                byte_length: Some(bytecode_bytes.len()),
                ..BytecodeArtifact::default()
            },
        )?;
        self.record(
            ArtifactStep::FetchContractBytecode,
            &bytecode_path,
            ArtifactKind::Artifact,
            StepStatus::BytecodeFetched,
            "Fetched deployed runtime bytecode.",
        );
        self.write_selector_index(&target, &bytecode_bytes, &bytecode_path)?;
        self.write_storage_probe_plan(&target)?;
        Ok(StepStatus::BytecodeFetched)
    }

    fn write_selector_index(
        &mut self,
        target: &RunTarget,
        bytecode: &[u8],
        source_artifact: &WorkspaceRelPath,
    ) -> AppResult<()> {
        let mut selectors = BTreeMap::<String, Vec<usize>>::new();
        for offset in 0..bytecode.len().saturating_sub(4) {
            if bytecode[offset] != 0x63 {
                continue;
            }
            let selector = format!("0x{}", hex_lower(&bytecode[offset + 1..offset + 5]));
            selectors.entry(selector).or_default().push(offset);
        }
        let entries = selectors
            .into_iter()
            .map(|(selector, offsets)| SelectorEntry {
                selector,
                offsets,
                signature_hints: Vec::new(),
            })
            .collect::<Vec<_>>();
        let path = self.workspace.store().write_json(
            paths::SELECTOR_INDEX,
            &SelectorIndexArtifact {
                header: self.header(target.clone(), StepStatus::Executed),
                selectors: entries,
                source_artifact: Some(source_artifact.clone()),
                note: Some(
                    "Selectors are extracted from PUSH4 bytecode patterns and may include non-dispatcher constants."
                        .to_string(),
                ),
            },
        )?;
        self.record(
            ArtifactStep::FetchContractBytecode,
            &path,
            ArtifactKind::Artifact,
            ArtifactStatus::Executed,
            "Extracted raw PUSH4 selector candidates from runtime bytecode.",
        );
        Ok(())
    }

    fn write_storage_probe_plan(&mut self, target: &RunTarget) -> AppResult<()> {
        let path = self.workspace.store().write_json(
            paths::STORAGE_PROBE_PLAN,
            &StorageProbePlanArtifact {
                header: self.header(target.clone(), StepStatus::Executed),
                probes: storage_probes(),
                note: "Suggested storage probes only; these slots are not conclusions until read from chain or a fork."
                    .to_string(),
            },
        )?;
        self.record(
            ArtifactStep::FetchContractBytecode,
            &path,
            ArtifactKind::Artifact,
            ArtifactStatus::Executed,
            "Stored suggested storage probes for closed-source review.",
        );
        Ok(())
    }

    fn write_skipped_heimdall_manifest(
        &mut self,
        address: &EvmAddress,
        chain: &ChainAlias,
        source_kind: SourceKind,
        note: &str,
        error: Option<String>,
    ) -> AppResult<StepStatus> {
        let path = self.workspace.store().write_json(
            paths::HEIMDALL_MANIFEST,
            &HeimdallManifest {
                header: self.header(
                    target_for_kind(address, chain, source_kind),
                    StepStatus::ConfiguredNotExecuted,
                ),
                input_bytecode_artifact: Some(WorkspaceRelPath::new(paths::BYTECODE)),
                note: Some(note.to_string()),
                error,
                ..HeimdallManifest::default()
            },
        )?;
        self.record(
            ArtifactStep::PrepareHeimdallArtifacts,
            &path,
            ArtifactKind::Artifact,
            StepStatus::ConfiguredNotExecuted,
            "Skipped Heimdall preparation because its prerequisites are not available.",
        );
        Ok(StepStatus::ConfiguredNotExecuted)
    }

    fn run_heimdall_command(
        &mut self,
        spec: HeimdallCommandSpec,
    ) -> AppResult<HeimdallCommandRecord> {
        let output = Command::new("heimdall").args(&spec.args).output();
        let (exit_status, stdout, stderr) = match output {
            Ok(output) => (
                output.status.code(),
                String::from_utf8_lossy(&output.stdout).to_string(),
                String::from_utf8_lossy(&output.stderr).to_string(),
            ),
            Err(error) => (None, String::new(), error.to_string()),
        };

        let stdout_path = self
            .workspace
            .store()
            .write_text(spec.stdout_path, &stdout)?;
        let stderr_path = self
            .workspace
            .store()
            .write_text(spec.stderr_path, &stderr)?;
        self.record(
            ArtifactStep::PrepareHeimdallArtifacts,
            &stdout_path,
            ArtifactKind::Artifact,
            if exit_status == Some(0) {
                ArtifactStatus::Executed
            } else {
                StepStatus::ExecutedWithError
            },
            "Stored Heimdall stdout output.",
        );
        self.record(
            ArtifactStep::PrepareHeimdallArtifacts,
            &stderr_path,
            ArtifactKind::Artifact,
            if stderr.trim().is_empty() {
                ArtifactStatus::Executed
            } else {
                StepStatus::ExecutedWithError
            },
            "Stored Heimdall stderr output.",
        );

        Ok(HeimdallCommandRecord {
            kind: spec.kind.to_string(),
            command: std::iter::once("heimdall".to_string())
                .chain(spec.args)
                .collect(),
            exit_status,
            stdout_path: Some(stdout_path),
            stderr_path: Some(stderr_path),
            stdout_preview: non_empty_preview(&stdout),
            stderr_preview: non_empty_preview(&stderr),
        })
    }

    fn header(&self, target: RunTarget, status: StepStatus) -> RunArtifactHeader {
        RunArtifactHeader {
            target,
            run_id: self.workspace.run_id().clone(),
            status,
        }
    }
}

struct HeimdallCommandSpec {
    kind: &'static str,
    args: Vec<String>,
    stdout_path: &'static str,
    stderr_path: &'static str,
}

fn fetch_runtime_bytecode(rpc_url: &Url, address: &EvmAddress) -> Result<String, String> {
    let client = Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|error| error.to_string())?;
    let response = client
        .post(rpc_url.clone())
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1u64,
            "method": "eth_getCode",
            "params": [address.as_str(), BYTECODE_BLOCK_TAG],
        }))
        .send()
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "eth_getCode HTTP request failed with status {}",
            response.status()
        ));
    }
    let payload: Value = response.json().map_err(|error| error.to_string())?;
    if let Some(error) = payload.get("error") {
        return Err(error.to_string());
    }
    let bytecode = payload
        .get("result")
        .and_then(Value::as_str)
        .ok_or_else(|| "eth_getCode returned a non-string result".to_string())?
        .to_string();
    validate_hex_bytecode(&bytecode)?;
    Ok(bytecode)
}

fn heimdall_version() -> Result<String, String> {
    let output = Command::new("heimdall")
        .arg("--version")
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn validate_hex_bytecode(value: &str) -> Result<(), String> {
    let Some(body) = value.strip_prefix("0x") else {
        return Err("bytecode result is missing 0x prefix".to_string());
    };
    if body.len() % 2 != 0 {
        return Err("bytecode result has an odd hex length".to_string());
    }
    if !body.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err("bytecode result contains non-hex characters".to_string());
    }
    Ok(())
}

fn decode_hex_bytes(value: &str) -> Option<Vec<u8>> {
    validate_hex_bytecode(value).ok()?;
    let body = value.strip_prefix("0x")?;
    let mut out = Vec::with_capacity(body.len() / 2);
    for index in (0..body.len()).step_by(2) {
        out.push(u8::from_str_radix(&body[index..index + 2], 16).ok()?);
    }
    Some(out)
}

fn storage_probes() -> Vec<StorageProbe> {
    vec![
        StorageProbe {
            name: "eip1967_implementation".to_string(),
            slot: "0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc".to_string(),
            purpose: "Proxy implementation slot".to_string(),
        },
        StorageProbe {
            name: "eip1967_admin".to_string(),
            slot: "0xb53127684a568b3173ae13b9f8a6016e243e63b6e8ee1178d6a717850b5d6103".to_string(),
            purpose: "Proxy admin slot".to_string(),
        },
        StorageProbe {
            name: "eip1967_beacon".to_string(),
            slot: "0xa3f0ad74e5423aebfd80d3ef4346578335a9a72aeaee59ff6cb3582b35133d50".to_string(),
            purpose: "Proxy beacon slot".to_string(),
        },
        StorageProbe {
            name: "slot_0".to_string(),
            slot: "0x0".to_string(),
            purpose: "Common owner/admin/initializer packed-storage candidate".to_string(),
        },
        StorageProbe {
            name: "slot_1".to_string(),
            slot: "0x1".to_string(),
            purpose: "Common owner/admin/paused/fee storage candidate".to_string(),
        },
    ]
}

fn target_for_kind(address: &EvmAddress, chain: &ChainAlias, source_kind: SourceKind) -> RunTarget {
    RunTarget::new_with_source_kind(address.clone(), chain.clone(), source_kind)
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn non_empty_preview(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.chars().take(PREVIEW_CHARS).collect())
    }
}
