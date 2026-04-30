use clap::{Parser, Subcommand};
use regex::Regex;
use serde_json::{Value, json};

use super::config::AppConfig;
use super::envelope::{EXIT_OK, error_envelope, print_json, step_envelope};
use super::errors::{AppError, AppResult};
use super::mongo_store::sync_run_to_mongo;
use super::pipeline::AuditPipelineService;
use super::workspace::{RunRequestContext, RunWorkspace, load_request_context};

#[derive(Parser)]
#[command(name = "agent-audit", about = "Run the local smart contract audit pipeline scaffold.")]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    InitRun {
        #[arg(long)]
        address: String,
        #[arg(long)]
        chain: Option<String>,
    },
    FetchSource {
        #[arg(long = "run-id")]
        run_id: String,
    },
    RunDependency {
        #[arg(long = "run-id")]
        run_id: String,
    },
    PrepareSlither {
        #[arg(long = "run-id")]
        run_id: String,
    },
    AggregateMaterials {
        #[arg(long = "run-id")]
        run_id: String,
    },
    SyncRun {
        #[arg(long = "run-id")]
        run_id: String,
    },
}

pub fn run() -> i32 {
    let args = Args::parse();
    let config = match AppConfig::load(None) {
        Ok(config) => config,
        Err(error) => {
            let (envelope, code) = error_envelope(None, "", &error);
            print_json(&envelope);
            return code;
        }
    };

    let result = match args.command {
        Command::InitRun { address, chain } => cmd_init_run(&config, &address, chain.as_deref()),
        Command::FetchSource { ref run_id } => cmd_step(&config, "fetch-source", run_id, cmd_fetch_source),
        Command::RunDependency { ref run_id } => cmd_step(&config, "run-dependency", run_id, cmd_run_dependency),
        Command::PrepareSlither { ref run_id } => cmd_step(&config, "prepare-slither", run_id, cmd_prepare_slither),
        Command::AggregateMaterials { ref run_id } => cmd_step(&config, "aggregate-materials", run_id, cmd_aggregate_materials),
        Command::SyncRun { ref run_id } => cmd_sync_run(&config, run_id),
    };

    match result {
        Ok(code) => code,
        Err((command, run_id, error)) => {
            let (envelope, code) = error_envelope(command.as_deref(), &run_id, &error);
            print_json(&envelope);
            code
        }
    }
}

fn cmd_init_run(config: &AppConfig, address: &str, chain: Option<&str>) -> Result<i32, (Option<String>, String, AppError)> {
    let address = validate_address(address).map_err(|error| (Some("init-run".to_string()), String::new(), error))?;
    let chain = chain.unwrap_or(&config.default_chain);
    let workspace = RunWorkspace::create(&config.project_root, &config.runs_dir, &address, chain)
        .map_err(|error| (Some("init-run".to_string()), String::new(), error))?;
    workspace
        .write_json(
            "input/request.json",
            &json!({
                "address": address,
                "chain": chain,
            }),
        )
        .map_err(|error| (Some("init-run".to_string()), workspace.run_id.clone(), error))?;

    let envelope = json!({
        "ok": true,
        "status": "completed",
        "retryable": false,
        "run_id": workspace.run_id,
        "run_persisted": true,
        "payload": {
            "run_id": workspace.run_id,
            "run_dir": workspace.root.to_string_lossy(),
            "address": address,
            "chain": chain,
        },
        "next_action": {
            "type": "continue",
            "command": format!("agent-audit fetch-source --run-id {}", workspace.run_id),
        },
    });
    print_json(&envelope);
    Ok(EXIT_OK)
}

type StepFn = fn(&AppConfig, &str) -> AppResult<(RunWorkspace, Value)>;

fn cmd_step(
    config: &AppConfig,
    command: &str,
    run_id: &str,
    step_fn: StepFn,
) -> Result<i32, (Option<String>, String, AppError)> {
    let workspace = RunWorkspace::load(&config.project_root, &config.runs_dir, run_id)
        .map_err(|error| (Some(command.to_string()), run_id.to_string(), error))?;
    let _guard = workspace.lock().map_err(|error| (Some(command.to_string()), run_id.to_string(), error))?;
    let (_, payload) = step_fn(config, run_id)
        .map_err(|error| (Some(command.to_string()), run_id.to_string(), error))?;
    let (envelope, code) = step_envelope(command, run_id, &payload);
    print_json(&envelope);
    Ok(code)
}

