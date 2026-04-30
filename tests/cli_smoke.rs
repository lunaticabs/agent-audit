use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use std::fs;
use tempfile::TempDir;

fn temp_project() -> TempDir {
    let dir = TempDir::new().expect("create temp dir");
    fs::write(
        dir.path().join(".env"),
        "AGENT_AUDIT_DEFAULT_CHAIN=eth\nAGENT_AUDIT_RUNS_DIR=runs\n",
    )
    .expect("write env");
    dir
}

#[test]
fn init_run_rejects_invalid_address_with_fatal_error() {
    let project = temp_project();

    let output = Command::cargo_bin("agent-audit")
        .expect("binary exists")
        .current_dir(project.path())
        .args(["init-run", "--address", "not-an-address"])
        .assert()
        .code(20)
        .stdout(predicate::str::contains("\"status\": \"fatal_error\""))
        .get_output()
        .stdout
        .clone();

    let payload: Value = serde_json::from_slice(&output).expect("valid json output");
    assert_eq!(payload["error"]["code"], "INVALID_ARGUMENT");
}

#[test]
fn aggregate_materials_missing_run_returns_precondition_error() {
    let project = temp_project();

    let output = Command::cargo_bin("agent-audit")
        .expect("binary exists")
        .current_dir(project.path())
        .args(["aggregate-materials", "--run-id", "missing-run"])
        .assert()
        .code(30)
        .stdout(predicate::str::contains(
            "\"status\": \"precondition_missing\"",
        ))
        .get_output()
        .stdout
        .clone();

    let payload: Value = serde_json::from_slice(&output).expect("valid json output");
    assert_eq!(payload["error"]["code"], "RUN_NOT_FOUND");
}
