# k3s parallel audit deployment

This directory runs many one-shot audits on a single-node k3s cluster. Each
Redis message becomes one Kubernetes Job; each Job runs one DSH headless audit
and writes its complete workspace to a node-local `runs/` directory.

Topology:

```text
Redis Stream
  -> agent-audit-dispatcher Deployment
  -> one Kubernetes Job per task
  -> agent-audit runner (DSH headless + tools)
  -> /opt/agent-audit/runs/<run_id>
  -> hostPath /var/lib/agent-audit/runs/<run_id>
```

There is no database, no archive service, and no long-running runner
Deployment. The run directory is the durable artifact. The dispatcher only
creates Jobs; it does not assemble prompts or write task state.

## Build images

```bash
./docker/build.sh
./docker/build-dispatcher.sh
```

Or let GitHub Actions publish both images to GHCR. The manifests currently use:

- `ghcr.io/lunaticabs/agent-audit:rebuild`
- `ghcr.io/lunaticabs/agent-audit-dispatcher:rebuild`

`rebuild` is a mutable branch tag: every push to the `rebuild` branch overwrites
it. That is convenient while iterating, but a node that already cached the tag
will not re-pull it under `IfNotPresent`. For a fixed release, use the
`sha-<short>` tag published by the same workflow, or an image digest.

If you use different tags, update [runner-configmap.yaml](runner-configmap.yaml)
and [dispatcher-deployment.yaml](dispatcher-deployment.yaml). For tighter
release control, use a `sha-...` tag or an image digest.

## Prepare the node

Each runner Job mounts a hostPath volume. Create the directory once on the k3s
node (the `DirectoryOrCreate` type also creates it, but an explicit directory
with restrictive permissions is clearer):

```bash
sudo mkdir -p /var/lib/agent-audit/runs
sudo chmod 700 /var/lib/agent-audit/runs
```

The path is configurable through
`DISPATCHER_RUNNER_RUNS_HOST_PATH` in [runner-configmap.yaml](runner-configmap.yaml).

## Prepare secrets

```bash
cp k3s/runner-secret.example.yaml k3s/runner-secret.yaml
cp k3s/dispatcher-secret.example.yaml k3s/dispatcher-secret.yaml
```

Fill [runner-secret.example.yaml](runner-secret.example.yaml):

- `DEEPSEEK_API_KEY` — required by DSH
- `DSH_MODEL` — defaults to `deepseek-flash`; use `deepseek-v4-pro` for a
  stronger model
- `DSH_REASONING_EFFORT` — `off`, `low`, `high`, or `max`
- `ETHERSCAN_API_KEY`, `ETHERSCAN_API_URL` — direct Etherscan v2 access
- `RPC_URL` — archive RPC for fork tests and fuzzing

No Mongo or Codex/OpenRouter values are used. Remove stale keys from existing
secrets before applying.

For the default in-cluster Redis, `redis://agent-audit-redis:6379/0` is enough
in the dispatcher secret.

If the GHCR images are private, create a registry pull secret named
`agent-audit-registry` in the `agent-audit` namespace and set
`DISPATCHER_RUNNER_IMAGE_PULL_SECRET` accordingly.

## Deploy

```bash
k3s kubectl apply -f k3s/namespace.yaml
k3s kubectl apply -f k3s/runner-secret.yaml
k3s kubectl apply -f k3s/dispatcher-secret.yaml
k3s kubectl apply -k k3s/
```

If you change the runner ConfigMap later, restart the dispatcher so it reloads
the Job template:

```bash
k3s kubectl -n agent-audit rollout restart deploy/agent-audit-dispatcher
k3s kubectl -n agent-audit rollout status deploy/agent-audit-dispatcher
```

Watch the control plane:

```bash
k3s kubectl -n agent-audit get deploy,pods
k3s kubectl -n agent-audit get jobs,pods -w
```

## Submit tasks

Each Redis message carries a `task_id`, one complete prompt, and an optional
image override. `task_id` is the idempotency key and becomes the run directory
name after normalization.

```bash
k3s kubectl -n agent-audit exec deploy/agent-audit-redis -- \
  redis-cli XADD agent-audit:tasks '*' \
    task_id audit-20261026-001 \
    full_prompt 'Audit 0x0000000000000000000000000000000000000000 on eth.' \
    image ghcr.io/lunaticabs/agent-audit:rebuild
```

Batch submission from an address list:

```bash
k3s kubectl -n agent-audit port-forward svc/agent-audit-redis 6380:6379
```

```bash
python3 scripts/enqueue_redis.py \
  --chain eth \
  --address-file scripts/addresses/addrs.txt \
  --host 127.0.0.1 \
  --port 6380 \
  --image ghcr.io/lunaticabs/agent-audit:rebuild
```

The default prompt template is:

```text
Audit {address} on {chain}.
```

DSH's skill catalog triggers the `evm-audit` skill from its description, so the
prompt can stay minimal and needs no routing text.

Preview without sending:

```bash
python3 scripts/enqueue_redis.py --chain eth --dry-run
```

## Read results

Use Kubernetes Job/Pod state for execution state and the host run directory for
audit artifacts:

```bash
k3s kubectl -n agent-audit describe job agent-audit-audit-20261026-001
sudo ls -la /var/lib/agent-audit/runs/audit-20261026-001
```

Each run contains:

```text
input/prompt.txt
input/request.json
input/run_meta.json          # status, exit code, timings (written on exit)
artifacts/                   # evidence the agent produced
sources/                     # optional fetched source
reports/final_report.json    # required report
reports/final_answer.md      # stdout captured from DSH
logs/agent.stdout.log
logs/agent.stderr.log
logs/stats.log               # stats collector output
stats/metrics.json           # derived post-run metrics
```

DSH session files live under the image's `DSH_HOME`, outside the run directory;
the audit evidence and final answer are captured in `runs/`.

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

A CronJob mounting the same hostPath can run this on a schedule. The dispatcher
now injects `AGENT_AUDIT_IMAGE`, so results can be stratified by image tag; add
a `batch_id` field to the Redis message when you need per-batch completion.
See [docs/audit-statistics.md](../docs/audit-statistics.md) for metric
definitions and failure modes.

## Operational notes

- The dispatcher derives a stable Job name from `task_id` and will not create a
  duplicate if the Job already exists.
- `FULL_PROMPT`, `TASK_ID`, and `RUN_ID` are injected into the Job pod.
- Finished Jobs and Pods clean themselves up with `ttlSecondsAfterFinished`.
  The hostPath run directory is not deleted.
- Effective parallelism is bounded by node CPU/memory and the runner Job
  requests/limits, not by a dispatcher-level queue cap.
- `hostPath` is node-local. This is intentional for the current single-node k3s
  deployment. All Jobs on the node share the same runs root, so a runner can
  read sibling run directories; use per-run volumes if audits need tenant
  isolation. For a multi-node cluster, replace it with a shared ReadWriteMany
  volume or a per-node collector; do not assume runs from different nodes are
  in the same directory.
- A runtime image update is a tag update plus `kubectl apply -k k3s/`; existing
  Jobs keep their image until replaced.
