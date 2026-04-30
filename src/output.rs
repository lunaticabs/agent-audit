use serde::Serialize;
use serde_json::Value;

use crate::models::envelope::{CommandEnvelope, EnvelopeError, NextAction};
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

pub fn step_envelope(command: &str, run_id: &str, payload: Value) -> (CommandEnvelope<Value>, i32) {
    let status = payload
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default();

    if matches!(status, "source_fetch_failed") {
        let retry_command = if command == "init-run" {
            format!("agent-audit fetch-source --run-id {run_id}")
        } else {
            format!("agent-audit {command} --run-id {run_id}")
        };
        return (
            CommandEnvelope {
                ok: false,
                status: "retryable_error".to_string(),
                retryable: true,
                run_id: run_id.to_string(),
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

    if matches!(status, "source_not_fetched" | "source_files_missing") {
        let prerequisite = match command {
            "run-dependency" | "prepare-slither" | "prepare-tooling" => "fetch-source",
            _ => "init-run",
        };
        let next_command = if prerequisite == "init-run" {
            "agent-audit init-run --chain <chain> --address <address>".to_string()
        } else {
            format!("agent-audit {prerequisite} --run-id {run_id}")
        };
        return (
            CommandEnvelope {
                ok: false,
                status: "precondition_missing".to_string(),
                retryable: false,
                run_id: run_id.to_string(),
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

    if status == "source_api_not_configured" {
        return (
            CommandEnvelope {
                ok: false,
                status: "fatal_error".to_string(),
                retryable: false,
                run_id: run_id.to_string(),
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
            status: "completed".to_string(),
            retryable: false,
            run_id: run_id.to_string(),
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
    run_id: &str,
    error: &crate::error::AppError,
) -> (CommandEnvelope<Value>, i32) {
    use crate::error::AppError;

    match error {
        AppError::RunNotFound(message) => (
            CommandEnvelope {
                ok: false,
                status: "precondition_missing".to_string(),
                retryable: false,
                run_id: run_id.to_string(),
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
                status: "fatal_error".to_string(),
                retryable: false,
                run_id: run_id.to_string(),
                run_persisted: !run_id.is_empty(),
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
                status: "fatal_error".to_string(),
                retryable: false,
                run_id: run_id.to_string(),
                run_persisted: !run_id.is_empty(),
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
            let same_command = match (command, run_id.is_empty()) {
                (Some(cmd), false) => {
                    format!("agent-audit {cmd} --run-id {run_id}")
                }
                _ => "agent-audit <same-command>".to_string(),
            };
            (
                CommandEnvelope {
                    ok: false,
                    status: "retryable_error".to_string(),
                    retryable: true,
                    run_id: run_id.to_string(),
                    run_persisted: !run_id.is_empty(),
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
