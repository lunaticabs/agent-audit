use serde::Serialize;
use serde_json::{Value, json};

pub const EXIT_OK: i32 = 0;
pub const EXIT_RETRYABLE: i32 = 10;
pub const EXIT_FATAL: i32 = 20;
pub const EXIT_PRECONDITION: i32 = 30;

pub fn print_json<T: Serialize>(value: &T) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).expect("serialize json envelope")
    );
}

pub fn step_envelope(command: &str, run_id: &str, payload: &Value) -> (Value, i32) {
    let status = payload
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default();

    if matches!(status, "source_fetch_failed") {
        return (
            json!({
                "ok": false,
                "status": "retryable_error",
                "retryable": true,
                "run_id": run_id,
                "run_persisted": true,
                "payload": payload,
                "next_action": {
                    "type": "retry_same_command",
                    "command": format!("agent-audit {command} --run-id {run_id}"),
                    "retry_after_sec": 5,
                    "max_retries": 3,
                },
            }),
            EXIT_RETRYABLE,
        );
    }

    if matches!(status, "source_not_fetched" | "source_files_missing") {
        let prerequisite = match command {
            "run-dependency" | "prepare-slither" => "fetch-source",
            _ => "init-run",
        };
        let next_command = if prerequisite == "init-run" {
            "agent-audit init-run --chain <chain> --address <address>".to_string()
        } else {
            format!("agent-audit {prerequisite} --run-id {run_id}")
        };
        return (
            json!({
                "ok": false,
                "status": "precondition_missing",
                "retryable": false,
                "run_id": run_id,
                "run_persisted": true,
                "payload": payload,
                "next_action": {
                    "type": "run_prerequisite",
                    "command": next_command,
                },
            }),
            EXIT_PRECONDITION,
        );
    }

    if status == "source_api_not_configured" {
        return (
            json!({
                "ok": false,
                "status": "fatal_error",
                "retryable": false,
                "run_id": run_id,
                "run_persisted": true,
                "payload": payload,
                "error": {
                    "code": "SOURCE_API_NOT_CONFIGURED",
                    "message": "Configure AGENT_AUDIT_SOURCE_API_BASE before fetch-source.",
                },
                "next_action": {
                    "type": "stop",
                    "command": "set AGENT_AUDIT_SOURCE_API_BASE in .env",
                },
            }),
            EXIT_FATAL,
        );
    }

    (
        json!({
            "ok": true,
            "status": "completed",
            "retryable": false,
            "run_id": run_id,
            "run_persisted": true,
            "payload": payload,
            "next_action": {
                "type": "continue",
            },
        }),
        EXIT_OK,
    )
}

pub fn error_envelope(command: Option<&str>, run_id: &str, error: &super::errors::AppError) -> (Value, i32) {
    use super::errors::AppError;

    match error {
        AppError::RunNotFound(message) => (
            json!({
                "ok": false,
                "status": "precondition_missing",
                "retryable": false,
                "run_id": run_id,
                "run_persisted": false,
                "error": {
                    "code": "RUN_NOT_FOUND",
                    "message": message,
                },
                "next_action": {
                    "type": "run_prerequisite",
                    "command": "agent-audit init-run --chain <chain> --address <address>",
                },
            }),
            EXIT_PRECONDITION,
        ),
        AppError::InvalidAddress(message) => (
            json!({
                "ok": false,
                "status": "fatal_error",
                "retryable": false,
                "run_id": run_id,
                "run_persisted": !run_id.is_empty(),
                "error": {
                    "code": "INVALID_ARGUMENT",
                    "message": format!("invalid EVM address: {message}"),
                },
                "next_action": {
                    "type": "stop",
                },
            }),
            EXIT_FATAL,
        ),
        AppError::Message(message) => (
            json!({
                "ok": false,
                "status": "fatal_error",
                "retryable": false,
                "run_id": run_id,
                "run_persisted": !run_id.is_empty(),
                "error": {
                    "code": "INVALID_ARGUMENT",
                    "message": message,
                },
                "next_action": {
                    "type": "stop",
                },
            }),
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
                json!({
                    "ok": false,
                    "status": "retryable_error",
                    "retryable": true,
                    "run_id": run_id,
                    "run_persisted": !run_id.is_empty(),
                    "error": {
                        "code": "UNHANDLED_EXCEPTION",
                        "message": error.to_string(),
                    },
                    "next_action": {
                        "type": "retry_same_command",
                        "command": same_command,
                        "retry_after_sec": 5,
                        "max_retries": 2,
                    },
                }),
                EXIT_RETRYABLE,
            )
        }
    }
}
