use std::collections::BTreeMap;
use std::process::ExitCode;

use clap::Parser;
use k8s_openapi::api::batch::v1::Job;
use k8s_openapi::api::core::v1::{
    Container, EmptyDirVolumeSource, EnvFromSource, EnvVar, PodSpec, PodTemplateSpec,
    ResourceRequirements, SecretEnvSource, Volume, VolumeMount,
};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::api::PostParams;
use kube::{Api, Client};
use redis::aio::MultiplexedConnection;
use redis::streams::{StreamId, StreamReadOptions, StreamReadReply};
use redis::{AsyncCommands, RedisError};
use sha2::{Digest, Sha256};
use thiserror::Error;

const MANAGED_BY_LABEL: &str = "app.kubernetes.io/managed-by";
const MANAGED_BY_VALUE: &str = "agent-audit-dispatcher";
const TASK_ID_ANNOTATION: &str = "agent-audit/task-id";
const JOB_NAME_PREFIX: &str = "agent-audit-";

#[derive(Debug, Clone, Parser)]
#[command(
    name = "agent-audit-dispatcher",
    about = "Bridge Redis stream tasks into single-run agent-audit Jobs on k3s."
)]
struct Cli {
    #[arg(long, env = "DISPATCHER_NAMESPACE", default_value = "agent-audit")]
    namespace: String,
    #[arg(long, env = "DISPATCHER_REDIS_URL")]
    redis_url: String,
    #[arg(
        long,
        env = "DISPATCHER_REDIS_STREAM",
        default_value = "agent-audit:tasks"
    )]
    redis_stream: String,
    #[arg(
        long,
        env = "DISPATCHER_REDIS_GROUP",
        default_value = "agent-audit-dispatcher"
    )]
    redis_group: String,
    #[arg(long, env = "DISPATCHER_REDIS_CONSUMER", default_value = "dispatcher")]
    redis_consumer: String,
    #[arg(long, env = "DISPATCHER_IMAGE")]
    image: String,
    #[arg(long, env = "DISPATCHER_RUNNER_ENV_SECRET")]
    runner_env_secret: String,
    #[arg(long, env = "DISPATCHER_IMAGE_PULL_SECRET")]
    image_pull_secret: Option<String>,
    #[arg(
        long,
        env = "DISPATCHER_JOB_PULL_POLICY",
        default_value = "IfNotPresent"
    )]
    job_pull_policy: String,
    #[arg(long, env = "DISPATCHER_JOB_TTL_SECONDS", default_value_t = 86_400)]
    job_ttl_seconds: i32,
    #[arg(long, env = "DISPATCHER_REDIS_BLOCK_MS", default_value_t = 5_000)]
    redis_block_ms: usize,
    #[arg(long, env = "DISPATCHER_JOB_CPU_REQUEST")]
    job_cpu_request: Option<String>,
    #[arg(long, env = "DISPATCHER_JOB_CPU_LIMIT")]
    job_cpu_limit: Option<String>,
    #[arg(long, env = "DISPATCHER_JOB_MEMORY_REQUEST")]
    job_memory_request: Option<String>,
    #[arg(long, env = "DISPATCHER_JOB_MEMORY_LIMIT")]
    job_memory_limit: Option<String>,
    #[arg(long, env = "DISPATCHER_RUNS_VOLUME_SIZE_LIMIT")]
    runs_volume_size_limit: Option<String>,
}

#[derive(Debug, Clone)]
struct DispatcherConfig {
    namespace: String,
    redis_stream: String,
    redis_group: String,
    redis_consumer: String,
    default_image: String,
    runner_env_secret: String,
    image_pull_secret: Option<String>,
    job_pull_policy: String,
    job_ttl_seconds: i32,
    redis_block_ms: usize,
    job_cpu_request: Option<String>,
    job_cpu_limit: Option<String>,
    job_memory_request: Option<String>,
    job_memory_limit: Option<String>,
    runs_volume_size_limit: Option<String>,
}

impl From<Cli> for DispatcherConfig {
    fn from(value: Cli) -> Self {
        Self {
            namespace: value.namespace.trim().to_string(),
            redis_stream: value.redis_stream.trim().to_string(),
            redis_group: value.redis_group.trim().to_string(),
            redis_consumer: value.redis_consumer.trim().to_string(),
            default_image: value.image.trim().to_string(),
            runner_env_secret: value.runner_env_secret.trim().to_string(),
            image_pull_secret: trimmed_option(value.image_pull_secret),
            job_pull_policy: value.job_pull_policy.trim().to_string(),
            job_ttl_seconds: value.job_ttl_seconds,
            redis_block_ms: value.redis_block_ms,
            job_cpu_request: trimmed_option(value.job_cpu_request),
            job_cpu_limit: trimmed_option(value.job_cpu_limit),
            job_memory_request: trimmed_option(value.job_memory_request),
            job_memory_limit: trimmed_option(value.job_memory_limit),
            runs_volume_size_limit: trimmed_option(value.runs_volume_size_limit),
        }
    }
}

