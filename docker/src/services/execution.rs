use crate::config::AppConfig;
use crate::error::{AppError, AppResult};
use crate::models::command::CommandName;
use crate::models::envelope::{
    AggregateMaterialsDetails, FetchSourceDetails, InitRunDetails, PrepareSlitherDetails,
    StepPayload, SyncRunPayload,
};
use crate::models::identity::{ChainAlias, EvmAddress, RunId};
use crate::models::path::WorkspaceRelPath;
use crate::models::run::RunRequest;
use crate::models::step::StepStatus;
use crate::output::EXIT_OK;
use crate::services::pipeline::AuditPipelineService;
use crate::services::run_sync::sync_run_to_mongo;
use crate::workspace::{RunGuard, RunWorkspace, load_request_context, paths};

pub const INIT_RUN_COMMAND: CommandName = CommandName::InitRun;
pub const SYNC_RUN_COMMAND: CommandName = CommandName::SyncRun;

const SLITHER_PROJECT_ROOT: &str = "slither_project";

pub type ExecutionResult<T> = Result<T, ExecutionError>;

#[derive(Clone, Debug)]
pub struct InitRunInput {
    pub address: EvmAddress,
    pub chain: ChainAlias,
}

pub struct ExecutionError {
    pub run_id: Option<RunId>,
    pub source: AppError,
}

impl ExecutionError {
    fn without_run_id(source: AppError) -> Self {
        Self {
            run_id: None,
            source,
        }
    }

    fn with_run_id(run_id: RunId, source: AppError) -> Self {
        Self {
            run_id: Some(run_id),
            source,
        }
    }
}

pub fn execute_init_run(config: &AppConfig, input: InitRunInput) -> ExecutionResult<StepPayload> {
    let InitRunInput { address, chain } = input;
    let workspace = RunWorkspace::create(&config.project_root, &config.runs_dir, &address, &chain)
        .map_err(ExecutionError::without_run_id)?;
    let run_id = workspace.run_id().clone();
    workspace
        .store()
        .write_json(paths::REQUEST, &RunRequest { address, chain })
        .map_err(|source| ExecutionError::with_run_id(run_id.clone(), source))?;

    let mut run = LockedRunContext::from_workspace(config, INIT_RUN_COMMAND, workspace)
        .map_err(|source| ExecutionError::with_run_id(run_id.clone(), source))?;
    execute_full_prepare(run.context_mut())
        .map_err(|source| ExecutionError::with_run_id(run.run_id().clone(), source))
}

pub fn parse_init_run_input(
    config: &AppConfig,
    address: &str,
    chain: Option<&str>,
) -> ExecutionResult<InitRunInput> {
    let address = address
        .parse::<EvmAddress>()
        .map_err(ExecutionError::without_run_id)?;
    let chain = match chain {
        Some(chain) => chain
            .parse::<ChainAlias>()
            .map_err(ExecutionError::without_run_id)?,
        None => config.default_chain.clone(),
    };
    Ok(InitRunInput { address, chain })
}

pub fn parse_run_id(raw: &str) -> ExecutionResult<RunId> {
    raw.parse::<RunId>().map_err(ExecutionError::without_run_id)
}

pub fn execute_workspace_step(
    config: &AppConfig,
    step: WorkspaceStep,
    run_id: RunId,
) -> ExecutionResult<StepPayload> {
    let mut run = LockedRunContext::load(config, step.command_name(), &run_id)
        .map_err(|source| ExecutionError::with_run_id(run_id.clone(), source))?;
    step.execute(run.context_mut())
        .map_err(|source| ExecutionError::with_run_id(run.run_id().clone(), source))
}

pub fn execute_sync_run(
    config: &AppConfig,
    run_id: RunId,
) -> ExecutionResult<(SyncRunPayload, i32)> {
    let workspace = RunWorkspace::load(&config.project_root, &config.runs_dir, &run_id)
        .map_err(|source| ExecutionError::with_run_id(run_id.clone(), source))?;
    let sync = sync_run_to_mongo(config, &workspace)
        .map_err(|source| ExecutionError::with_run_id(run_id.clone(), source))?;

    Ok((
        SyncRunPayload {
            status: crate::models::envelope::CommandStatus::Completed,
            file_count: sync.file_count,
            total_size_bytes: sync.total_size_bytes,
            upserted_file_records: sync.upserted_file_records,
        },
        EXIT_OK,
    ))
}

pub struct RunExecutionContext {
    request: RunRequest,
    pipeline: AuditPipelineService,
}

impl RunExecutionContext {
    fn from_workspace(config: &AppConfig, workspace: RunWorkspace) -> AppResult<Self> {
        let request = load_request_context(&workspace)?;
        let pipeline = AuditPipelineService::new(config.clone(), workspace);
        Ok(Self { request, pipeline })
    }
}

