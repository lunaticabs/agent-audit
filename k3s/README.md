# k3s parallel audit deployment

This directory runs many one-shot audits on a single-node k3s cluster. KEDA
watches the Redis Stream and creates one Kubernetes Job per queued task; the
Job claims exactly one message, runs one DSH headless audit, writes the complete
workspace to a node-local `runs/` directory, and only then acknowledges the
message.

Topology:

```text
Redis Stream (agent-audit:tasks, group agent-audit-runners)
  -> KEDA ScaledJob (lag -> number of Jobs, max 26 unfinished)
  -> one Kubernetes Job per task
  -> runner pod claims one message (consumer = Job name)
  -> agent-audit runner (DSH headless + tools)
  -> /opt/agent-audit/runs/<run_id>
  -> hostPath /var/lib/agent-audit/runs/<run_id>
  -> ack on success (when configured: runs_meta + runs_files for that run)
```

There is no long-running runner Deployment. KEDA owns admission control (how
many Jobs may exist), Job history, and pause/resume; the runner owns claiming,
retrying, and acknowledging its own task.

## Why the runner claims its own task

KEDA cannot inject a prompt into a Job: it only creates Jobs from queue depth.
So the Job claims its own task:

- first attempt reads new work (`XREADGROUP ... >`);
- a retry reads this Job's own pending entry (`... 0`) first, so the second pod
  of the same Job re-runs the **same** task;
- the consumer name is therefore the **Job name** (downward API), not the pod
  name, which changes on retry;
- `XACK` happens after `run_meta.json`, replay, metrics, and the MongoDB archive
  are written. A crashed run stays pending and is retried; it is never lost.

## Prerequisites: install KEDA

The cluster needs the KEDA CRDs before `runner-scaledjob.yaml` can be applied.

```bash
./scripts/install_keda.sh          # pinned KEDA 2.21.0
# KEDA_VERSION=2.21.0 KUBECTL=kubectl ./scripts/install_keda.sh
```

Redis must be 7+ for the `lagCount` trigger (the bundled image is 7.2).

## Build and pin the runner image

```bash
./docker/build.sh

# Resolves the tag to a digest, then rewrites the Job image and
# AGENT_AUDIT_IMAGE in k3s/runner-scaledjob.yaml.
./scripts/set_runner_image.sh ghcr.io/lunaticabs/agent-audit:rebuild --apply
```

`scripts/set_runner_image.sh <repo@sha256:...> [--apply]` rewrites
`image:` and `AGENT_AUDIT_IMAGE` in [runner-scaledjob.yaml](runner-scaledjob.yaml).
Both must be identical: the runner refuses to start a task whose queued image
differs from `AGENT_AUDIT_IMAGE` (override with `AGENT_AUDIT_ALLOW_IMAGE_MISMATCH=1`).

A tag is mutable; a digest is not. Repinning is the only way a running batch
changes code, and running Jobs keep their image.

## Prepare the node

```bash
sudo mkdir -p /var/lib/agent-audit/runs
sudo chmod 700 /var/lib/agent-audit/runs
```

The run root is set in [runner-scaledjob.yaml](runner-scaledjob.yaml)
(`volumes[0].hostPath.path`). Every Job on the node shares it.

## Concurrency and resources

`maxReplicaCount: 26` is the hard gate; KEDA never creates more than 26
unfinished Jobs. What you actually get is the smaller of the CPU and memory
admission limits:

| Node | CPU slots (`300m` request) | Memory slots (`768Mi` request) | Effective |
| --- | --- | --- | --- |
| 8 CPU / 16 Gi | 26 | ~21 | ~21 |
| 8 CPU / 24 Gi | 26 | 32 | 26 (cap) |
| 16 CPU / 32 Gi | 26 (cap) | 42 | 26 (cap) |

Tuning notes:

- CPU is the gate at 8 cores. `limits.cpu: 1500m` allows a burst for `forge
  build` while keeping the worst-case oversubscription near 5x; expect builds to
  slow as concurrency rises.
- Memory is the risk, not the gate. `limits.memory: 3Gi` protects the node from
  one runaway run, but 26 concurrent runs can still pressure a 16 Gi node into
  OOM kills. Watch for `OOMKilled` in failed Jobs and lower `maxReplicaCount` if
  it appears.
- If you want `requests == limits` (Guaranteed QoS, predictable latency), raise
  requests to the measured p90 and lower `maxReplicaCount` accordingly.
- Results can be stratified by image; the digest is recorded in `run_meta.image`
  and `stats/metrics.json`.

## Prepare secrets

```bash
cp k3s/runner-secret.example.yaml k3s/runner-secret.yaml
```

Fill [runner-secret.example.yaml](runner-secret.example.yaml):

- `DEEPSEEK_API_KEY` — required by DSH
- `DSH_MODEL` — defaults to `deepseek-flash`; use `deepseek-v4-pro` for a
  stronger model
- `DSH_REASONING_EFFORT` — `off`, `low`, `high`, or `max`
- `ETHERSCAN_API_KEY`, `ETHERSCAN_API_URL` — direct Etherscan v2 access
- `RPC_URL` — archive RPC for fork tests, fuzzing, and the archive-time
  `eth_blockNumber` fallback
