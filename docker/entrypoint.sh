#!/usr/bin/env bash
set -uo pipefail

PROJECT_ROOT="${AGENT_AUDIT_PROJECT_ROOT:-/opt/agent-audit}"
RUNS_DIR="${AGENT_AUDIT_RUNS_DIR:-${PROJECT_ROOT}/runs}"
ENV_FILE="${AGENT_AUDIT_ENV_FILE:-${PROJECT_ROOT}/.env}"
DSH_HOME="${AGENT_AUDIT_DSH_HOME:-${PROJECT_ROOT}/dsh-home}"
DSH_PROFILE="${AGENT_AUDIT_DSH_PROFILE:-headless}"
DSH_PID=""
STDOUT_TEE=""
STDERR_TEE=""
TIMED_OUT=0
QUEUE_MESSAGE_ID=""
QUEUE_ATTEMPT=""
QUEUE_CONSUMER=""

usage() {
  cat >&2 <<'EOF'
usage: docker run ... [--prompt <text>]

Run one autonomous smart-contract audit with DeepSeek Harness headless mode.

prompt sources (highest priority first):
  1. FULL_PROMPT environment variable
  2. --prompt <text> CLI argument
  3. a task claimed from the Redis Stream when AGENT_AUDIT_REDIS_URL is set
     (the k3s / KEDA path: one Job claims exactly one task)

optional:
  TASK_ID              Stable task identifier; also becomes the run directory name.
  RUN_ID               Explicit run directory name (overrides TASK_ID).
  DSH_MODEL            DeepSeek model id. Default: deepseek-flash
  DSH_REASONING_EFFORT Reasoning effort: off, low, high, max. Default: max
  AGENT_AUDIT_RUNS_DIR Working run root. Default: /opt/agent-audit/runs
  AGENT_AUDIT_MAX_DURATION_SECONDS
                       Per-attempt wall-clock budget for dsh. On expiry the
                       agent is terminated, the run is marked "timeout", and
                       the exit code is 124 so the Job can retry. Default: 0
                       (disabled); the k3s ScaledJob sets 3000.
  AGENT_AUDIT_QUEUE_MODE
                       auto (default), pull, or off. In auto, pull mode is used
                       only when no prompt is given and AGENT_AUDIT_REDIS_URL
                       is set.
  AGENT_AUDIT_REDIS_URL
                       redis://host:port/db for pull mode.
  AGENT_AUDIT_QUEUE_STREAM
                       Stream name. Default: agent-audit:tasks
  AGENT_AUDIT_QUEUE_GROUP
                       Consumer group. Default: agent-audit-runners
  AGENT_AUDIT_QUEUE_CONSUMER
                       Consumer name. Default: $JOB_NAME, then $HOSTNAME. Use
                       the Job name so a retried pod reclaims its own task.
  AGENT_AUDIT_QUEUE_BLOCK_MS
                       How long to wait for work. Default: 5000
  AGENT_AUDIT_QUEUE_ACK
                       Set to 0 to leave claimed tasks unacknowledged.
  AGENT_AUDIT_ALLOW_IMAGE_MISMATCH
                       Set to 1 to run even when the queued image digest does
                       not match this container's AGENT_AUDIT_IMAGE.
  AGENT_AUDIT_PERSIST_DIR
                       Optional second run root; the finished run is copied there.
  AGENT_AUDIT_STATS    Set to 0 to skip post-run stats/metrics.json collection.
  AGENT_AUDIT_STATS_MODULE
                       Python path containing the audit_stats package.
                       Default: /opt/agent-audit/tools
  AGENT_AUDIT_REPLAY   Set to 0 to skip the post-exit fork replay. The replay
                       re-runs the tests each finding claims, at the pinned fork
                       block and at block minus delta. Default: on
  AGENT_AUDIT_REPLAY_TIMEOUT_MS
                       Total replay budget in milliseconds. Default: 900000
  AGENT_AUDIT_REPLAY_BLOCK_DELTA
                       Blocks between the replay and the state-sensitivity run.
                       Default: 7200
  AGENT_AUDIT_MONGO_URI
                       MongoDB URI. Persistence is skipped when this is unset.
  AGENT_AUDIT_MONGO_DB   Database name. Default: agent
  AGENT_AUDIT_MONGO_RUNS_META_COLLECTION
                       Metadata collection. Default: runs_meta
  AGENT_AUDIT_MONGO_RUNS_FILES_COLLECTION
                       Per-run file collection. Default: runs_files
  AGENT_AUDIT_MONGO_MAX_INLINE_FILE_BYTES
                       Skip files larger than this instead of failing the run.
                       Default: 8388608
  AGENT_AUDIT_MONGO_PERSIST
                       Set to 0 to skip MongoDB persistence.
  AGENT_AUDIT_PERSIST_MODULE
                       Python path containing the run_persist package.
                       Default: /opt/agent-audit/tools
  -h, --help           Show this help message

The container persists the whole run directory. In the single-audit Docker
entrypoint the host runs/ directory is bind-mounted at /opt/agent-audit/runs,
so evidence is durable as soon as it is written. In k3s the dispatcher mounts
the node-local /var/lib/agent-audit/runs directory into every Job.

Examples:
  ./docker/run.sh --prompt "Audit 0x... on eth."
  FULL_PROMPT="Audit 0x... on base." ./docker/run.sh
EOF
}