fn cmd_fetch_source(config: &AppConfig, run_id: &str) -> AppResult<(RunWorkspace, Value)> {
    let (workspace, context, mut pipeline) = load_pipeline(config, run_id)?;
    let status = pipeline.fetch_contract_source(&context.address, &context.chain)?;
    let slither_status = if status == "source_fetched" {
        pipeline.prepare_slither_project(&context.address, &context.chain)?
    } else {
        "not_prepared".to_string()
    };
    let payload = step_payload(
        &workspace,
        "fetch-source",
        &status,
        &pipeline.write_artifact_index()?,
        Some(json!({
            "slither_project_status": slither_status,
            "slither_build_manifest_path": if slither_status == "prepared" { "slither_project/build_manifest.json" } else { "" },
        })),
    );
    workspace.write_json("logs/fetch_source_result.json", &payload)?;
    Ok((workspace, payload))
}

fn cmd_run_dependency(config: &AppConfig, run_id: &str) -> AppResult<(RunWorkspace, Value)> {
    let (workspace, context, mut pipeline) = load_pipeline(config, run_id)?;
    let status = pipeline.run_dependency_analysis(&context.address, &context.chain)?;
    let payload = step_payload(&workspace, "run-dependency", &status, &pipeline.write_artifact_index()?, None);
    workspace.write_json("logs/run_dependency_result.json", &payload)?;
    Ok((workspace, payload))
}

fn cmd_prepare_slither(config: &AppConfig, run_id: &str) -> AppResult<(RunWorkspace, Value)> {
    let (workspace, context, mut pipeline) = load_pipeline(config, run_id)?;
    let status = pipeline.prepare_slither_project(&context.address, &context.chain)?;
    let payload = step_payload(
        &workspace,
        "prepare-slither",
        &status,
        &pipeline.write_artifact_index()?,
        Some(json!({
            "slither_build_manifest_path": "slither_project/build_manifest.json",
            "slither_project_root": "slither_project",
        })),
    );
    workspace.write_json("logs/prepare_slither_result.json", &payload)?;
    Ok((workspace, payload))
}

fn cmd_aggregate_materials(config: &AppConfig, run_id: &str) -> AppResult<(RunWorkspace, Value)> {
    let (workspace, context, mut pipeline) = load_pipeline(config, run_id)?;
    let manifest_path = pipeline.aggregate_materials(&context.address, &context.chain)?;
    let payload = step_payload(
        &workspace,
        "aggregate-materials",
        "executed",
        &pipeline.write_artifact_index()?,
        Some(json!({
            "materials_manifest_path": manifest_path,
        })),
    );
    workspace.write_json("logs/aggregate_materials_result.json", &payload)?;
    Ok((workspace, payload))
}

fn cmd_sync_run(config: &AppConfig, run_id: &str) -> Result<i32, (Option<String>, String, AppError)> {
    let workspace = RunWorkspace::load(&config.project_root, &config.runs_dir, run_id)
        .map_err(|error| (Some("sync-run".to_string()), run_id.to_string(), error))?;
    let sync = sync_run_to_mongo(config, &workspace)
        .map_err(|error| (Some("sync-run".to_string()), run_id.to_string(), error))?;
    print_json(&json!({
        "ok": true,
        "status": "completed",
        "retryable": false,
        "run_id": sync.run_id,
        "run_persisted": true,
        "mongo_sync": {
            "status": "completed",
            "file_count": sync.file_count,
            "total_size_bytes": sync.total_size_bytes,
            "upserted_file_records": sync.upserted_file_records,
        },
        "next_action": {
            "type": "continue",
        },
    }));
    Ok(EXIT_OK)
}

fn validate_address(address: &str) -> AppResult<String> {
    let pattern = Regex::new(r"^0x[a-fA-F0-9]{40}$").expect("valid address regex");
    if !pattern.is_match(address) {
        return Err(AppError::InvalidAddress(address.to_string()));
    }
    Ok(address.to_lowercase())
}

fn load_pipeline(config: &AppConfig, run_id: &str) -> AppResult<(RunWorkspace, RunRequestContext, AuditPipelineService)> {
    let workspace = RunWorkspace::load(&config.project_root, &config.runs_dir, run_id)?;
    let context = load_request_context(&workspace)?;
    let pipeline = AuditPipelineService::new(config.clone(), workspace.clone());
    Ok((workspace, context, pipeline))
}

fn step_payload(
    workspace: &RunWorkspace,
    step: &str,
    status: &str,
    artifact_index: &str,
    extra: Option<Value>,
) -> Value {
    let mut payload = json!({
        "run_id": workspace.run_id,
        "run_dir": workspace.root.to_string_lossy(),
        "step": step,
        "status": status,
        "artifact_index": artifact_index,
    });
    if let Some(extra) = extra {
        if let (Some(payload_obj), Some(extra_obj)) = (payload.as_object_mut(), extra.as_object()) {
            for (key, value) in extra_obj {
                payload_obj.insert(key.clone(), value.clone());
            }
        }
    }
    payload
}