#[derive(Debug)]
struct Dispatcher {
    config: DispatcherConfig,
    jobs: Api<Job>,
    redis: MultiplexedConnection,
}

#[derive(Debug, Clone)]
struct StreamMessage {
    message_id: String,
    task: TaskMessage,
}

#[derive(Debug, Clone)]
struct TaskMessage {
    task_id: String,
    full_prompt: String,
    image: Option<String>,
}

#[derive(Debug, Clone)]
struct InvalidTaskMessage {
    task_id: Option<String>,
    message: String,
}

#[derive(Debug, Error)]
enum DispatcherError {
    #[error("{0}")]
    Redis(#[from] RedisError),
    #[error("{0}")]
    Kube(#[from] kube::Error),
}

type Result<T> = std::result::Result<T, DispatcherError>;

#[tokio::main]
async fn main() -> ExitCode {
    let _ = dotenvy::dotenv();

    let cli = Cli::parse();
    let config = DispatcherConfig::from(cli.clone());

    let client = match redis::Client::open(cli.redis_url.as_str()) {
        Ok(client) => client,
        Err(error) => {
            eprintln!("failed to create redis client: {error}");
            return ExitCode::from(1);
        }
    };

    let redis = match client.get_multiplexed_async_connection().await {
        Ok(connection) => connection,
        Err(error) => {
            eprintln!("failed to connect to redis: {error}");
            return ExitCode::from(1);
        }
    };

    let kube_client = match Client::try_default().await {
        Ok(client) => client,
        Err(error) => {
            eprintln!("failed to create kubernetes client: {error}");
            return ExitCode::from(1);
        }
    };

    let mut dispatcher = Dispatcher {
        jobs: Api::namespaced(kube_client, &config.namespace),
        config,
        redis,
    };

    match dispatcher.run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("dispatcher failed: {error}");
            ExitCode::from(1)
        }
    }
}

impl Dispatcher {
    async fn run(&mut self) -> Result<()> {
        self.ensure_consumer_group().await?;
        self.log_startup();

        loop {
            if let Some(message) = self.read_pending_message().await? {
                self.handle_message(message).await?;
                continue;
            }

            if let Some(message) = self.read_new_message().await? {
                self.handle_message(message).await?;
            }
        }
    }

    fn log_startup(&self) {
        eprintln!(
            "dispatcher started namespace={} stream={} group={} consumer={}",
            self.config.namespace,
            self.config.redis_stream,
            self.config.redis_group,
            self.config.redis_consumer
        );
    }

