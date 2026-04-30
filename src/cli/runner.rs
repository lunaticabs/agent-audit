use std::sync::OnceLock;

use clap::Parser;
use regex::Regex;
use serde_json::{Value, to_value};

use crate::cli::args::{Cli, Command, InitRunArgs};
use crate::config::AppConfig;
use crate::error::{AppError, AppResult};
use crate::models::envelope::{
    AggregateMaterialsDetails, FetchSourceDetails, InitRunDetails, PrepareSlitherDetails,
    StepPayload, SyncRunPayload,
};
use crate::models::envelope::{CommandEnvelope, NextAction};
use crate::models::run::RunRequest;
use crate::output::{EXIT_OK, error_envelope, print_json, step_envelope};
use crate::services::pipeline::AuditPipelineService;
use crate::services::run_sync::sync_run_to_mongo;
use crate::workspace::{RunGuard, RunWorkspace, load_request_context};

const INIT_RUN_COMMAND: &str = "init-run";
const SYNC_RUN_COMMAND: &str = "sync-run";
const INIT_RUN_LOG_PATH: &str = "logs/init_run_result.json";
const TOOLING_MANIFEST_PATH: &str = "artifacts/tooling_manifest.json";
const SLITHER_BUILD_MANIFEST_PATH: &str = "slither_project/build_manifest.json";
const FOUNDRY_BUILD_MANIFEST_PATH: &str = "foundry_project/build_manifest.json";
const ECHIDNA_BUILD_MANIFEST_PATH: &str = "echidna_project/build_manifest.json";
const SLITHER_PROJECT_ROOT: &str = "slither_project";

pub fn run() -> i32 {
    let cli = Cli::parse();
    match CliApp::bootstrap() {
        Ok(app) => app.execute(cli.command),
        Err(source) => CommandFailure::bootstrap(source).emit(),
    }
}

struct CliApp {
    config: AppConfig,
}

impl CliApp {
    fn bootstrap() -> AppResult<Self> {
        Ok(Self {
            config: AppConfig::load(None)?,
        })
    }

    fn execute(&self, command: Command) -> i32 {
        match command.execute(self) {
            Ok(output) => output.emit(),
            Err(error) => error.emit(),
        }
    }

    fn execute_init_run(&self, args: InitRunArgs) -> CommandResult<CommandOutput> {
        let address = validate_address(&args.address)
            .map_err(|source| CommandFailure::for_command(INIT_RUN_COMMAND, "", source))?;
        let chain = args
            .chain
            .unwrap_or_else(|| self.config.default_chain.clone());
        let workspace = RunWorkspace::create(
            &self.config.project_root,
            &self.config.runs_dir,
            &address,
            &chain,
        )
        .map_err(|source| CommandFailure::for_command(INIT_RUN_COMMAND, "", source))?;
        let run_id = workspace.run_id.clone();
        workspace
            .write_json("input/request.json", &RunRequest { address, chain })
            .map_err(|source| CommandFailure::for_command(INIT_RUN_COMMAND, &run_id, source))?;

        let mut run = LockedRunContext::from_workspace(&self.config, INIT_RUN_COMMAND, workspace)?;
        let run_id = run.run_id().to_owned();
        let payload = execute_full_prepare(run.context_mut())
            .map_err(|source| CommandFailure::for_command(INIT_RUN_COMMAND, &run_id, source))?;
        Ok(CommandOutput::step(INIT_RUN_COMMAND, &run_id, payload))
    }

    fn execute_step(&self, step: WorkspaceStep, run_id: &str) -> CommandResult<CommandOutput> {
        let mut run = LockedRunContext::load(&self.config, step.command_name(), run_id)?;
        let run_id = run.run_id().to_owned();
        let payload = step
            .execute(run.context_mut())
            .map_err(|source| CommandFailure::for_command(step.command_name(), &run_id, source))?;
        Ok(CommandOutput::step(step.command_name(), &run_id, payload))
    }

