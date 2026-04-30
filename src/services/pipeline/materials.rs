use crate::error::AppResult;
use crate::models::finding::DependencyFindingsArtifact;
use crate::models::identity::{ChainAlias, EvmAddress};
use crate::models::path::WorkspaceRelPath;
use crate::models::run::RunTarget;
use crate::models::tooling::{MaterialStatusSnapshot, MaterialsManifest};

use super::AuditPipelineService;

const MATERIAL_NOTES: &[&str] = &[
    "This manifest is a neutral map of prepared review materials.",
    "Use it to locate evidence; do not treat it as an audit conclusion.",
    "Repository-side findings, when present, live in artifacts/dependency_findings.json.",
    "Directly-invoked tools may leave optional artifacts under runs/<run_id>/artifacts/ that are not produced by the CLI itself.",
];

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
            &MaterialsManifest {
                target: RunTarget::new(address.clone(), chain.clone()),
                run_id: self.workspace.run_id.clone(),
                statuses: self.material_status_snapshot()?,
                inputs: self.existing_paths(&["input/request.json", "input/source_request.json"]),
                core_materials: self.existing_paths(&[
                    "artifacts/source_bundle.json",
                    "artifacts/dependency_findings.json",
                ]),
                optional_tool_artifacts,
                artifact_records: self.artifacts.clone(),
                notes: MATERIAL_NOTES
                    .iter()
                    .map(|item| (*item).to_string())
                    .collect(),
            },
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

    pub fn material_status_snapshot(&self) -> AppResult<MaterialStatusSnapshot> {
        let source_payload = self.load_source_bundle_payload()?;
        let dependency_payload: DependencyFindingsArtifact = super::support::read_json_if_exists(
            &self
                .workspace
                .root
                .join("artifacts/dependency_findings.json"),
        )?;
        Ok(MaterialStatusSnapshot {
            source_fetch_status: source_payload.status,
            dependency_analysis_status: dependency_payload.status,
        })
    }
}