die() {
  printf 'agent-audit: %s\n' "$*" >&2
  exit 2
}

load_env_file() {
  local file="$1"
  local line key value
  [[ -f "$file" ]] || return 0

  while IFS= read -r line || [[ -n "$line" ]]; do
    line="${line%$'\r'}"
    [[ "$line" =~ ^[[:space:]]*$ ]] && continue
    [[ "$line" =~ ^[[:space:]]*# ]] && continue
    line="${line#"${line%%[![:space:]]*}"}"
    line="${line#export }"

    key="${line%%=*}"
    [[ "$key" =~ ^[A-Za-z_][A-Za-z0-9_]*$ ]] || continue
    value="${line#*=}"
    value="${value#"${value%%[![:space:]]*}"}"

    case "$value" in
      \"*\")
        value="${value:1:${#value}-2}"
        value="${value//\\n/$'\n'}"
        value="${value//\\\"/\"}"
        value="${value//\\\\/\\}"
        ;;
      \'*\')
        value="${value:1:${#value}-2}"
        ;;
      *)
        value="${value%%[[:space:]]#*}"
        value="${value%"${value##*[![:space:]]}"}"
        ;;
    esac

    if [[ -z "${!key+x}" ]]; then
      export "$key=$value"
    fi
  done <"$file"
}

parse_args() {
  PROMPT=""
  while [[ $# -gt 0 ]]; do
    case "$1" in
      -h|--help)
        HELP=1
        shift
        ;;
      --prompt)
        [[ $# -ge 2 ]] || die "--prompt requires a value"
        PROMPT="$2"
        shift 2
        ;;
      --prompt=*)
        PROMPT="${1#--prompt=}"
        shift
        ;;
      *)
        die "unknown argument: $1"
        ;;
    esac
  done
}

sanitize_run_id() {
  local raw="$1"
  local out
  out="$(printf '%s' "$raw" | tr '[:upper:]' '[:lower:]' | tr -c 'a-z0-9._-' '-' | sed -e 's/^-*//' -e 's/-*$//' -e 's/--*/-/g')"
  if [[ -z "$out" ]]; then
    out="run-$(date -u +%Y%m%dT%H%M%SZ)-$RANDOM"
  fi
  printf '%s' "${out:0:96}"
}

write_json_file() {
  local path="$1"
  local payload="$2"
  local tmp="${path}.tmp"
  printf '%s\n' "$payload" >"$tmp"
  mv "$tmp" "$path"
}

queue_module() {
  printf '%s' "${AGENT_AUDIT_TASK_QUEUE_MODULE:-${PROJECT_ROOT}/tools}"
}

# Claim exactly one task from the Redis Stream. Sets PROMPT, TASK_ID, RUN_ID,
# QUEUE_MESSAGE_ID, QUEUE_ATTEMPT and QUEUE_CONSUMER. Returns 3 when the queue
# is empty (the Job should succeed without doing anything).
claim_task() {
  local module
  module="$(queue_module)"
  if [[ ! -d "${module}/task_queue" ]]; then
    die "task_queue module not found under ${module}"
  fi
  if ! command -v python3 >/dev/null 2>&1; then
    die "python3 is required for pull mode"
  fi

  local tmp_json tmp_err rc=0
  tmp_json="$(mktemp)"
  tmp_err="$(mktemp)"
  PYTHONPATH="${module}${PYTHONPATH:+:${PYTHONPATH}}" \
    python3 -m task_queue claim >"$tmp_json" 2>"$tmp_err" || rc=$?
  cat "$tmp_err" >&2
  rm -f "$tmp_err"

  if [[ "$rc" -eq 3 ]]; then
    rm -f "$tmp_json"
    return 3
  fi
  if [[ "$rc" -ne 0 ]]; then
    rm -f "$tmp_json"
    die "task claim failed (exit=$rc)"
  fi

  local payload
  payload="$(cat "$tmp_json")"
  rm -f "$tmp_json"

  PROMPT="$(printf '%s' "$payload" | jq -r '.full_prompt')"
  TASK_ID="$(printf '%s' "$payload" | jq -r '.task_id')"
  RUN_ID="$(printf '%s' "$payload" | jq -r '.run_id')"
  QUEUE_MESSAGE_ID="$(printf '%s' "$payload" | jq -r '.message_id')"
  QUEUE_ATTEMPT="$(printf '%s' "$payload" | jq -r '.attempt')"
  QUEUE_CONSUMER="$(printf '%s' "$payload" | jq -r '.consumer')"

  local queued_image
  queued_image="$(printf '%s' "$payload" | jq -r '.image // empty')"
  if [[ -n "$queued_image" && -n "${AGENT_AUDIT_IMAGE:-}" && "$queued_image" != "$AGENT_AUDIT_IMAGE" ]]; then
    if [[ "${AGENT_AUDIT_ALLOW_IMAGE_MISMATCH:-0}" != "1" ]]; then
      die "queued image ${queued_image} does not match runner image ${AGENT_AUDIT_IMAGE} (set AGENT_AUDIT_ALLOW_IMAGE_MISMATCH=1 to override)"
    fi
    printf 'agent-audit: WARNING queued image %s != runner image %s\n' \
      "$queued_image" "$AGENT_AUDIT_IMAGE" >&2
  fi

  printf 'agent-audit: claimed task_id=%s message_id=%s attempt=%s consumer=%s\n' \
    "$TASK_ID" "$QUEUE_MESSAGE_ID" "$QUEUE_ATTEMPT" "$QUEUE_CONSUMER" >&2
  return 0
}

# Acknowledge the claimed task. Called only after a successful run, so a failed
# attempt stays pending and the Job retry (backoffLimit: 1) reclaims it.
ack_task() {
  local module rc=0
  module="$(queue_module)"
  PYTHONPATH="${module}${PYTHONPATH:+:${PYTHONPATH}}" \
    python3 -m task_queue ack --message-id "${QUEUE_MESSAGE_ID}" \
    >>"${RUN_DIR}/logs/queue.log" 2>&1 || rc=$?
  return "$rc"
}

wait_for_tee() {
  local pid="$1"
  local attempt
  for attempt in $(seq 1 20); do
    if ! kill -0 "$pid" 2>/dev/null; then
      wait "$pid" 2>/dev/null || true
      return 0
    fi
    sleep 0.1
  done
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
}

on_signal() {
  local code="$1"
  trap - TERM INT
  if [[ -n "${DSH_PID}" ]]; then
    kill -TERM "$DSH_PID" 2>/dev/null || true
  fi
  if [[ -n "${STDOUT_TEE}" ]]; then
    kill "$STDOUT_TEE" 2>/dev/null || true
  fi
  if [[ -n "${STDERR_TEE}" ]]; then
    kill "$STDERR_TEE" 2>/dev/null || true
  fi
  exit "$code"
}

finalize() {
  local code="$1"
  trap - EXIT INT TERM
  set +e

  if [[ -n "${RUN_DIR:-}" && -d "${RUN_DIR}" ]]; then
    local finished_at duration status
    finished_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    duration=$(( $(date +%s) - ${STARTED_EPOCH:-$(date +%s)} ))
    if [[ "$code" -eq 0 ]]; then
      status="completed"
    elif [[ "${TIMED_OUT:-0}" -eq 1 ]]; then
      status="timeout"
    else
      status="failed"
    fi

    if [[ -s "${RUN_DIR}/logs/agent.stdout.log" ]]; then
      cp "${RUN_DIR}/logs/agent.stdout.log" "${RUN_DIR}/reports/final_answer.md"
    fi

    jq -n \
      --arg run_id "${RUN_ID:-}" \
      --arg task_id "${TASK_ID:-}" \
      --arg prompt_file "input/prompt.txt" \
      --arg model "${DSH_MODEL:-}" \
      --arg reasoning_effort "${DSH_REASONING_EFFORT:-max}" \
      --arg image "${AGENT_AUDIT_IMAGE:-}" \
      --arg batch_id "${BATCH_ID:-}" \
      --arg queue_message_id "${QUEUE_MESSAGE_ID:-}" \
      --arg queue_attempt "${QUEUE_ATTEMPT:-}" \
      --arg status "$status" \
      --arg started_at "${STARTED_AT:-}" \
      --arg finished_at "$finished_at" \
      --arg stdout_log "logs/agent.stdout.log" \
      --arg stderr_log "logs/agent.stderr.log" \
      --argjson exit_code "$code" \
      --argjson duration_seconds "$duration" \
      '{
        schema_version: 1,
        run_id: $run_id,
        task_id: (if $task_id == "" then null else $task_id end),
        prompt_file: $prompt_file,
        model: $model,
        reasoning_effort: $reasoning_effort,
        image: (if $image == "" then null else $image end),
        batch_id: (if $batch_id == "" then null else $batch_id end),
        queue_message_id: (if $queue_message_id == "" then null else $queue_message_id end),
        queue_attempt: (if $queue_attempt == "" then null else ($queue_attempt | tonumber) end),
        status: $status,
        exit_code: $exit_code,
        started_at: $started_at,
        finished_at: $finished_at,
        duration_seconds: $duration_seconds,
        stdout_log: $stdout_log,
        stderr_log: $stderr_log
      }' >"${RUN_DIR}/input/run_meta.json"

    # Rebuild fork evidence after the agent has exited. The replay re-runs the
    # tests each finding claims against RPC_URL, records the command and block,
    # and repeats at block-minus-delta to test whether the assertion really
    # reads chain state. It runs before collection so metrics can trust it, and
    # it is best-effort: a replay failure never changes the audit exit code.
    if [[ "$code" -eq 0 ]] \
      && [[ "${AGENT_AUDIT_REPLAY:-1}" != "0" ]] \
      && command -v python3 >/dev/null 2>&1 \
      && [[ -s "${RUN_DIR}/reports/final_report.json" ]]; then
      local replay_module="${AGENT_AUDIT_STATS_MODULE:-${PROJECT_ROOT}/tools}"
      if [[ -d "${replay_module}/audit_stats" ]]; then
        PYTHONPATH="${replay_module}${PYTHONPATH:+:${PYTHONPATH}}" \
          python3 -m audit_stats replay "${RUN_DIR}" \
          >"${RUN_DIR}/logs/replay.log" 2>&1 || true
      fi
    fi

    # Derive stats/metrics.json from the finished run. This is best-effort:
    # a metrics failure must never change the audit's exit code, and the
    # collector already writes atomically. AGENT_AUDIT_STATS=0 disables it.
    if [[ "${AGENT_AUDIT_STATS:-1}" != "0" ]] && command -v python3 >/dev/null 2>&1; then
      local stats_module="${AGENT_AUDIT_STATS_MODULE:-${PROJECT_ROOT}/tools}"
      if [[ -d "${stats_module}/audit_stats" ]]; then
        PYTHONPATH="${stats_module}${PYTHONPATH:+:${PYTHONPATH}}" \
          python3 -m audit_stats collect "${RUN_DIR}" \
          >/dev/null 2>"${RUN_DIR}/logs/stats.log" || true
      fi
    fi

    # Persist exactly one finished run: one runs_meta document (target
    # flattened, stats embedded) plus that run's own runs_files material.
    # This runs once per address audit and never walks a batch-level directory.
    # A database failure is logged but does not change the audit exit code.
    if [[ "${AGENT_AUDIT_MONGO_PERSIST:-1}" != "0" ]]; then
      local persist_module="${AGENT_AUDIT_PERSIST_MODULE:-${PROJECT_ROOT}/tools}"
      if [[ -z "${AGENT_AUDIT_MONGO_URI:-}" ]]; then
        printf 'mongo persistence skipped: AGENT_AUDIT_MONGO_URI is not configured\n' \
          >>"${RUN_DIR}/logs/persist.log" 2>/dev/null || true
      elif ! command -v python3 >/dev/null 2>&1; then
        printf 'mongo persistence skipped: python3 is not available\n' \
          >>"${RUN_DIR}/logs/persist.log" 2>/dev/null || true
      elif [[ ! -d "${persist_module}/run_persist" ]]; then
        printf 'mongo persistence skipped: %s/run_persist not found\n' "$persist_module" \
          >>"${RUN_DIR}/logs/persist.log" 2>/dev/null || true
      else
        local persist_code=0
        PYTHONPATH="${persist_module}${PYTHONPATH:+:${PYTHONPATH}}" \
          python3 -m run_persist "${RUN_DIR}" \
          >>"${RUN_DIR}/logs/persist.log" 2>&1 || persist_code=$?
        if [[ "$persist_code" -ne 0 ]]; then
          printf 'agent-audit: mongo persistence failed (exit=%s); see %s/logs/persist.log\n' \
            "$persist_code" "${RUN_DIR}" >&2
        fi
      fi
    fi

    if [[ -n "${AGENT_AUDIT_PERSIST_DIR:-}" && "${AGENT_AUDIT_PERSIST_DIR}" != "${RUNS_DIR}" ]]; then
      mkdir -p "${AGENT_AUDIT_PERSIST_DIR}/${RUN_ID}"
      cp -a "${RUN_DIR}/." "${AGENT_AUDIT_PERSIST_DIR}/${RUN_ID}/"
    fi

    # Acknowledge only a successful run. Everything above (run_meta, replay,
    # metrics, Mongo archive) has already been written, so a crash between
    # here and XACK at worst re-runs an audit, never loses one.
    if [[ "$code" -eq 0 && -n "${QUEUE_MESSAGE_ID:-}" \
      && "${AGENT_AUDIT_QUEUE_ACK:-1}" != "0" ]]; then
      if ! ack_task; then
        printf 'agent-audit: WARNING failed to ack message %s; it stays pending\n' \
          "$QUEUE_MESSAGE_ID" >&2
      fi
    fi

    sync
    printf 'agent-audit: run %s %s (exit=%s) at %s\n' \
      "${RUN_ID}" "$status" "$code" "${RUN_DIR}" >&2
  fi

  exit "$code"
}

main() {
  HELP=0
  PROMPT=""
  parse_args "$@"

  if [[ "$HELP" -eq 1 ]]; then
    usage
    return 0
  fi

  load_env_file "$ENV_FILE"

  RUNS_DIR="${AGENT_AUDIT_RUNS_DIR:-${PROJECT_ROOT}/runs}"
  DSH_HOME="${AGENT_AUDIT_DSH_HOME:-${PROJECT_ROOT}/dsh-home}"
  DSH_PROFILE="${AGENT_AUDIT_DSH_PROFILE:-headless}"
  DSH_MODEL="${DSH_MODEL:-deepseek-flash}"

  # Resolve relative run roots before changing into the run directory.
  if [[ "$RUNS_DIR" != /* ]]; then
    RUNS_DIR="${PROJECT_ROOT}/${RUNS_DIR}"
  fi
  if [[ -n "${AGENT_AUDIT_PERSIST_DIR:-}" && "${AGENT_AUDIT_PERSIST_DIR}" != /* ]]; then
    AGENT_AUDIT_PERSIST_DIR="${PROJECT_ROOT}/${AGENT_AUDIT_PERSIST_DIR}"
  fi

  local env_prompt="${FULL_PROMPT:-}"
  if [[ -n "${env_prompt//[[:space:]]/}" ]]; then
    PROMPT="$FULL_PROMPT"
  fi

  # Pull mode: no prompt was supplied, so claim one task from the stream. KEDA
  # creates one Job per queued entry; the Job claims exactly one of them.
  if [[ -z "${PROMPT//[[:space:]]/}" ]]; then
    local queue_mode="${AGENT_AUDIT_QUEUE_MODE:-auto}"
    case "$queue_mode" in
      off)
        ;;
      pull|auto)
        if [[ -n "${AGENT_AUDIT_REDIS_URL:-}" ]]; then
          local claim_rc=0
          claim_task || claim_rc=$?
          if [[ "$claim_rc" -eq 3 ]]; then
            printf 'agent-audit: queue empty; nothing to do\n' >&2
            return 0
          elif [[ "$claim_rc" -ne 0 ]]; then
            return "$claim_rc"
          fi
        elif [[ "$queue_mode" == "pull" ]]; then
          die "AGENT_AUDIT_QUEUE_MODE=pull requires AGENT_AUDIT_REDIS_URL"
        fi
        ;;
      *)
        die "AGENT_AUDIT_QUEUE_MODE must be auto, pull, or off"
        ;;
    esac
  fi

  if [[ -z "${PROMPT//[[:space:]]/}" ]]; then
    usage
    return 2
  fi

  if [[ -z "${DEEPSEEK_API_KEY:-}" ]]; then
    die "DEEPSEEK_API_KEY is required (set it in ${ENV_FILE} or the environment)"
  fi

  export RUNS_DIR DSH_HOME DSH_PROFILE DSH_MODEL
  export DSH_PERMISSION_MODE="${DSH_PERMISSION_MODE:-danger-full-access}"
  export DSH_TELEMETRY_DISABLED="${DSH_TELEMETRY_DISABLED:-1}"

  export ETHERSCAN_API_KEY="${ETHERSCAN_API_KEY:-${AGENT_AUDIT_SOURCE_API_KEY:-}}"
  export ETHERSCAN_API_URL="${ETHERSCAN_API_URL:-${AGENT_AUDIT_SOURCE_API_BASE:-https://api.etherscan.io/v2/api}}"
  export RPC_URL="${RPC_URL:-${AGENT_AUDIT_RPC_URL:-}}"
  export AGENT_AUDIT_RPC_URL="$RPC_URL"
  export AGENT_AUDIT_SOURCE_API_BASE="$ETHERSCAN_API_URL"
  export AGENT_AUDIT_SOURCE_API_KEY="$ETHERSCAN_API_KEY"

  mkdir -p "$RUNS_DIR"

  local requested_run_id="${RUN_ID:-${TASK_ID:-}}"
  if [[ -n "$requested_run_id" ]]; then
    RUN_ID="$(sanitize_run_id "$requested_run_id")"
  else
    RUN_ID="run-$(date -u +%Y%m%dT%H%M%SZ)-$(printf '%04x' $RANDOM)"
  fi
  RUN_DIR="${RUNS_DIR}/${RUN_ID}"

  # A retried Job pod and a manually requeued task both land on an existing
  # run id. In pull mode, suffix instead of failing so the earlier attempt stays
  # on disk as evidence. The Docker single-run path keeps the strict error.
  if [[ -e "$RUN_DIR" ]]; then
    if [[ -n "${QUEUE_MESSAGE_ID:-}" ]]; then
      local base_run_id="$RUN_ID" attempt=2
      while [[ -e "${RUNS_DIR}/${RUN_ID}" ]]; do
        RUN_ID="${base_run_id}-r${attempt}"
        attempt=$((attempt + 1))
      done
      RUN_DIR="${RUNS_DIR}/${RUN_ID}"
      printf 'agent-audit: run id collision; using %s\n' "$RUN_ID" >&2
    else
      die "run directory already exists: $RUN_DIR"
    fi
  fi

  mkdir -p "$RUN_DIR/input" "$RUN_DIR/artifacts" "$RUN_DIR/reports" "$RUN_DIR/logs"
  printf '%s' "$PROMPT" >"$RUN_DIR/input/prompt.txt"

  STARTED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  STARTED_EPOCH="$(date +%s)"
  export RUN_ID RUN_DIR TASK_ID STARTED_AT STARTED_EPOCH

  write_json_file "$RUN_DIR/input/request.json" "$(jq -n \
    --arg run_id "$RUN_ID" \
    --arg task_id "${TASK_ID:-}" \
    --arg queue_message_id "${QUEUE_MESSAGE_ID:-}" \
    --arg prompt "$PROMPT" \
    --arg created_at "$STARTED_AT" \
    '{
      schema_version: 1,
      run_id: $run_id,
      task_id: (if $task_id == "" then null else $task_id end),
      queue_message_id: (if $queue_message_id == "" then null else $queue_message_id end),
      created_at: $created_at,
      prompt: $prompt
    }')"

  write_json_file "$RUN_DIR/input/run_meta.json" "$(jq -n \
    --arg run_id "$RUN_ID" \
    --arg task_id "${TASK_ID:-}" \
    --arg prompt_file "input/prompt.txt" \
    --arg model "$DSH_MODEL" \
    --arg reasoning_effort "${DSH_REASONING_EFFORT:-max}" \
    --arg image "${AGENT_AUDIT_IMAGE:-}" \
    --arg batch_id "${BATCH_ID:-}" \
    --arg queue_message_id "${QUEUE_MESSAGE_ID:-}" \
    --arg queue_attempt "${QUEUE_ATTEMPT:-}" \
    --arg started_at "$STARTED_AT" \
    '{
      schema_version: 1,
      run_id: $run_id,
      task_id: (if $task_id == "" then null else $task_id end),
      prompt_file: $prompt_file,
      model: $model,
      reasoning_effort: $reasoning_effort,
      image: (if $image == "" then null else $image end),
      batch_id: (if $batch_id == "" then null else $batch_id end),
      queue_message_id: (if $queue_message_id == "" then null else $queue_message_id end),
      queue_attempt: (if $queue_attempt == "" then null else ($queue_attempt | tonumber) end),
      status: "running",
      exit_code: null,
      started_at: $started_at,
      finished_at: null,
      duration_seconds: null,
      stdout_log: "logs/agent.stdout.log",
      stderr_log: "logs/agent.stderr.log"
    }')"

  trap 'finalize $?' EXIT
  trap 'on_signal 143' TERM
  trap 'on_signal 130' INT

  printf 'agent-audit: starting run %s\n' "$RUN_ID" >&2
  printf 'agent-audit: workspace %s\n' "$RUN_DIR" >&2
  printf 'agent-audit: model %s (%s)\n' "$DSH_MODEL" "${DSH_REASONING_EFFORT:-max}" >&2

  cd "$RUN_DIR" || die "cannot enter run directory: $RUN_DIR"

  local dsh_bin dsh_js
  dsh_bin="$(command -v dsh || true)"
  [[ -n "$dsh_bin" ]] || die "dsh is not installed"

  dsh_js="$(readlink -f "$dsh_bin" 2>/dev/null || printf '%s' "$dsh_bin")"
  if [[ "$dsh_js" == *.js ]]; then
    dsh_cmd=(node --expose-internals "$dsh_js")
  else
    dsh_cmd=("$dsh_bin")
  fi

  local code=0
  local max_duration="${AGENT_AUDIT_MAX_DURATION_SECONDS:-0}"
  if ! [[ "$max_duration" =~ ^[0-9]+$ ]]; then
    die "AGENT_AUDIT_MAX_DURATION_SECONDS must be a non-negative integer"
  fi

  # The Job has activeDeadlineSeconds as a hard backstop, but a hung agent must
  # fail *inside* the budget so the retry still has time to run. dsh runs under
  # coreutils timeout; exit 124 is what both timeout and the Job see.
  local -a run_cmd=("${dsh_cmd[@]}")
  if [[ "$max_duration" -gt 0 ]]; then
    if command -v timeout >/dev/null 2>&1; then
      run_cmd=(timeout --signal=TERM --kill-after=30s "${max_duration}s" "${dsh_cmd[@]}")
    else
      printf 'agent-audit: WARNING timeout(1) not found; running without a per-attempt budget\n' >&2
    fi
    printf 'agent-audit: per-attempt budget %ss\n' "$max_duration" >&2
  fi

  local fifo_dir="$RUN_DIR/logs/.stream"
  if mkdir -p "$fifo_dir" && mkfifo "$fifo_dir/stdout" "$fifo_dir/stderr" 2>/dev/null; then
    tee -a "$RUN_DIR/logs/agent.stdout.log" <"$fifo_dir/stdout" &
    STDOUT_TEE=$!
    tee -a "$RUN_DIR/logs/agent.stderr.log" >&2 <"$fifo_dir/stderr" &
    STDERR_TEE=$!

    # DSH headless reads stdin as the initial user message; prompt.txt is the
    # durable copy kept in the run record.
    "${run_cmd[@]}" --profile "$DSH_PROFILE" \
      <"$RUN_DIR/input/prompt.txt" \
      >"$fifo_dir/stdout" \
      2>"$fifo_dir/stderr" &
    DSH_PID=$!
    wait "$DSH_PID"
    code=$?
    DSH_PID=""

    wait_for_tee "$STDOUT_TEE"
    wait_for_tee "$STDERR_TEE"
    STDOUT_TEE=""
    STDERR_TEE=""
    rm -rf "$fifo_dir"
  else
    # DSH headless reads stdin as the initial user message; prompt.txt is the
    # durable copy kept in the run record.
    "${run_cmd[@]}" --profile "$DSH_PROFILE" \
      <"$RUN_DIR/input/prompt.txt" \
      >"$RUN_DIR/logs/agent.stdout.log" \
      2>"$RUN_DIR/logs/agent.stderr.log" &
    DSH_PID=$!
    wait "$DSH_PID"
    code=$?
    DSH_PID=""
    cat "$RUN_DIR/logs/agent.stdout.log"
    cat "$RUN_DIR/logs/agent.stderr.log" >&2
  fi

  if [[ "$code" -eq 124 ]]; then
    TIMED_OUT=1
    printf 'agent-audit: dsh exceeded %ss; terminating the attempt\n' "$max_duration" >&2
  fi

  return "$code"
}

main "$@"
