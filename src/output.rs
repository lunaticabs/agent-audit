use serde::Serialize;

use crate::error::AppError;
use crate::models::command::CommandName;
use crate::models::envelope::{
    CommandEnvelope, CommandStatus, EnvelopeError, NextAction, StepPayload,
};
use crate::models::identity::RunId;
use crate::models::step::StepStatus;
use crate::serde_ext::to_pretty_json;

pub const EXIT_OK: i32 = 0;
pub const EXIT_RETRYABLE: i32 = 10;
pub const EXIT_FATAL: i32 = 20;
pub const EXIT_PRECONDITION: i32 = 30;

pub fn print_json<T: Serialize>(value: &T) {
    println!(
        "{}",
        to_pretty_json(value).expect("serialize json envelope")
    );
}

pub fn step_envelope(run_id: &RunId, payload: StepPayload) -> (CommandEnvelope<StepPayload>, i32) {
    let decision = decision_for_step(payload.step, run_id.as_str(), payload.status);
    envelope_with_payload(Some(run_id.clone()), true, Some(payload), decision)
}

pub fn error_envelope(
    command: Option<CommandName>,
    run_id: Option<&RunId>,
    error: &crate::error::AppError,
) -> (CommandEnvelope<()>, i32) {
    let run_id_text = run_id.map(RunId::as_str).unwrap_or_default();
    let decision = decision_for_error(command, run_id_text, error);
    envelope_with_payload(run_id.cloned(), run_id.is_some(), None::<()>, decision)
}

struct EnvelopeDecision {
    ok: bool,
    status: CommandStatus,
    retryable: bool,
    error: Option<EnvelopeError>,
    next_action: NextAction,
    exit_code: i32,
}

fn decision_for_step(
    command: CommandName,
    run_id_text: &str,
    status: StepStatus,
) -> EnvelopeDecision {
    if status.is_retryable_failure() {
        return EnvelopeDecision {
            ok: false,
            status: CommandStatus::RetryableError,
            retryable: true,
            error: None,
            next_action: NextAction::RetrySameCommand {
                command: retry_command_for_step(command, run_id_text),
                retry_after_sec: 5,
                max_retries: 3,
            },
            exit_code: EXIT_RETRYABLE,
        };
    }

    if status.is_precondition_failure() {
        return EnvelopeDecision {
            ok: false,
            status: CommandStatus::PreconditionMissing,
            retryable: false,
            error: None,
            next_action: NextAction::RunPrerequisite {
                command: prerequisite_command_for_step(command, run_id_text),
            },
            exit_code: EXIT_PRECONDITION,
        };
    }

    match status {
        StepStatus::SourceApiNotConfigured => EnvelopeDecision {
            ok: false,
            status: CommandStatus::FatalError,
            retryable: false,
            error: Some(EnvelopeError {
                code: "SOURCE_API_NOT_CONFIGURED".to_string(),
                message: "Configure AGENT_AUDIT_SOURCE_API_BASE before fetch-source.".to_string(),
            }),
            next_action: NextAction::Stop {
                command: Some("set AGENT_AUDIT_SOURCE_API_BASE in .env".to_string()),
            },
            exit_code: EXIT_FATAL,
        },
        _ => completed_decision(),
    }
}

fn decision_for_error(
    command: Option<CommandName>,
    run_id_text: &str,
    error: &AppError,
) -> EnvelopeDecision {
    match error {
        AppError::RunNotFound(message) => EnvelopeDecision {
            ok: false,
            status: CommandStatus::PreconditionMissing,
            retryable: false,
            error: Some(EnvelopeError {
                code: "RUN_NOT_FOUND".to_string(),
                message: message.clone(),
            }),
            next_action: NextAction::RunPrerequisite {
                command: init_run_placeholder_command(),
            },
            exit_code: EXIT_PRECONDITION,
        },
        AppError::InvalidAddress(message) => EnvelopeDecision {
            ok: false,
            status: CommandStatus::FatalError,
            retryable: false,
            error: Some(EnvelopeError {
                code: "INVALID_ARGUMENT".to_string(),
                message: format!("invalid EVM address: {message}"),
            }),
            next_action: NextAction::Stop { command: None },
            exit_code: EXIT_FATAL,
        },
        AppError::Message(message) => EnvelopeDecision {
            ok: false,
            status: CommandStatus::FatalError,
            retryable: false,
            error: Some(EnvelopeError {
                code: "INVALID_ARGUMENT".to_string(),
                message: message.clone(),
            }),
            next_action: NextAction::Stop { command: None },
            exit_code: EXIT_FATAL,
        },
        _ => EnvelopeDecision {
            ok: false,
            status: CommandStatus::RetryableError,
            retryable: true,
            error: Some(EnvelopeError {
                code: "UNHANDLED_EXCEPTION".to_string(),
                message: error.to_string(),
            }),
            next_action: NextAction::RetrySameCommand {
                command: retry_command_for_error(command, run_id_text),
                retry_after_sec: 5,
                max_retries: 2,
            },
            exit_code: EXIT_RETRYABLE,
        },
    }
}