- `AGENT_AUDIT_REDIS_URL` — the queue the runner claims from. For the bundled
  Redis: `redis://agent-audit-redis:6379/0`
- `AGENT_AUDIT_MONGO_URI`, `AGENT_AUDIT_MONGO_DB`,
  `AGENT_AUDIT_MONGO_RUNS_META_COLLECTION`,
  `AGENT_AUDIT_MONGO_RUNS_FILES_COLLECTION` — per-run MongoDB mirror. Unset the
  URI to disable it for a deployment.

For a smoke test, point `AGENT_AUDIT_MONGO_DB` at a non-production database
(for example `Test`). Never validate against the production database. Remove
stale Codex/OpenRouter keys from existing secrets before applying.

If the GHCR image is private, create a registry pull secret in the namespace and
add `imagePullSecrets` to the ScaledJob pod template.

## Deploy

```bash
./scripts/install_keda.sh
k3s kubectl apply -f k3s/namespace.yaml
k3s kubectl apply -f k3s/runner-secret.yaml
k3s kubectl apply -k k3s/
```

Watch the control plane:

```bash
k3s kubectl -n agent-audit get scaledjob,jobs,pods
k3s kubectl -n agent-audit get jobs,pods -w

# one screen: queue lag/pending, running/failed Jobs, pod phase, retries
python3 scripts/watch_batch.py --kubectl "k3s kubectl"
```

Pause the whole batch (no new Jobs; running ones continue):

```bash
k3s kubectl -n agent-audit annotate scaledjob agent-audit-runner \
  autoscaling.keda.sh/paused=true --overwrite
# resume
k3s kubectl -n agent-audit annotate scaledjob agent-audit-runner \
  autoscaling.keda.sh/paused=false --overwrite
```

## Submit tasks

KEDA scales on the consumer group's lag, so the group must exist before tasks
are added. `scripts/enqueue_redis.py` creates it when missing. If you add tasks
by hand, bootstrap it first — the command is idempotent and also pins
`entries-read`, without which a trimmed stream reports `lag nil` and KEDA never
activates:

```bash
k3s kubectl -n agent-audit port-forward svc/agent-audit-redis 6380:6379 &
PYTHONPATH=tools AGENT_AUDIT_REDIS_URL=redis://127.0.0.1:6380/0 \
  python3 -m task_queue bootstrap

k3s kubectl -n agent-audit exec deploy/agent-audit-redis -- \
  redis-cli XADD agent-audit:tasks '*' \
    task_id audit-20261026-001 \
    full_prompt 'Audit 0x0000000000000000000000000000000000000000 on eth.'
```

Batch submission from an address list. The script opens the Redis
port-forward itself (a free local port), reads the pinned image from
[runner-scaledjob.yaml](runner-scaledjob.yaml), enqueues, writes a manifest,
and closes the tunnel:

```bash
./scripts/enqueue_batch.py
```

`--max-count 10` for a trial batch, `--chain base` for another chain,
`--host/--port` when a tunnel is already open, `--dry-run` to print only. The
lower-level `scripts/enqueue_redis.py` remains for custom Redis endpoints.

`--image` is resolved to `repo@sha256:...` before the first `XADD` (default
`--pin-digest`); the tag is kept in the message as `image_tag` and the resolved
digest is printed as `[IMAGE]`. Resolution failures abort the batch unless you
pass `--allow-unresolved-image`. For private registries pass `--registry-user`
and `--registry-token`; the token endpoint uses them as HTTP basic credentials.

Optional reproducibility manifest (task ids + tag + digest):

```bash
python3 scripts/enqueue_redis.py --chain eth --image ... \
  --manifest-out runs/batch-$(date -u +%Y%m%d)/manifest.json
```

The default prompt template is:

```text
Audit {address} on {chain}.
```

DSH's skill catalog triggers the `evm-audit` skill from its description, so the
prompt can stay minimal and needs no routing text.

Preview without sending (still resolves the digest unless `--no-pin-digest`):

```bash
python3 scripts/enqueue_redis.py --chain eth --dry-run
```

## Read results

Use Job state for execution state (`kubectl -n agent-audit get jobs`) and the
host run directory for audit artifacts:

```bash
k3s kubectl -n agent-audit get jobs -o custom-columns=\
NAME:.metadata.name,STATUS:.status.conditions[0].type,START:.status.startTime
sudo ls -la /var/lib/agent-audit/runs/
```

Each run contains:

```text
input/prompt.txt
input/request.json           # includes queue_message_id
input/run_meta.json          # status, exit code, timings, image, queue metadata
artifacts/                   # evidence the agent produced
sources/                     # optional fetched source
reports/final_report.json    # required report
reports/final_answer.md      # stdout captured from DSH
logs/agent.stdout.log
logs/agent.stderr.log
logs/stats.log               # stats collector output
logs/persist.log             # MongoDB persistence result
logs/queue.log               # claim/ack result
stats/metrics.json           # derived post-run metrics
```