    fn execute_sync_run(&self, run_id: &str) -> CommandResult<CommandOutput> {
        let workspace =
            RunWorkspace::load(&self.config.project_root, &self.config.runs_dir, run_id)
                .map_err(|source| CommandFailure::for_command(SYNC_RUN_COMMAND, run_id, source))?;
        let sync = sync_run_to_mongo(&self.config, &workspace)
            .map_err(|source| CommandFailure::for_command(SYNC_RUN_COMMAND, run_id, source))?;

        Ok(CommandOutput::json(
            to_value(CommandEnvelope {
                ok: true,
                status: "completed".to_string(),
                retryable: false,
                run_id: sync.run_id,
                run_persisted: true,
                payload: Some(SyncRunPayload {
                    status: "completed".to_string(),
                    file_count: sync.file_count,
                    total_size_bytes: sync.total_size_bytes,
                    upserted_file_records: sync.upserted_file_records,
                }),
                error: None,
                next_action: NextAction::Continue,
            })
            .map_err(AppError::from)
            .map_err(CommandFailure::bootstrap)?,
            EXIT_OK,
        ))
    }
}

impl Command {
    fn execute(self, app: &CliApp) -> CommandResult<CommandOutput> {
        match self {
            Self::InitRun(args) => app.execute_init_run(args),
            Self::FetchSource(args) => app.execute_step(WorkspaceStep::FetchSource, &args.run_id),
            Self::RunDependency(args) => {
                app.execute_step(WorkspaceStep::RunDependency, &args.run_id)
            }
            Self::PrepareSlither(args) => {
                app.execute_step(WorkspaceStep::PrepareSlither, &args.run_id)
            }
            Self::PrepareTooling(args) => {
                app.execute_step(WorkspaceStep::PrepareTooling, &args.run_id)
            }
            Self::AggregateMaterials(args) => {
                app.execute_step(WorkspaceStep::AggregateMaterials, &args.run_id)
            }
            Self::SyncRun(args) => app.execute_sync_run(&args.run_id),
        }
    }
}

type CommandResult<T> = Result<T, CommandFailure>;

struct CommandFailure {
    command: Option<&'static str>,
    run_id: String,
    source: AppError,
}

impl CommandFailure {
    fn bootstrap(source: AppError) -> Self {
        Self {
            command: None,
            run_id: String::new(),
            source,
        }
    }

    fn for_command(command: &'static str, run_id: impl Into<String>, source: AppError) -> Self {
        Self {
            command: Some(command),
            run_id: run_id.into(),
            source,
        }
    }

    fn emit(self) -> i32 {
        let (envelope, code) = error_envelope(self.command, &self.run_id, &self.source);
        print_json(&envelope);
        code
    }
}

struct CommandOutput {
    envelope: Value,
    exit_code: i32,
}

impl CommandOutput {
    fn step(command: &str, run_id: &str, payload: Value) -> Self {
        let (envelope, exit_code) = step_envelope(command, run_id, payload);
        Self {
            envelope: to_value(envelope).expect("serialize step envelope"),
            exit_code,
        }
    }

    fn json(envelope: Value, exit_code: i32) -> Self {
        Self {
            envelope,
            exit_code,
        }
    }

    fn emit(self) -> i32 {
        print_json(&self.envelope);
        self.exit_code
    }
}

struct RunExecutionContext {
    workspace: RunWorkspace,
    request: RunRequest,
    pipeline: AuditPipelineService,
}

impl RunExecutionContext {
    fn from_workspace(config: &AppConfig, workspace: RunWorkspace) -> AppResult<Self> {
        let request = load_request_context(&workspace)?;
        let pipeline = AuditPipelineService::new(config.clone(), workspace.clone());
        Ok(Self {
            workspace,
            request,
            pipeline,
        })
    }
}

struct LockedRunContext {
    _guard: RunGuard,
    context: RunExecutionContext,
}

impl LockedRunContext {
    fn load(config: &AppConfig, command: &'static str, run_id: &str) -> CommandResult<Self> {
        let workspace = RunWorkspace::load(&config.project_root, &config.runs_dir, run_id)
            .map_err(|source| CommandFailure::for_command(command, run_id, source))?;
        Self::from_workspace(config, command, workspace)
    }

    fn from_workspace(
        config: &AppConfig,
        command: &'static str,
        workspace: RunWorkspace,
    ) -> CommandResult<Self> {
        let run_id = workspace.run_id.clone();
        let guard = workspace
            .lock()
            .map_err(|source| CommandFailure::for_command(command, &run_id, source))?;
        let context = RunExecutionContext::from_workspace(config, workspace)
            .map_err(|source| CommandFailure::for_command(command, &run_id, source))?;
        Ok(Self {
            _guard: guard,
            context,
        })
    }

