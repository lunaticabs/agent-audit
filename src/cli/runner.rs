use clap::Parser;

use crate::cli::args::{Cli, Command, InitRunArgs, RunIdArgs};
use crate::config::AppConfig;
use crate::error::AppResult;
use crate::models::command::CommandName;
use crate::models::envelope::{
    CommandEnvelope, CommandStatus, NextAction, StepPayload, SyncRunPayload,
};
use crate::models::identity::RunId;
use crate::output::{error_envelope, print_json, step_envelope};
use crate::services::execution::{
    ExecutionError, INIT_RUN_COMMAND, InitRunInput, SYNC_RUN_COMMAND, WorkspaceStep,
    execute_init_run, execute_sync_run, execute_workspace_step, parse_init_run_input, parse_run_id,
};

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

    fn execute_step(&self, step: WorkspaceStep, args: &RunIdArgs) -> CommandResult<CommandOutput> {
        let run_id = parse_run_id(&args.run_id)
            .map_err(|error| Box::new(CommandFailure::for_command(step.command_name(), error)))?;
        let payload = execute_workspace_step(&self.config, step, run_id.clone())
            .map_err(|error| Box::new(CommandFailure::for_command(step.command_name(), error)))?;
        Ok(CommandOutput::step(&run_id, payload))
    }

    fn execute_sync_run(&self, args: &RunIdArgs) -> CommandResult<CommandOutput> {
        let run_id = parse_run_id(&args.run_id)
            .map_err(|error| Box::new(CommandFailure::for_command(SYNC_RUN_COMMAND, error)))?;
        let (payload, exit_code) = execute_sync_run(&self.config, run_id.clone())
            .map_err(|error| Box::new(CommandFailure::for_command(SYNC_RUN_COMMAND, error)))?;
        Ok(CommandOutput::sync(run_id, payload, exit_code))
    }

    fn parse_init_run_input(&self, args: InitRunArgs) -> Result<InitRunInput, ExecutionError> {
        parse_init_run_input(&self.config, &args.address, args.chain.as_deref())
    }
}

impl Command {
    fn execute(self, app: &CliApp) -> CommandResult<CommandOutput> {
        match self {
            Self::InitRun(args) => {
                let input = app.parse_init_run_input(args).map_err(|error| {
                    Box::new(CommandFailure::for_command(INIT_RUN_COMMAND, error))
                })?;
                let payload = execute_init_run(&app.config, input).map_err(|error| {
                    Box::new(CommandFailure::for_command(INIT_RUN_COMMAND, error))
                })?;
                let run_id = payload.run_id.clone();
                Ok(CommandOutput::step(&run_id, payload))
            }
            Self::FetchSource(args) => app.execute_step(WorkspaceStep::FetchSource, &args),
            Self::RunDependency(args) => app.execute_step(WorkspaceStep::RunDependency, &args),
            Self::PrepareSlither(args) => app.execute_step(WorkspaceStep::PrepareSlither, &args),
            Self::PrepareTooling(args) => app.execute_step(WorkspaceStep::PrepareTooling, &args),
            Self::AggregateMaterials(args) => {
                app.execute_step(WorkspaceStep::AggregateMaterials, &args)
            }
            Self::SyncRun(args) => app.execute_sync_run(&args),
        }
    }
}

type CommandResult<T> = Result<T, Box<CommandFailure>>;

struct CommandFailure {
    command: Option<CommandName>,
    run_id: Option<RunId>,
    source: crate::error::AppError,
}

impl CommandFailure {
    fn bootstrap(source: crate::error::AppError) -> Self {
        Self {
            command: None,
            run_id: None,
            source,
        }
    }

    fn for_command(command: CommandName, error: ExecutionError) -> Self {
        Self {
            command: Some(command),
            run_id: error.run_id,
            source: error.source,
        }
    }

    fn emit(self) -> i32 {
        let (envelope, code) = error_envelope(self.command, self.run_id.as_ref(), &self.source);
        print_json(&envelope);
        code
    }
}

enum CommandOutput {
    Step {
        envelope: Box<CommandEnvelope<StepPayload>>,
        exit_code: i32,
    },
    Sync {
        envelope: Box<CommandEnvelope<SyncRunPayload>>,
        exit_code: i32,
    },
}

impl CommandOutput {
    fn step(run_id: &RunId, payload: StepPayload) -> Self {
        let (envelope, exit_code) = step_envelope(run_id, payload);
        Self::Step {
            envelope: Box::new(envelope),
            exit_code,
        }
    }

    fn sync(run_id: RunId, payload: SyncRunPayload, exit_code: i32) -> Self {
        Self::Sync {
            envelope: Box::new(CommandEnvelope {
                ok: true,
                status: CommandStatus::Completed,
                retryable: false,
                run_id: Some(run_id),
                run_persisted: true,
                payload: Some(payload),
                error: None,
                next_action: NextAction::Continue,
            }),
            exit_code,
        }
    }

    fn emit(self) -> i32 {
        match self {
            Self::Step {
                envelope,
                exit_code,
            } => {
                print_json(&envelope);
                exit_code
            }
            Self::Sync {
                envelope,
                exit_code,
            } => {
                print_json(&envelope);
                exit_code
            }
        }
    }
}