Retries do not overwrite evidence: when the run id already exists, the runner
suffixes it (`<run_id>-r2`, `-r3`, …), so a failed first attempt stays on disk
next to the successful retry. `run_meta.queue_attempt` records which attempt it
was.

## Failures and the pending list

- `backoffLimit: 1` gives each task one retry inside the same Job. The retry
  re-reads the Job's pending entry, so it re-runs the same task.
- `activeDeadlineSeconds: 7200` bounds the Job. The runner also enforces
  `AGENT_AUDIT_MAX_DURATION_SECONDS` (set to 3000 — 50 minutes — in the ScaledJob
  manifest) **per attempt** and exits 124 on expiry, which leaves room for the
  retry inside the 2h window.
- A task whose Job failed twice stays in the consumer group's pending list and
  is no longer counted as lag, so it does not block scaling. Inspect and requeue
  it:

```bash
PYTHONPATH=tools python3 -m task_queue pending --details
PYTHONPATH=tools python3 -m task_queue requeue --message-id <id>
```

- `failedJobsHistoryLimit: 100` keeps failed Jobs for inspection (and the run
  directories stay on the hostPath). Successful Jobs are kept 10 deep.
- If Redis loses its memory (it has no persistence volume), both the stream and
  the group are gone. Re-enqueue the batch; there is no replay of lost tasks.

## MongoDB archive

When `AGENT_AUDIT_MONGO_URI` is set in the runner secret, the entrypoint writes
one persistence record per address audit:

- `runs_meta`: `_id` is the run id; `target_address` / `target_chain` are
  top-level fields; the full `stats/metrics.json` object is under `stats`;
  status, timestamps, duration, block number, file count, total bytes, and
  `has_final_report` stay top-level.
- `runs_files`: one document for every regular file under that run's
  directory, including logs, stats, build output, and root files.
  `logs/persist.log` and oversized files are handled specially; batch-level
  directories are never walked.
- `materials`: one manifest entry per archived file (`rel_path`, `kind`,
  `size_bytes`, `sha256`) inside `runs_meta`, so evidence paths can be checked
  from the run document alone. `block_number` is taken from saved evidence
  first, then fetched with `eth_blockNumber` from `RPC_URL`; the source is
  stored in `block_number_source`.

Create the indexes once after the first deployment (safe to re-run):

```bash
PYTHONPATH=tools python3 -m run_persist --init-db
```

The historical nested `target.address` / `target.chain` documents are not
migrated. New documents use the flattened schema. A persistence failure is
logged in `runs/<run_id>/logs/persist.log` and does not change the audit exit
code. See [../tools/run_persist/README.md](../tools/run_persist/README.md).

## Round statistics

The runner writes `stats/metrics.json` from the finished run before the
container exits, so the metrics sit next to the evidence on the node-local
hostPath. Nothing else is required for collection.

Aggregate on the node (or on any host that can see the runs root):

```bash
PYTHONPATH=tools python3 -m audit_stats collect --runs-root /var/lib/agent-audit/runs
PYTHONPATH=tools python3 -m audit_stats report \
  --runs-root /var/lib/agent-audit/runs \
  --expected 500 \
  --out /var/lib/agent-audit/reports/round-20261027
```

For multiple nodes, copy only the metrics files and aggregate centrally:

```bash
for node in node1 node2 node3; do
  rsync -a --include='*/' --include='stats/' --include='stats/metrics.json' \
    --exclude='*' "root@${node}:/var/lib/agent-audit/runs/" /central/runs/
done
PYTHONPATH=tools python3 -m audit_stats report --runs-root /central/runs --expected 500
```

A CronJob mounting the same hostPath can run this on a schedule. The runner
records `AGENT_AUDIT_IMAGE`, so results can be stratified by image digest; add a
`batch_id` field to the Redis message when you need per-batch completion.

The MongoDB mirror is independent of this file-based aggregation. It stores the
same `stats/metrics.json` inside `runs_meta`, so a central consumer can query
`runs_meta.stats` without reaching a node-local hostPath.
See [docs/audit-statistics.md](../docs/audit-statistics.md) for metric
definitions and failure modes.

## Operational notes

- Job names are generated by KEDA (`<scaledjob>-<random>`); the run directory
  name comes from `task_id`, and `input/request.json` carries
  `queue_message_id` for correlation back to the stream.
- KEDA creates at most `maxReplicaCount` unfinished Jobs. A Job that is waiting
  on CPU/memory stays Pending and counts against that limit, so the queue waits
  on scheduling, not on a queue controller.
- There is no `ttlSecondsAfterFinished`: Jobs are reaped by the KEDA history
  limits so failures remain inspectable.
- `hostPath` is node-local. All Jobs on the node share the runs root, so a
  runner can read sibling run directories; use per-run volumes if audits need
  tenant isolation. For a multi-node cluster, replace it with shared
  ReadWriteMany storage or a per-node collector; do not assume runs from
  different nodes are in the same directory.
- A runtime image change is: build, `scripts/set_runner_image.sh <digest>
  --apply`. Running Jobs keep their image; new Jobs use the new digest. The
  runner then rejects tasks enqueued for the old digest until the batch is
  re-enqueued.
