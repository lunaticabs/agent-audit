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

usage() {
  cat >&2 <<'EOF'
usage: docker run ... [--prompt <text>]

Run one autonomous smart-contract audit with DeepSeek Harness headless mode.

prompt sources (highest priority first):
  1. FULL_PROMPT environment variable
  2. --prompt <text> CLI argument

optional:
  TASK_ID              Stable task identifier; also becomes the run directory name.
  RUN_ID               Explicit run directory name (overrides TASK_ID).
  DSH_MODEL            DeepSeek model id. Default: deepseek-flash
  DSH_REASONING_EFFORT Reasoning effort: off, low, high, max. Default: max
  AGENT_AUDIT_RUNS_DIR Working run root. Default: /opt/agent-audit/runs
  AGENT_AUDIT_PERSIST_DIR
                       Optional second run root; the finished run is copied there.
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
        status: $status,
        exit_code: $exit_code,
        started_at: $started_at,
        finished_at: $finished_at,
        duration_seconds: $duration_seconds,
        stdout_log: $stdout_log,
        stderr_log: $stderr_log
      }' >"${RUN_DIR}/input/run_meta.json"

    if [[ -n "${AGENT_AUDIT_PERSIST_DIR:-}" && "${AGENT_AUDIT_PERSIST_DIR}" != "${RUNS_DIR}" ]]; then
      mkdir -p "${AGENT_AUDIT_PERSIST_DIR}/${RUN_ID}"
      cp -a "${RUN_DIR}/." "${AGENT_AUDIT_PERSIST_DIR}/${RUN_ID}/"
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

  if [[ -e "$RUN_DIR" ]]; then
    die "run directory already exists: $RUN_DIR"
  fi

  mkdir -p "$RUN_DIR/input" "$RUN_DIR/artifacts" "$RUN_DIR/reports" "$RUN_DIR/logs"
  printf '%s' "$PROMPT" >"$RUN_DIR/input/prompt.txt"

  STARTED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  STARTED_EPOCH="$(date +%s)"
  export RUN_ID RUN_DIR TASK_ID STARTED_AT STARTED_EPOCH

  write_json_file "$RUN_DIR/input/request.json" "$(jq -n \
    --arg run_id "$RUN_ID" \
    --arg task_id "${TASK_ID:-}" \
    --arg prompt "$PROMPT" \
    --arg created_at "$STARTED_AT" \
    '{
      schema_version: 1,
      run_id: $run_id,
      task_id: (if $task_id == "" then null else $task_id end),
      created_at: $created_at,
      prompt: $prompt
    }')"

  write_json_file "$RUN_DIR/input/run_meta.json" "$(jq -n \
    --arg run_id "$RUN_ID" \
    --arg task_id "${TASK_ID:-}" \
    --arg prompt_file "input/prompt.txt" \
    --arg model "$DSH_MODEL" \
    --arg reasoning_effort "${DSH_REASONING_EFFORT:-max}" \
    --arg started_at "$STARTED_AT" \
    '{
      schema_version: 1,
      run_id: $run_id,
      task_id: (if $task_id == "" then null else $task_id end),
      prompt_file: $prompt_file,
      model: $model,
      reasoning_effort: $reasoning_effort,
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
  local fifo_dir="$RUN_DIR/logs/.stream"
  if mkdir -p "$fifo_dir" && mkfifo "$fifo_dir/stdout" "$fifo_dir/stderr" 2>/dev/null; then
    tee -a "$RUN_DIR/logs/agent.stdout.log" <"$fifo_dir/stdout" &
    STDOUT_TEE=$!
    tee -a "$RUN_DIR/logs/agent.stderr.log" >&2 <"$fifo_dir/stderr" &
    STDERR_TEE=$!

    # DSH headless reads stdin as the initial user message; prompt.txt is the
    # durable copy kept in the run record.
    "${dsh_cmd[@]}" --profile "$DSH_PROFILE" \
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
    "${dsh_cmd[@]}" --profile "$DSH_PROFILE" \
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

  return "$code"
}

main "$@"
