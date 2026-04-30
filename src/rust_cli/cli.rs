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
#[command(
    name = "agent-audit",
    about = "Run the local smart contract audit pipeline scaffold."
)]
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
    PrepareTooling {
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
        Command::FetchSource { ref run_id } => {
            cmd_step(&config, "fetch-source", run_id, cmd_fetch_source)
        }
        Command::RunDependency { ref run_id } => {
            cmd_step(&config, "run-dependency", run_id, cmd_run_dependency)
        }
        Command::PrepareSlither { ref run_id } => {
            cmd_step(&config, "prepare-slither", run_id, cmd_prepare_slither)
        }
        Command::PrepareTooling { ref run_id } => {
            cmd_step(&config, "prepare-tooling", run_id, cmd_prepare_tooling)
        }
        Command::AggregateMaterials { ref run_id } => cmd_step(
            &config,
            "aggregate-materials",
            run_id,
            cmd_aggregate_materials,
        ),
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

fn cmd_init_run(
    config: &AppConfig,
    address: &str,
    chain: Option<&str>,
) -> Result<i32, (Option<String>, String, AppError)> {
    let address = validate_address(address)
        .map_err(|error| (Some("init-run".to_string()), String::new(), error))?;
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
        .map_err(|error| {
            (
                Some("init-run".to_string()),
                workspace.run_id.clone(),
                error,
            )
        })?;
    let payload = run_full_workspace_prepare(config, &workspace).map_err(|error| {
        (
            Some("init-run".to_string()),
            workspace.run_id.clone(),
            error,
        )
    })?;
    let (envelope, code) = step_envelope("init-run", &workspace.run_id, &payload);
    print_json(&envelope);
    Ok(code)
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
    let _guard = workspace
        .lock()
        .map_err(|error| (Some(command.to_string()), run_id.to_string(), error))?;
    let (_, payload) = step_fn(config, run_id)
        .map_err(|error| (Some(command.to_string()), run_id.to_string(), error))?;
    let (envelope, code) = step_envelope(command, run_id, &payload);
    print_json(&envelope);
    Ok(code)
}

fn cmd_fetch_source(config: &AppConfig, run_id: &str) -> AppResult<(RunWorkspace, Value)> {
    let (workspace, context, mut pipeline) = load_pipeline(config, run_id)?;
    let status = pipeline.fetch_contract_source(&context.address, &context.chain)?;
    let tooling_status = pipeline.prepare_tooling_workspaces(&context.address, &context.chain)?;
    let payload = step_payload(
        &workspace,
        "fetch-source",
        &status,
        &pipeline.write_artifact_index()?,
        Some(json!({
            "tooling_status": tooling_status,
            "tooling_manifest_path": "artifacts/tooling_manifest.json",
            "slither_build_manifest_path": "slither_project/build_manifest.json",
            "foundry_build_manifest_path": "foundry_project/build_manifest.json",
            "echidna_build_manifest_path": "echidna_project/build_manifest.json",
        })),
    );
    workspace.write_json("logs/fetch_source_result.json", &payload)?;
    Ok((workspace, payload))
}

fn cmd_run_dependency(config: &AppConfig, run_id: &str) -> AppResult<(RunWorkspace, Value)> {
    let (workspace, context, mut pipeline) = load_pipeline(config, run_id)?;
    let status = pipeline.run_dependency_analysis(&context.address, &context.chain)?;
    let payload = step_payload(
        &workspace,
        "run-dependency",
        &status,
        &pipeline.write_artifact_index()?,
        None,
    );
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

fn cmd_prepare_tooling(config: &AppConfig, run_id: &str) -> AppResult<(RunWorkspace, Value)> {
    let (workspace, context, mut pipeline) = load_pipeline(config, run_id)?;
    let status = pipeline.prepare_tooling_workspaces(&context.address, &context.chain)?;
    let payload = step_payload(
        &workspace,
        "prepare-tooling",
        &status,
        &pipeline.write_artifact_index()?,
        Some(json!({
            "tooling_manifest_path": "artifacts/tooling_manifest.json",
            "slither_build_manifest_path": "slither_project/build_manifest.json",
            "foundry_build_manifest_path": "foundry_project/build_manifest.json",
            "echidna_build_manifest_path": "echidna_project/build_manifest.json",
        })),
    );
    workspace.write_json("logs/prepare_tooling_result.json", &payload)?;
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

fn cmd_sync_run(
    config: &AppConfig,
    run_id: &str,
) -> Result<i32, (Option<String>, String, AppError)> {
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

fn load_pipeline(
    config: &AppConfig,
    run_id: &str,
) -> AppResult<(RunWorkspace, RunRequestContext, AuditPipelineService)> {
    let workspace = RunWorkspace::load(&config.project_root, &config.runs_dir, run_id)?;
    let context = load_request_context(&workspace)?;
    let pipeline = AuditPipelineService::new(config.clone(), workspace.clone());
    Ok((workspace, context, pipeline))
}

fn run_full_workspace_prepare(config: &AppConfig, workspace: &RunWorkspace) -> AppResult<Value> {
    let context = load_request_context(workspace)?;
    let _guard = workspace.lock()?;
    let mut pipeline = AuditPipelineService::new(config.clone(), workspace.clone());

    let source_status = pipeline.fetch_contract_source(&context.address, &context.chain)?;
    let dependency_status = pipeline.run_dependency_analysis(&context.address, &context.chain)?;
    let tooling_status = pipeline.prepare_tooling_workspaces(&context.address, &context.chain)?;
    let materials_manifest_path = pipeline.aggregate_materials(&context.address, &context.chain)?;
    let status = full_prepare_status(&source_status, &dependency_status, &tooling_status);
    let payload = step_payload(
        workspace,
        "init-run",
        &status,
        &pipeline.write_artifact_index()?,
        Some(json!({
            "address": context.address,
            "chain": context.chain,
            "source_fetch_status": source_status,
            "dependency_analysis_status": dependency_status,
            "tooling_status": tooling_status,
            "tooling_manifest_path": "artifacts/tooling_manifest.json",
            "materials_manifest_path": materials_manifest_path,
            "slither_build_manifest_path": "slither_project/build_manifest.json",
            "foundry_build_manifest_path": "foundry_project/build_manifest.json",
            "echidna_build_manifest_path": "echidna_project/build_manifest.json",
        })),
    );
    workspace.write_json("logs/init_run_result.json", &payload)?;
    Ok(payload)
}

fn full_prepare_status(
    source_status: &str,
    dependency_status: &str,
    tooling_status: &str,
) -> String {
    if source_status != "source_fetched" {
        return source_status.to_string();
    }
    if dependency_status != "executed" {
        return dependency_status.to_string();
    }
    if tooling_status != "prepared" {
        return tooling_status.to_string();
    }
    "prepared".to_string()
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