struct LockedRunContext {
    _guard: RunGuard,
    context: RunExecutionContext,
}

impl LockedRunContext {
    fn load(config: &AppConfig, command: CommandName, run_id: &RunId) -> AppResult<Self> {
        let workspace = RunWorkspace::load(&config.project_root, &config.runs_dir, run_id)?;
        Self::from_workspace(config, command, workspace)
    }

    fn from_workspace(
        config: &AppConfig,
        _command: CommandName,
        workspace: RunWorkspace,
    ) -> AppResult<Self> {
        let guard = workspace.lock_handle().acquire()?;
        let context = RunExecutionContext::from_workspace(config, workspace)?;
        Ok(Self {
            _guard: guard,
            context,
        })
    }

    fn context_mut(&mut self) -> &mut RunExecutionContext {
        &mut self.context
    }

    fn run_id(&self) -> &RunId {
        self.context.pipeline.workspace.run_id()
    }
}

#[derive(Clone, Copy)]
pub enum WorkspaceStep {
    FetchSource,
    RunDependency,
    PrepareSlither,
    PrepareTooling,
    AggregateMaterials,
}

impl WorkspaceStep {
    pub const fn command_name(self) -> CommandName {
        match self {
            Self::FetchSource => CommandName::FetchSource,
            Self::RunDependency => CommandName::RunDependency,
            Self::PrepareSlither => CommandName::PrepareSlither,
            Self::PrepareTooling => CommandName::PrepareTooling,
            Self::AggregateMaterials => CommandName::AggregateMaterials,
        }
    }

    const fn log_path(self) -> &'static str {
        match self {
            Self::FetchSource => paths::FETCH_SOURCE_LOG,
            Self::RunDependency => paths::RUN_DEPENDENCY_LOG,
            Self::PrepareSlither => paths::PREPARE_SLITHER_LOG,
            Self::PrepareTooling => paths::PREPARE_TOOLING_LOG,
            Self::AggregateMaterials => paths::AGGREGATE_MATERIALS_LOG,
        }
    }

    fn execute(self, run: &mut RunExecutionContext) -> AppResult<StepPayload> {
        let outcome = match self {
            Self::FetchSource => self.fetch_source(run)?,
            Self::RunDependency => self.run_dependency(run)?,
            Self::PrepareSlither => self.prepare_slither(run)?,
            Self::PrepareTooling => self.prepare_tooling(run)?,
            Self::AggregateMaterials => self.aggregate_materials(run)?,
        };

        persist_step_payload(
            &run.pipeline.workspace,
            self.command_name(),
            self.log_path(),
            &run.pipeline,
            outcome,
        )
    }

    fn fetch_source(self, run: &mut RunExecutionContext) -> AppResult<StepOutcome> {
        let status = run
            .pipeline
            .fetch_contract_source(&run.request.address, &run.request.chain)?;
        let tooling_status = run
            .pipeline
            .prepare_tooling_workspaces(&run.request.address, &run.request.chain)?;

        Ok(
            StepOutcome::new(status).with_fetch_source(FetchSourceDetails {
                tooling_status,
                tooling_manifest_path: WorkspaceRelPath::new(paths::TOOLING_MANIFEST),
                slither_build_manifest_path: WorkspaceRelPath::new(paths::SLITHER_BUILD_MANIFEST),
                foundry_build_manifest_path: WorkspaceRelPath::new(paths::FOUNDRY_BUILD_MANIFEST),
                echidna_build_manifest_path: WorkspaceRelPath::new(paths::ECHIDNA_BUILD_MANIFEST),
            }),
        )
    }

    fn run_dependency(self, run: &mut RunExecutionContext) -> AppResult<StepOutcome> {
        let status = run
            .pipeline
            .run_dependency_analysis(&run.request.address, &run.request.chain)?;
        Ok(StepOutcome::new(status))
    }

    fn prepare_slither(self, run: &mut RunExecutionContext) -> AppResult<StepOutcome> {
        let status = run
            .pipeline
            .prepare_slither_project(&run.request.address, &run.request.chain)?;

        Ok(
            StepOutcome::new(status).with_prepare_slither(PrepareSlitherDetails {
                slither_build_manifest_path: WorkspaceRelPath::new(paths::SLITHER_BUILD_MANIFEST),
                slither_project_root: WorkspaceRelPath::new(SLITHER_PROJECT_ROOT),
            }),
        )
    }

    fn prepare_tooling(self, run: &mut RunExecutionContext) -> AppResult<StepOutcome> {
        let status = run
            .pipeline
            .prepare_tooling_workspaces(&run.request.address, &run.request.chain)?;
        Ok(StepOutcome::new(status))
    }

    fn aggregate_materials(self, run: &mut RunExecutionContext) -> AppResult<StepOutcome> {
        let manifest_path = run
            .pipeline
            .aggregate_materials(&run.request.address, &run.request.chain)?;
        Ok(
            StepOutcome::new(StepStatus::Executed).with_aggregate_materials(
                AggregateMaterialsDetails {
                    materials_manifest_path: manifest_path,
                },
            ),
        )
    }
}

