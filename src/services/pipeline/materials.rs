use serde::Serialize;

use crate::error::AppResult;
use crate::models::artifact::{ArtifactKind, ArtifactRecord, ArtifactStatus, ArtifactStep};
use crate::models::finding::DependencyFindingsArtifact;
use crate::models::identity::{ChainAlias, ChainId, EvmAddress, RunId};
use crate::models::path::WorkspaceRelPath;
use crate::models::tooling::MaterialStatusSnapshot;
use crate::workspace::paths;

use super::AuditPipelineService;

const MATERIAL_NOTES: &[&str] = &[
    "This manifest is a neutral map of prepared review materials.",
    "Use it to locate evidence; do not treat it as an audit conclusion.",
    "Repository-side findings, when present, live in artifacts/dependency_findings.json.",
    "Directly-invoked tools may leave optional artifacts under runs/<run_id>/artifacts/ that are not produced by the CLI itself.",
];

#[derive(Serialize)]
struct RunTargetRef<'a> {
    address: &'a EvmAddress,
    chain: &'a ChainAlias,
    #[serde(skip_serializing_if = "Option::is_none")]
    chain_id: Option<ChainId>,
}

#[derive(Serialize)]
struct MaterialsManifestRef<'a> {
    target: RunTargetRef<'a>,
    run_id: &'a RunId,
    statuses: MaterialStatusSnapshot,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    inputs: Vec<WorkspaceRelPath>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    core_materials: Vec<WorkspaceRelPath>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    optional_tool_artifacts: Vec<WorkspaceRelPath>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    artifact_records: Vec<&'a ArtifactRecord>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    notes: Vec<&'static str>,
}

impl AuditPipelineService {
    pub fn aggregate_materials(
        &mut self,
        address: &EvmAddress,
        chain: &ChainAlias,
    ) -> AppResult<WorkspaceRelPath> {
        let mut optional_tool_artifacts = self.existing_paths(&[
            "artifacts/chain_checks_plan.json",
            "artifacts/chain_checks_output.txt",
            "artifacts/chain_checks_findings.json",
            "artifacts/chain_index.json",
            "artifacts/static_plan.json",
            "artifacts/slither_raw.json",
            "artifacts/static_findings.json",
            "artifacts/analyzer_index.json",
            paths::TOOLING_MANIFEST,
            paths::SLITHER_BUILD_MANIFEST,
            "slither_project/remappings.txt",
            "slither_project/slither_inputs.json",
            paths::FOUNDRY_BUILD_MANIFEST,
            "foundry_project/foundry.toml",
            "foundry_project/remappings.txt",
            paths::ECHIDNA_BUILD_MANIFEST,
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
        let artifact_records = self.artifact_records().iter().collect::<Vec<_>>();
        let manifest_path = self.workspace.store().write_json(
            paths::MATERIALS_MANIFEST,
            &MaterialsManifestRef {
                target: RunTargetRef {
                    address,
                    chain,
                    chain_id: None,
                },
                run_id: self.workspace.run_id(),
                statuses: self.material_status_snapshot()?,
                inputs: self.existing_paths(&[paths::REQUEST, paths::SOURCE_REQUEST]),
                core_materials: self.existing_paths(&[
                    paths::SOURCE_BUNDLE,
                    paths::DEPENDENCY_FINDINGS,
                    paths::DEPENDENCY_CHAIN_CHECKS,
                    paths::PROXY_CHECKS,
                    paths::ORACLE_CHECKS,
                    paths::FLASH_LOAN_SURFACE,
                ]),
                optional_tool_artifacts,
                artifact_records,
                notes: MATERIAL_NOTES.to_vec(),
            },
        )?;
        self.record(
            ArtifactStep::AggregateMaterials,
            &manifest_path,
            ArtifactKind::Report,
            ArtifactStatus::Executed,
            "Stored a neutral manifest of prepared review materials.",
        );
        Ok(manifest_path)
    }

    pub fn material_status_snapshot(&self) -> AppResult<MaterialStatusSnapshot> {
        let source_payload = self.load_source_bundle_payload()?;
        let dependency_payload: DependencyFindingsArtifact = super::support::read_json_if_exists(
            &self.workspace.paths().resolve(paths::DEPENDENCY_FINDINGS),
        )?;
        Ok(MaterialStatusSnapshot {
            source_fetch_status: source_payload.status,
            dependency_analysis_status: dependency_payload.status,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use crate::models::finding::{
        ChainCheckStatus, DependencyChainChecksArtifact, DependencyFindingsArtifact,
        FlashLoanSurfaceArtifact, OracleChecksArtifact, ProxyChecksArtifact,
    };
    use crate::models::identity::{ChainAlias, EvmAddress, RunId};
    use crate::models::run::{RunRequest, RunTarget};
    use crate::models::source::SourceBundleArtifact;
    use crate::models::step::StepStatus;
    use crate::services::pipeline::AuditPipelineService;
    use crate::workspace::RunWorkspace;
    use std::fs;
    use tempfile::TempDir;

    fn test_workspace() -> (TempDir, RunWorkspace, RunTarget) {
        let temp = TempDir::new().expect("temp dir");
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
        workspace
            .store()
            .write_json(
                paths::REQUEST,
                &RunRequest {
                    address: target.address.clone(),
                    chain: target.chain.clone(),
                },
            )
            .expect("write request");
        (temp, workspace, target)
    }

    #[test]
    fn aggregate_materials_includes_dependency_chain_artifacts() {
        let (_temp, workspace, target) = test_workspace();
        workspace
            .store()
            .write_json(
                paths::SOURCE_BUNDLE,
                &SourceBundleArtifact {
                    target: target.clone(),
                    status: StepStatus::SourceFetched,
                    ..SourceBundleArtifact::default()
                },
            )
            .expect("source bundle");
        workspace
            .store()
            .write_json(
                paths::DEPENDENCY_FINDINGS,
                &DependencyFindingsArtifact::new(target.clone(), StepStatus::Executed, Vec::new()),
            )
            .expect("dependency findings");
        workspace
            .store()
            .write_json(
                paths::DEPENDENCY_CHAIN_CHECKS,
                &DependencyChainChecksArtifact::new(target.clone(), ChainCheckStatus::Executed),
            )
            .expect("chain checks");
        workspace
            .store()
            .write_json(
                paths::PROXY_CHECKS,
                &ProxyChecksArtifact::new(target.clone(), ChainCheckStatus::Executed),
            )
            .expect("proxy checks");
        workspace
            .store()
            .write_json(
                paths::ORACLE_CHECKS,
                &OracleChecksArtifact::new(target.clone(), ChainCheckStatus::Executed),
            )
            .expect("oracle checks");
        workspace
            .store()
            .write_json(
                paths::FLASH_LOAN_SURFACE,
                &FlashLoanSurfaceArtifact::new(target.clone(), ChainCheckStatus::Executed),
            )
            .expect("flash checks");

        let config = AppConfig::load(Some(workspace.project_root.clone())).expect("config");
        let mut service = AuditPipelineService::new(config, workspace);
        let manifest = service
            .aggregate_materials(&target.address, &target.chain)
            .expect("aggregate");
        let text = fs::read_to_string(service.workspace.paths().resolve(manifest.as_str()))
            .expect("read manifest");

        assert!(text.contains(paths::DEPENDENCY_CHAIN_CHECKS));
        assert!(text.contains(paths::PROXY_CHECKS));
        assert!(text.contains(paths::ORACLE_CHECKS));
        assert!(text.contains(paths::FLASH_LOAN_SURFACE));
    }
}