    fn context_mut(&mut self) -> &mut RunExecutionContext {
        &mut self.context
    }

    fn run_id(&self) -> &str {
        &self.context.workspace.run_id
    }
}

#[derive(Clone, Copy)]
enum WorkspaceStep {
    FetchSource,
    RunDependency,
    PrepareSlither,
    PrepareTooling,
    AggregateMaterials,
}

impl WorkspaceStep {
    const fn command_name(self) -> &'static str {
        match self {
            Self::FetchSource => "fetch-source",
            Self::RunDependency => "run-dependency",
            Self::PrepareSlither => "prepare-slither",
            Self::PrepareTooling => "prepare-tooling",
            Self::AggregateMaterials => "aggregate-materials",
        }
    }

    const fn log_path(self) -> &'static str {
        match self {
            Self::FetchSource => "logs/fetch_source_result.json",
            Self::RunDependency => "logs/run_dependency_result.json",
            Self::PrepareSlither => "logs/prepare_slither_result.json",
            Self::PrepareTooling => "logs/prepare_tooling_result.json",
            Self::AggregateMaterials => "logs/aggregate_materials_result.json",
        }
    }

    fn execute(self, run: &mut RunExecutionContext) -> AppResult<Value> {
        let outcome = match self {
            Self::FetchSource => self.fetch_source(run)?,
            Self::RunDependency => self.run_dependency(run)?,
            Self::PrepareSlither => self.prepare_slither(run)?,
            Self::PrepareTooling => self.prepare_tooling(run)?,
            Self::AggregateMaterials => self.aggregate_materials(run)?,
        };

        let payload = outcome.into_payload(
            &run.workspace,
            self.command_name(),
            run.pipeline.write_artifact_index()?,
        );
        run.workspace.write_json(self.log_path(), &payload)?;
        to_value(payload).map_err(Into::into)
    }

    fn fetch_source(self, run: &mut RunExecutionContext) -> AppResult<StepOutcome> {
        let address = run.request.address.as_str();
        let chain = run.request.chain.as_str();
        let status = run.pipeline.fetch_contract_source(address, chain)?;
        let tooling_status = run.pipeline.prepare_tooling_workspaces(address, chain)?;

        Ok(with_tooling_manifest_fields(
            StepOutcome::new(status).with_fetch_source(FetchSourceDetails {
                tooling_status,
                tooling_manifest_path: TOOLING_MANIFEST_PATH.to_string(),
                slither_build_manifest_path: SLITHER_BUILD_MANIFEST_PATH.to_string(),
                foundry_build_manifest_path: FOUNDRY_BUILD_MANIFEST_PATH.to_string(),
                echidna_build_manifest_path: ECHIDNA_BUILD_MANIFEST_PATH.to_string(),
            }),
        ))
    }

    fn run_dependency(self, run: &mut RunExecutionContext) -> AppResult<StepOutcome> {
        let address = run.request.address.as_str();
        let chain = run.request.chain.as_str();
        let status = run.pipeline.run_dependency_analysis(address, chain)?;
        Ok(StepOutcome::new(status))
    }

    fn prepare_slither(self, run: &mut RunExecutionContext) -> AppResult<StepOutcome> {
        let address = run.request.address.as_str();
        let chain = run.request.chain.as_str();
        let status = run.pipeline.prepare_slither_project(address, chain)?;

        Ok(
            StepOutcome::new(status).with_prepare_slither(PrepareSlitherDetails {
                slither_build_manifest_path: SLITHER_BUILD_MANIFEST_PATH.to_string(),
                slither_project_root: SLITHER_PROJECT_ROOT.to_string(),
            }),
        )
    }

    fn prepare_tooling(self, run: &mut RunExecutionContext) -> AppResult<StepOutcome> {
        let address = run.request.address.as_str();
        let chain = run.request.chain.as_str();
        let status = run.pipeline.prepare_tooling_workspaces(address, chain)?;
        Ok(with_tooling_manifest_fields(StepOutcome::new(status)))
    }

    fn aggregate_materials(self, run: &mut RunExecutionContext) -> AppResult<StepOutcome> {
        let address = run.request.address.as_str();
        let chain = run.request.chain.as_str();
        let manifest_path = run.pipeline.aggregate_materials(address, chain)?;
        Ok(
            StepOutcome::new("executed").with_aggregate_materials(AggregateMaterialsDetails {
                materials_manifest_path: manifest_path,
            }),
        )
    }
}

