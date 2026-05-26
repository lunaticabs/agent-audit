use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
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

#[test]
fn run_dependency_writes_chain_artifacts_when_rpc_is_configured() {
    let project = temp_project();
    let rpc = spawn_mock_rpc_server();
    fs::write(
        project.path().join(".env"),
        format!(
            "AGENT_AUDIT_DEFAULT_CHAIN=eth\nAGENT_AUDIT_RUNS_DIR=runs\nAGENT_AUDIT_RPC_URL={}\n",
            rpc
        ),
    )
    .expect("write env");

    let run_dir = project.path().join("runs/run-1");
    fs::create_dir_all(run_dir.join("input")).expect("input dir");
    fs::create_dir_all(run_dir.join("artifacts")).expect("artifacts dir");
    fs::create_dir_all(run_dir.join("reports")).expect("reports dir");
    fs::create_dir_all(run_dir.join("logs")).expect("logs dir");

    fs::write(
        run_dir.join("input/request.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "address": "0x1234567890abcdef1234567890abcdef12345678",
            "chain": "eth"
        }))
        .expect("request json"),
    )
    .expect("write request");
    fs::write(
        run_dir.join("artifacts/source_bundle.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "target": {
                "address": "0x1234567890abcdef1234567890abcdef12345678",
                "chain": "eth"
            },
            "status": "source_fetched",
            "contract": {
                "name": "TestTarget",
                "proxy": false
            },
            "abi": [
                {
                    "type": "function",
                    "name": "rebalanceAndBorrow",
                    "inputs": []
                }
            ]
        }))
        .expect("source bundle json"),
    )
    .expect("write source bundle");

    Command::cargo_bin("agent-audit")
        .expect("binary exists")
        .current_dir(project.path())
        .args(["run-dependency", "--run-id", "run-1"])
        .assert()
        .code(0)
        .stdout(predicate::str::contains("\"status\": \"completed\""));

    let chain_summary: Value = serde_json::from_str(
        &fs::read_to_string(run_dir.join("artifacts/dependency_chain_checks.json"))
            .expect("read chain summary"),
    )
    .expect("valid chain summary");
    assert_eq!(chain_summary["status"], "executed");

    let proxy_checks = run_dir.join("artifacts/proxy_checks.json");
    let oracle_checks = run_dir.join("artifacts/oracle_checks.json");
    let flash_surface = run_dir.join("artifacts/flash_loan_surface.json");
    assert!(proxy_checks.exists());
    assert!(oracle_checks.exists());
    assert!(flash_surface.exists());
}

fn spawn_mock_rpc_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock rpc");
    let address = listener.local_addr().expect("local addr");
    thread::spawn(move || {
        for stream in listener.incoming().take(16) {
            let mut stream = match stream {
                Ok(stream) => stream,
                Err(_) => continue,
            };
            let mut buffer = [0u8; 8192];
            let bytes = match stream.read(&mut buffer) {
                Ok(bytes) => bytes,
                Err(_) => continue,
            };
            let request = String::from_utf8_lossy(&buffer[..bytes]);
            let body = request.split("\r\n\r\n").nth(1).unwrap_or_default();
            let payload: Value = serde_json::from_str(body).unwrap_or(Value::Null);
            let method = payload
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let result = match method {
                "eth_getBlockByNumber" => serde_json::json!({
                    "number": "0x10",
                    "timestamp": "0x64"
                }),
                "eth_getStorageAt" => Value::String(
                    "0x0000000000000000000000000000000000000000000000000000000000000000"
                        .to_string(),
                ),
                "eth_call" => Value::String("0x".to_string()),
                _ => Value::Null,
            };
            let response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": payload.get("id").cloned().unwrap_or(Value::from(1)),
                "result": result
            })
            .to_string();
            let http = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                response.len(),
                response
            );
            let _ = stream.write_all(http.as_bytes());
            let _ = stream.flush();
        }
    });
    format!("http://{}", address)
}