    async fn ensure_consumer_group(&mut self) -> Result<()> {
        let created = self
            .redis
            .xgroup_create_mkstream::<_, _, _, ()>(
                &self.config.redis_stream,
                &self.config.redis_group,
                "0",
            )
            .await;

        match created {
            Ok(()) => Ok(()),
            Err(error) if error.code() == Some("BUSYGROUP") => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    async fn read_pending_message(&mut self) -> Result<Option<StreamMessage>> {
        let options = StreamReadOptions::default()
            .group(&self.config.redis_group, &self.config.redis_consumer)
            .count(1);
        self.read_stream_message(&["0"], &options).await
    }

    async fn read_new_message(&mut self) -> Result<Option<StreamMessage>> {
        let options = StreamReadOptions::default()
            .group(&self.config.redis_group, &self.config.redis_consumer)
            .count(1)
            .block(self.config.redis_block_ms);
        self.read_stream_message(&[">"], &options).await
    }

    async fn read_stream_message(
        &mut self,
        ids: &[&str],
        options: &StreamReadOptions,
    ) -> Result<Option<StreamMessage>> {
        let reply = self
            .redis
            .xread_options::<_, _, Option<StreamReadReply>>(
                &[&self.config.redis_stream],
                ids,
                options,
            )
            .await?;

        let Some(reply) = reply else {
            return Ok(None);
        };
        let Some(entry) = first_stream_entry(reply) else {
            return Ok(None);
        };

        match TaskMessage::from_entry(&entry) {
            Ok(task) => Ok(Some(StreamMessage {
                message_id: entry.id,
                task,
            })),
            Err(invalid) => {
                self.ack_message(&entry.id).await?;
                match invalid.task_id.as_deref() {
                    Some(task_id) => eprintln!(
                        "discarded invalid stream message id={} task_id={} error={}",
                        entry.id, task_id, invalid.message
                    ),
                    None => eprintln!(
                        "discarded invalid stream message id={} error={}",
                        entry.id, invalid.message
                    ),
                }
                Ok(None)
            }
        }
    }

    async fn handle_message(&mut self, message: StreamMessage) -> Result<()> {
        let job_name = job_name_for_task(&message.task.task_id);

        if self.jobs.get_opt(&job_name).await?.is_none() {
            let job = build_job(&self.config, &message.task, &job_name);
            self.jobs.create(&PostParams::default(), &job).await?;
            eprintln!(
                "created job task_id={} job_name={} image={} message_id={}",
                message.task.task_id,
                job_name,
                message
                    .task
                    .image
                    .as_deref()
                    .unwrap_or(self.config.default_image.as_str()),
                message.message_id
            );
        } else {
            eprintln!(
                "job already exists task_id={} job_name={} message_id={}",
                message.task.task_id, job_name, message.message_id
            );
        }

        self.ack_message(&message.message_id).await?;
        Ok(())
    }

    async fn ack_message(&mut self, message_id: &str) -> Result<()> {
        self.redis
            .xack::<_, _, _, usize>(
                &self.config.redis_stream,
                &self.config.redis_group,
                &[message_id],
            )
            .await?;
        Ok(())
    }
}

impl TaskMessage {
    fn from_entry(entry: &StreamId) -> std::result::Result<Self, InvalidTaskMessage> {
        let task_id = entry
            .get::<String>("task_id")
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let full_prompt = entry.get::<String>("full_prompt");

        let Some(task_id) = task_id else {
            return Err(InvalidTaskMessage {
                task_id: None,
                message: "task_id is required".to_string(),
            });
        };

        let Some(full_prompt) = full_prompt else {
            return Err(InvalidTaskMessage {
                task_id: Some(task_id),
                message: "full_prompt is required".to_string(),
            });
        };

        if full_prompt.trim().is_empty() {
            return Err(InvalidTaskMessage {
                task_id: Some(task_id),
                message: "full_prompt must not be blank".to_string(),
            });
        }

        Ok(Self {
            task_id,
            full_prompt,
            image: trimmed_option(entry.get("image")),
        })
    }
}

fn build_job(config: &DispatcherConfig, task: &TaskMessage, job_name: &str) -> Job {
    let mut labels = BTreeMap::new();
    labels.insert(MANAGED_BY_LABEL.to_string(), MANAGED_BY_VALUE.to_string());
    labels.insert(
        "app.kubernetes.io/name".to_string(),
        "agent-audit".to_string(),
    );
    labels.insert(
        "app.kubernetes.io/component".to_string(),
        "runner".to_string(),
    );
    labels.insert(
        "agent-audit/task-id-sanitized".to_string(),
        sanitize_task_id(&task.task_id),
    );

    let mut annotations = BTreeMap::new();
    annotations.insert(TASK_ID_ANNOTATION.to_string(), task.task_id.clone());

    let env_from = vec![EnvFromSource {
        secret_ref: Some(SecretEnvSource {
            name: config.runner_env_secret.clone(),
            optional: Some(false),
        }),
        ..Default::default()
    }];

    let env = vec![
        EnvVar {
            name: "FULL_PROMPT".to_string(),
            value: Some(task.full_prompt.clone()),
            ..Default::default()
        },
        EnvVar {
            name: "TASK_ID".to_string(),
            value: Some(task.task_id.clone()),
            ..Default::default()
        },
    ];

    let resources = build_resources(config);

    let volume = Volume {
        name: "runs".to_string(),
        empty_dir: Some(EmptyDirVolumeSource {
            size_limit: config
                .runs_volume_size_limit
                .as_ref()
                .map(|value| Quantity(value.clone())),
            ..Default::default()
        }),
        ..Default::default()
    };

    let image = task
        .image
        .as_deref()
        .unwrap_or(config.default_image.as_str())
        .to_string();

    Job {
        metadata: ObjectMeta {
            name: Some(job_name.to_string()),
            namespace: Some(config.namespace.clone()),
            labels: Some(labels.clone()),
            annotations: Some(annotations.clone()),
            ..Default::default()
        },
        spec: Some(k8s_openapi::api::batch::v1::JobSpec {
            backoff_limit: Some(0),
            ttl_seconds_after_finished: Some(config.job_ttl_seconds),
            template: PodTemplateSpec {
                metadata: Some(ObjectMeta {
                    labels: Some(labels),
                    annotations: Some(annotations),
                    ..Default::default()
                }),
                spec: Some(PodSpec {
                    restart_policy: Some("Never".to_string()),
                    image_pull_secrets: config.image_pull_secret.as_ref().map(|name| {
                        vec![k8s_openapi::api::core::v1::LocalObjectReference {
                            name: name.clone(),
                        }]
                    }),
                    volumes: Some(vec![volume]),
                    containers: vec![Container {
                        name: "agent-audit".to_string(),
                        image: Some(image),
                        image_pull_policy: Some(config.job_pull_policy.clone()),
                        env_from: Some(env_from),
                        env: Some(env),
                        resources,
                        volume_mounts: Some(vec![VolumeMount {
                            name: "runs".to_string(),
                            mount_path: "/opt/agent-audit/runs".to_string(),
                            ..Default::default()
                        }]),
                        ..Default::default()
                    }],
                    ..Default::default()
                }),
            },
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn build_resources(config: &DispatcherConfig) -> Option<ResourceRequirements> {
    let mut requests = BTreeMap::new();
    let mut limits = BTreeMap::new();

    if let Some(cpu) = config.job_cpu_request.as_deref() {
        requests.insert("cpu".to_string(), Quantity(cpu.to_string()));
    }
    if let Some(memory) = config.job_memory_request.as_deref() {
        requests.insert("memory".to_string(), Quantity(memory.to_string()));
    }
    if let Some(cpu) = config.job_cpu_limit.as_deref() {
        limits.insert("cpu".to_string(), Quantity(cpu.to_string()));
    }
    if let Some(memory) = config.job_memory_limit.as_deref() {
        limits.insert("memory".to_string(), Quantity(memory.to_string()));
    }

    if requests.is_empty() && limits.is_empty() {
        return None;
    }

    Some(ResourceRequirements {
        requests: (!requests.is_empty()).then_some(requests),
        limits: (!limits.is_empty()).then_some(limits),
        ..Default::default()
    })
}

fn first_stream_entry(reply: StreamReadReply) -> Option<StreamId> {
    let mut keys = reply.keys.into_iter();
    let key = keys.next()?;
    key.ids.into_iter().next()
}

fn job_name_for_task(task_id: &str) -> String {
    let sanitized = sanitize_task_id(task_id);
    let max_len = 63usize;
    let suffix = short_hash(task_id);

    if JOB_NAME_PREFIX.len() + sanitized.len() <= max_len {
        return format!("{JOB_NAME_PREFIX}{sanitized}");
    }

    let reserved = JOB_NAME_PREFIX.len() + 1 + suffix.len();
    let body_limit = max_len.saturating_sub(reserved).max(1);
    let truncated = sanitized[..sanitized.len().min(body_limit)]
        .trim_end_matches('-')
        .to_string();
    let truncated = if truncated.is_empty() {
        "task".to_string()
    } else {
        truncated
    };
    format!("{JOB_NAME_PREFIX}{truncated}-{suffix}")
}

fn sanitize_task_id(task_id: &str) -> String {
    let mut output = String::with_capacity(task_id.len());
    let mut last_dash = false;

    for ch in task_id.chars() {
        let normalized = ch.to_ascii_lowercase();
        if normalized.is_ascii_alphanumeric() {
            output.push(normalized);
            last_dash = false;
        } else if !last_dash {
            output.push('-');
            last_dash = true;
        }
    }

    let output = output.trim_matches('-').to_string();
    if output.is_empty() {
        "task".to_string()
    } else {
        output
    }
}

fn short_hash(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    digest[..4]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

fn trimmed_option(value: Option<String>) -> Option<String> {
    value.and_then(|raw| {
        let trimmed = raw.trim().to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use redis::Value;
    use std::collections::HashMap;

    fn entry(fields: &[(&str, &str)]) -> StreamId {
        let mut map = HashMap::new();
        for (key, value) in fields {
            map.insert(
                (*key).to_string(),
                Value::BulkString(value.as_bytes().to_vec()),
            );
        }
        StreamId {
            id: "1-0".to_string(),
            map,
            milliseconds_elapsed_from_delivery: None,
            delivered_count: None,
        }
    }

    #[test]
    fn task_message_requires_task_id() {
        let invalid = TaskMessage::from_entry(&entry(&[("full_prompt", "audit this")]))
            .expect_err("missing task_id should fail");
        assert_eq!(invalid.message, "task_id is required");
    }

    #[test]
    fn task_message_requires_non_blank_prompt() {
        let invalid =
            TaskMessage::from_entry(&entry(&[("task_id", "audit-1"), ("full_prompt", "   ")]))
                .expect_err("blank full_prompt should fail");
        assert_eq!(invalid.task_id.as_deref(), Some("audit-1"));
        assert_eq!(invalid.message, "full_prompt must not be blank");
    }

    #[test]
    fn job_name_is_stable_and_dns_safe() {
        let job_name = job_name_for_task("Audit 2026/05/05:Mainnet");
        assert!(job_name.starts_with("agent-audit-"));
        assert!(job_name.len() <= 63);
        assert!(
            job_name
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
        );
    }

    #[test]
    fn long_job_name_gets_hash_suffix() {
        let task_id = "x".repeat(128);
        let job_name = job_name_for_task(&task_id);
        assert!(job_name.len() <= 63);
        assert!(job_name.contains('-'));
    }
}