struct StepOutcome {
    status: String,
    init_run: Option<InitRunDetails>,
    fetch_source: Option<FetchSourceDetails>,
    prepare_slither: Option<PrepareSlitherDetails>,
    aggregate_materials: Option<AggregateMaterialsDetails>,
}

impl StepOutcome {
    fn new(status: impl Into<String>) -> Self {
        Self {
            status: status.into(),
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
        step: &str,
        artifact_index: String,
    ) -> StepPayload {
        StepPayload {
            run_id: workspace.run_id.clone(),
            run_dir: workspace.root.to_string_lossy().into_owned(),
            step: step.to_string(),
            status: self.status,
            artifact_index,
            init_run: self.init_run,
            fetch_source: self.fetch_source,
            prepare_slither: self.prepare_slither,
            aggregate_materials: self.aggregate_materials,
        }
    }
}

fn execute_full_prepare(run: &mut RunExecutionContext) -> AppResult<Value> {
    let address = run.request.address.as_str();
    let chain = run.request.chain.as_str();
    let source_status = run.pipeline.fetch_contract_source(address, chain)?;
    let dependency_status = run.pipeline.run_dependency_analysis(address, chain)?;
    let tooling_status = run.pipeline.prepare_tooling_workspaces(address, chain)?;
    let materials_manifest_path = run.pipeline.aggregate_materials(address, chain)?;

    let payload = with_tooling_manifest_fields(
        StepOutcome::new(full_prepare_status(
            &source_status,
            &dependency_status,
            &tooling_status,
        ))
        .with_init_run(InitRunDetails {
            address: address.to_string(),
            chain: chain.to_string(),
            source_fetch_status: source_status,
            dependency_analysis_status: dependency_status,
            tooling_status,
            tooling_manifest_path: TOOLING_MANIFEST_PATH.to_string(),
            materials_manifest_path,
            slither_build_manifest_path: SLITHER_BUILD_MANIFEST_PATH.to_string(),
            foundry_build_manifest_path: FOUNDRY_BUILD_MANIFEST_PATH.to_string(),
            echidna_build_manifest_path: ECHIDNA_BUILD_MANIFEST_PATH.to_string(),
        }),
    )
    .into_payload(
        &run.workspace,
        INIT_RUN_COMMAND,
        run.pipeline.write_artifact_index()?,
    );

    run.workspace.write_json(INIT_RUN_LOG_PATH, &payload)?;
    to_value(payload).map_err(Into::into)
}

fn with_tooling_manifest_fields(outcome: StepOutcome) -> StepOutcome {
    outcome
}

fn full_prepare_status<'a>(
    source_status: &'a str,
    dependency_status: &'a str,
    tooling_status: &'a str,
) -> &'a str {
    if source_status != "source_fetched" {
        return source_status;
    }
    if dependency_status != "executed" {
        return dependency_status;
    }
    if tooling_status != "prepared" {
        return tooling_status;
    }
    "prepared"
}

fn validate_address(address: &str) -> AppResult<String> {
    static ADDRESS_PATTERN: OnceLock<Regex> = OnceLock::new();
    let pattern =
        ADDRESS_PATTERN.get_or_init(|| Regex::new(r"^0x[a-fA-F0-9]{40}$").expect("valid regex"));
    if !pattern.is_match(address) {
        return Err(AppError::InvalidAddress(address.to_string()));
    }
    Ok(address.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_address_lowercases_valid_input() {
        let input = "0x1234567890abcdef1234567890ABCDEF12345678";
        let expected = "0x1234567890abcdef1234567890abcdef12345678";
        assert_eq!(validate_address(input).unwrap(), expected);
    }

    #[test]
    fn full_prepare_status_returns_first_incomplete_step() {
        assert_eq!(
            full_prepare_status("source_fetch_failed", "executed", "prepared"),
            "source_fetch_failed"
        );
        assert_eq!(
            full_prepare_status("source_fetched", "source_not_fetched", "prepared"),
            "source_not_fetched"
        );
        assert_eq!(
            full_prepare_status("source_fetched", "executed", "source_files_missing"),
            "source_files_missing"
        );
    }
}