fn envelope_with_payload<T: Serialize>(
    run_id: Option<RunId>,
    run_persisted: bool,
    payload: Option<T>,
    decision: EnvelopeDecision,
) -> (CommandEnvelope<T>, i32) {
    (
        CommandEnvelope {
            ok: decision.ok,
            status: decision.status,
            retryable: decision.retryable,
            run_id,
            run_persisted,
            payload,
            error: decision.error,
            next_action: decision.next_action,
        },
        decision.exit_code,
    )
}

fn completed_decision() -> EnvelopeDecision {
    EnvelopeDecision {
        ok: true,
        status: CommandStatus::Completed,
        retryable: false,
        error: None,
        next_action: NextAction::Continue,
        exit_code: EXIT_OK,
    }
}

fn retry_command_for_step(command: CommandName, run_id_text: &str) -> String {
    if command == CommandName::InitRun {
        format!("agent-audit fetch-source --run-id {run_id_text}")
    } else {
        format!("agent-audit {} --run-id {run_id_text}", command.as_str())
    }
}

fn prerequisite_command_for_step(command: CommandName, run_id_text: &str) -> String {
    match command {
        CommandName::RunDependency | CommandName::PrepareSlither | CommandName::PrepareTooling => {
            format!("agent-audit fetch-source --run-id {run_id_text}")
        }
        _ => init_run_placeholder_command(),
    }
}

fn retry_command_for_error(command: Option<CommandName>, run_id_text: &str) -> String {
    match (command, run_id_text.is_empty()) {
        (Some(command), false) => {
            format!("agent-audit {} --run-id {run_id_text}", command.as_str())
        }
        _ => "agent-audit <same-command>".to_string(),
    }
}

fn init_run_placeholder_command() -> String {
    "agent-audit init-run --chain <chain> --address <address>".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::command::CommandName;
    use crate::models::envelope::AggregateMaterialsDetails;
    use crate::models::identity::RunId;
    use crate::models::path::WorkspaceRelPath;
    use crate::workspace::paths;
    use std::path::PathBuf;

    fn sample_payload(status: StepStatus) -> StepPayload {
        StepPayload {
            run_id: RunId::new("run-1").expect("valid run id"),
            run_dir: PathBuf::from("/tmp/run-1"),
            step: CommandName::AggregateMaterials,
            status,
            artifact_index: WorkspaceRelPath::new(paths::ARTIFACT_INDEX),
            init_run: None,
            fetch_source: None,
            prepare_slither: None,
            aggregate_materials: Some(AggregateMaterialsDetails {
                materials_manifest_path: WorkspaceRelPath::new(paths::MATERIALS_MANIFEST),
            }),
        }
    }

    #[test]
    fn step_envelope_marks_missing_prerequisite_as_precondition() {
        let (envelope, exit_code) = step_envelope(
            &RunId::new("run-1").expect("valid run id"),
            StepPayload {
                step: CommandName::PrepareTooling,
                ..sample_payload(StepStatus::SourceNotFetched)
            },
        );

        assert_eq!(exit_code, EXIT_PRECONDITION);
        assert_eq!(envelope.status, CommandStatus::PreconditionMissing);
        assert!(!envelope.ok);
    }

    #[test]
    fn step_envelope_marks_successful_payload_as_completed() {
        let (envelope, exit_code) = step_envelope(
            &RunId::new("run-1").expect("valid run id"),
            sample_payload(StepStatus::Executed),
        );

        assert_eq!(exit_code, EXIT_OK);
        assert_eq!(envelope.status, CommandStatus::Completed);
        assert!(envelope.ok);
    }

    #[test]
    fn step_envelope_marks_source_fetch_failure_as_retryable() {
        let (envelope, exit_code) = step_envelope(
            &RunId::new("run-1").expect("valid run id"),
            StepPayload {
                step: CommandName::FetchSource,
                ..sample_payload(StepStatus::SourceFetchFailed)
            },
        );

        assert_eq!(exit_code, EXIT_RETRYABLE);
        assert_eq!(envelope.status, CommandStatus::RetryableError);
        assert!(envelope.retryable);
    }
}