struct StepOutcome {
    status: StepStatus,
    init_run: Option<InitRunDetails>,
    fetch_source: Option<FetchSourceDetails>,
    prepare_slither: Option<PrepareSlitherDetails>,
    aggregate_materials: Option<AggregateMaterialsDetails>,
}

impl StepOutcome {
    fn new(status: StepStatus) -> Self {
        Self {
            status,
            init_run: None,
            fetch_source: None,
            prepare_slither: None,
            aggregate_materials: None,
        }
    }

    fn with_init_run(mut self, details: InitRunDetails) -> Self {
        self.init_run = Some(details);
        self
    }

    fn with_fetch_source(mut self, details: FetchSourceDetails) -> Self {
        self.fetch_source = Some(details);
        self
    }

    fn with_prepare_slither(mut self, details: PrepareSlitherDetails) -> Self {
        self.prepare_slither = Some(details);
        self
    }

    fn with_aggregate_materials(mut self, details: AggregateMaterialsDetails) -> Self {
        self.aggregate_materials = Some(details);
        self
    }

    fn into_payload(
        self,
        workspace: &RunWorkspace,
        step: CommandName,
        artifact_index: WorkspaceRelPath,
    ) -> StepPayload {
        StepPayload {
            run_id: workspace.run_id().clone(),
            run_dir: workspace.root().to_path_buf(),
            step,
            status: self.status,
            artifact_index,
            init_run: self.init_run,
            fetch_source: self.fetch_source,
            prepare_slither: self.prepare_slither,
            aggregate_materials: self.aggregate_materials,
        }
    }
}

fn execute_full_prepare(run: &mut RunExecutionContext) -> AppResult<StepPayload> {
    let chain = run.request.chain.clone();
    let address = run.request.address.clone();
    let source_status = run.pipeline.fetch_contract_source(&address, &chain)?;
    let dependency_status = run.pipeline.run_dependency_analysis(&address, &chain)?;
    let tooling_status = run.pipeline.prepare_tooling_workspaces(&address, &chain)?;
    let materials_manifest_path = run.pipeline.aggregate_materials(&address, &chain)?;

    persist_step_payload(
        &run.pipeline.workspace,
        INIT_RUN_COMMAND,
        paths::INIT_RUN_LOG,
        &run.pipeline,
        StepOutcome::new(full_prepare_status(
            source_status,
            dependency_status,
            tooling_status,
        ))
        .with_init_run(InitRunDetails {
            address,
            chain,
            source_fetch_status: source_status,
            dependency_analysis_status: dependency_status,
            tooling_status,
            tooling_manifest_path: WorkspaceRelPath::new(paths::TOOLING_MANIFEST),
            materials_manifest_path,
            slither_build_manifest_path: WorkspaceRelPath::new(paths::SLITHER_BUILD_MANIFEST),
            foundry_build_manifest_path: WorkspaceRelPath::new(paths::FOUNDRY_BUILD_MANIFEST),
            echidna_build_manifest_path: WorkspaceRelPath::new(paths::ECHIDNA_BUILD_MANIFEST),
        }),
    )
}

fn persist_step_payload(
    workspace: &RunWorkspace,
    step: CommandName,
    log_path: &str,
    pipeline: &AuditPipelineService,
    outcome: StepOutcome,
) -> AppResult<StepPayload> {
    let payload = outcome.into_payload(workspace, step, pipeline.write_artifact_index()?);
    workspace.store().write_json(log_path, &payload)?;
    Ok(payload)
}

fn full_prepare_status(
    source_status: StepStatus,
    dependency_status: StepStatus,
    tooling_status: StepStatus,
) -> StepStatus {
    if source_status != StepStatus::SourceFetched {
        return source_status;
    }
    if dependency_status != StepStatus::Executed {
        return dependency_status;
    }
    if tooling_status != StepStatus::Prepared {
        return tooling_status;
    }
    StepStatus::Prepared
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_prepare_status_returns_first_incomplete_step() {
        assert_eq!(
            full_prepare_status(
                StepStatus::SourceFetchFailed,
                StepStatus::Executed,
                StepStatus::Prepared
            ),
            StepStatus::SourceFetchFailed
        );
        assert_eq!(
            full_prepare_status(
                StepStatus::SourceFetched,
                StepStatus::SourceNotFetched,
                StepStatus::Prepared
            ),
            StepStatus::SourceNotFetched
        );
        assert_eq!(
            full_prepare_status(
                StepStatus::SourceFetched,
                StepStatus::Executed,
                StepStatus::SourceFilesMissing
            ),
            StepStatus::SourceFilesMissing
        );
    }
}
