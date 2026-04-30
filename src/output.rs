use serde::Serialize;

use crate::models::envelope::{
    CommandEnvelope, CommandStatus, EnvelopeError, NextAction, StepPayload, StepStatus,
};
use crate::models::identity::RunId;
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

pub fn step_envelope(
    command: &str,
    run_id: &RunId,
    payload: StepPayload,
) -> (CommandEnvelope<StepPayload>, i32) {
    let run_id_text = run_id.as_str();
    if matches!(payload.status, StepStatus::SourceFetchFailed) {
        let retry_command = if command == "init-run" {
            format!("agent-audit fetch-source --run-id {run_id_text}")
        } else {
            format!("agent-audit {command} --run-id {run_id_text}")
        };
        return (
            CommandEnvelope {
                ok: false,
                status: CommandStatus::RetryableError,
                retryable: true,
                run_id: Some(run_id.clone()),
                run_persisted: true,
                payload: Some(payload),
                error: None,
                next_action: NextAction::RetrySameCommand {
                    command: retry_command,
                    retry_after_sec: 5,
                    max_retries: 3,
                },
            },
            EXIT_RETRYABLE,
        );
    }

    if matches!(
        payload.status,
        StepStatus::SourceNotFetched | StepStatus::SourceFilesMissing
    ) {
        let prerequisite = match command {
            "run-dependency" | "prepare-slither" | "prepare-tooling" => "fetch-source",
            _ => "init-run",
        };
        let next_command = if prerequisite == "init-run" {
            "agent-audit init-run --chain <chain> --address <address>".to_string()
        } else {
            format!("agent-audit {prerequisite} --run-id {run_id_text}")
        };
        return (
            CommandEnvelope {
                ok: false,
                status: CommandStatus::PreconditionMissing,
                retryable: false,
                run_id: Some(run_id.clone()),
                run_persisted: true,
                payload: Some(payload),
                error: None,
                next_action: NextAction::RunPrerequisite {
                    command: next_command,
                },
            },
            EXIT_PRECONDITION,
        );
    }

    if matches!(payload.status, StepStatus::SourceApiNotConfigured) {
        return (
            CommandEnvelope {
                ok: false,
                status: CommandStatus::FatalError,
                retryable: false,
                run_id: Some(run_id.clone()),
                run_persisted: true,
                payload: Some(payload),
                error: Some(EnvelopeError {
                    code: "SOURCE_API_NOT_CONFIGURED".to_string(),
                    message: "Configure AGENT_AUDIT_SOURCE_API_BASE before fetch-source."
                        .to_string(),
                }),
                next_action: NextAction::Stop {
                    command: Some("set AGENT_AUDIT_SOURCE_API_BASE in .env".to_string()),
                },
            },
            EXIT_FATAL,
        );
    }

    (
        CommandEnvelope {
            ok: true,
            status: CommandStatus::Completed,
            retryable: false,
            run_id: Some(run_id.clone()),
            run_persisted: true,
            payload: Some(payload),
            error: None,
            next_action: NextAction::Continue,
        },
        EXIT_OK,
    )
}

pub fn error_envelope(
    command: Option<&str>,
    run_id: Option<&RunId>,
    error: &crate::error::AppError,
) -> (CommandEnvelope<()>, i32) {
    use crate::error::AppError;
    let run_id_text = run_id.map(RunId::as_str).unwrap_or_default();
    let run_id_value = run_id.cloned();
    let run_persisted = run_id.is_some();

    match error {
        AppError::RunNotFound(message) => (
            CommandEnvelope {
                ok: false,
                status: CommandStatus::PreconditionMissing,
                retryable: false,
                run_id: run_id_value,
                run_persisted: false,
                payload: None,
                error: Some(EnvelopeError {
                    code: "RUN_NOT_FOUND".to_string(),
                    message: message.clone(),
                }),
                next_action: NextAction::RunPrerequisite {
                    command: "agent-audit init-run --chain <chain> --address <address>".to_string(),
                },
            },
            EXIT_PRECONDITION,
        ),
        AppError::InvalidAddress(message) => (
            CommandEnvelope {
                ok: false,
                status: CommandStatus::FatalError,
                retryable: false,
                run_id: run_id_value,
                run_persisted,
                payload: None,
                error: Some(EnvelopeError {
                    code: "INVALID_ARGUMENT".to_string(),
                    message: format!("invalid EVM address: {message}"),
                }),
                next_action: NextAction::Stop { command: None },
            },
            EXIT_FATAL,
        ),
        AppError::Message(message) => (
            CommandEnvelope {
                ok: false,
                status: CommandStatus::FatalError,
                retryable: false,
                run_id: run_id_value,
                run_persisted,
                payload: None,
                error: Some(EnvelopeError {
                    code: "INVALID_ARGUMENT".to_string(),
                    message: message.clone(),
                }),
                next_action: NextAction::Stop { command: None },
            },
            EXIT_FATAL,
        ),
        _ => {
            let same_command = match (command, run_id_text.is_empty()) {
                (Some(cmd), false) => {
                    format!("agent-audit {cmd} --run-id {run_id_text}")
                }
                _ => "agent-audit <same-command>".to_string(),
            };
            (
                CommandEnvelope {
                    ok: false,
                    status: CommandStatus::RetryableError,
                    retryable: true,
                    run_id: run_id_value,
                    run_persisted,
                    payload: None,
                    error: Some(EnvelopeError {
                        code: "UNHANDLED_EXCEPTION".to_string(),
                        message: error.to_string(),
                    }),
                    next_action: NextAction::RetrySameCommand {
                        command: same_command,
                        retry_after_sec: 5,
                        max_retries: 2,
                    },
                },
                EXIT_RETRYABLE,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::envelope::AggregateMaterialsDetails;
    use crate::models::identity::RunId;
    use crate::models::path::WorkspaceRelPath;
    use std::path::PathBuf;

    fn sample_payload(status: StepStatus) -> StepPayload {
        StepPayload {
            run_id: RunId::new("run-1").expect("valid run id"),
            run_dir: PathBuf::from("/tmp/run-1"),
            step: "aggregate-materials".to_string(),
            status,
            artifact_index: WorkspaceRelPath::new("artifacts/artifact_index.json"),
            init_run: None,
            fetch_source: None,
            prepare_slither: None,
            aggregate_materials: Some(AggregateMaterialsDetails {
                materials_manifest_path: WorkspaceRelPath::new("reports/materials_manifest.json"),
            }),
        }
    }

    #[test]
    fn step_envelope_marks_missing_prerequisite_as_precondition() {
        let (envelope, exit_code) = step_envelope(
            "prepare-tooling",
            &RunId::new("run-1").expect("valid run id"),
            sample_payload(StepStatus::SourceNotFetched),
        );

        assert_eq!(exit_code, EXIT_PRECONDITION);
        assert_eq!(envelope.status, CommandStatus::PreconditionMissing);
        assert!(!envelope.ok);
    }

    #[test]
    fn step_envelope_marks_successful_payload_as_completed() {
        let (envelope, exit_code) = step_envelope(
            "aggregate-materials",
            &RunId::new("run-1").expect("valid run id"),
            sample_payload(StepStatus::Executed),
        );

        assert_eq!(exit_code, EXIT_OK);
        assert_eq!(envelope.status, CommandStatus::Completed);
        assert!(envelope.ok);
    }
}
