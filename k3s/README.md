# k3s Parallel Audit Deployment

This directory is the single-server `k3s` deployment for running multiple one-shot `agent-audit` tasks.

Topology:

- one `Redis` deployment for the submission queue
- one `agent-audit-dispatcher` deployment that bridges Redis Stream messages into `Job` objects
- one Kubernetes `Job` per audit task
- `Mongo` remains the durable archive through the existing `sync-run` flow

There is no long-running `agent-audit` Deployment. The runner exists only as one-shot Jobs that the dispatcher creates from the template settings in [runner-configmap.yaml](/Users/lunaticabs/code/agent-audit/k3s/runner-configmap.yaml).

Even though this is scoped to `k3s`, it still uses the standard Kubernetes APIs that `k3s` exposes: `Deployment`, `Service`, `Job`, `Secret`, `ConfigMap`, and `RBAC`.

## Build Images

You can build locally:

```bash
./docker/build.sh
```

```bash
./docker/build-dispatcher.sh
```

Or use GitHub Actions + GHCR:

- push the repository to GitHub
- let [.github/workflows/publish-images.yml](/Users/lunaticabs/code/agent-audit/.github/workflows/publish-images.yml) build and publish both images
- after the first successful publish, set both GHCR packages to `public` for the simplest k3s pull path

The workflow publishes:

- `ghcr.io/<owner>/agent-audit:<tag>`
- `ghcr.io/<owner>/agent-audit-dispatcher:<tag>`

The current manifests are wired for your `rebuild` branch tags:

- `ghcr.io/lunaticabs/agent-audit:rebuild`
- `ghcr.io/lunaticabs/agent-audit-dispatcher:rebuild`

Set image addresses in two places before applying if you later switch away from `rebuild`:

- runner image in [runner-configmap.yaml](/Users/lunaticabs/code/agent-audit/k3s/runner-configmap.yaml)
- dispatcher image in [dispatcher-deployment.yaml](/Users/lunaticabs/code/agent-audit/k3s/dispatcher-deployment.yaml)

These manifests intentionally use the moving `rebuild` tag plus `imagePullPolicy: Always` so you do not need to commit a new digest after each publish. If you later move to immutable deploys, replace those tags with a concrete `sha-...` tag or image digest.

If you want to check the currently configured image references:

```bash
rg -n "ghcr.io/.*/agent-audit" k3s
```

## Prepare Secrets

Copy [runner-secret.example.yaml](/Users/lunaticabs/code/agent-audit/k3s/runner-secret.example.yaml) to `k3s/runner-secret.yaml` and fill in:

- `APIAPI_API_KEY`
- every required `AGENT_AUDIT_*`
- `AGENT_AUDIT_MONGO_URI`

Copy [dispatcher-secret.example.yaml](/Users/lunaticabs/code/agent-audit/k3s/dispatcher-secret.example.yaml) to `k3s/dispatcher-secret.yaml`. For the default in-cluster Redis service, `redis://agent-audit-redis:6379/0` is enough.

Host-side `k3s` reservations are separate from Pod requests and limits. The current recommendation for your single server is captured in [server-config.example.yaml](/Users/lunaticabs/code/agent-audit/k3s/server-config.example.yaml):

- reserve `4` CPU cores and `16Gi` memory for the host via `system-reserved`
- keep `traefik` and `servicelb` disabled

Apply that file to `/etc/rancher/k3s/config.yaml` on the server and restart `k3s`:

```bash
sudo install -d /etc/rancher/k3s
sudo cp k3s/server-config.example.yaml /etc/rancher/k3s/config.yaml
sudo systemctl restart k3s
```

If you keep the GHCR images `public`, no image pull secret is required.

If you keep them `private`, create a registry pull secret named `agent-audit-registry` in the `agent-audit` namespace:

```bash
k3s kubectl create secret docker-registry agent-audit-registry \
  --namespace agent-audit \
  --docker-server ghcr.io \
  --docker-username "$GITHUB_USER" \
  --docker-password "$GHCR_READ_PACKAGES_TOKEN"
```

For private images:

- add an `imagePullSecrets` stanza back into [dispatcher-deployment.yaml](/Users/lunaticabs/code/agent-audit/k3s/dispatcher-deployment.yaml)
- set `DISPATCHER_RUNNER_IMAGE_PULL_SECRET` to `agent-audit-registry` in [runner-configmap.yaml](/Users/lunaticabs/code/agent-audit/k3s/runner-configmap.yaml)

## Deploy

Apply the namespace first:

```bash
k3s kubectl apply -f k3s/namespace.yaml
```

Apply your filled secrets:

```bash
k3s kubectl apply -f k3s/runner-secret.yaml
k3s kubectl apply -f k3s/dispatcher-secret.yaml
```

Apply the rest:

```bash
k3s kubectl apply -k k3s/
```

If you change [runner-configmap.yaml](/Users/lunaticabs/code/agent-audit/k3s/runner-configmap.yaml) later, restart the dispatcher so it reloads the new Job template environment:

```bash
k3s kubectl -n agent-audit rollout restart deploy/agent-audit-dispatcher
k3s kubectl -n agent-audit rollout status deploy/agent-audit-dispatcher
```

Watch the control plane:

```bash
k3s kubectl -n agent-audit get deploy,pods
```

## Submit Tasks

Each Redis message contains a `task_id`, one complete prompt, and an optional image override. The dispatcher does not assemble prompts and does not write task state back into Redis.

Example:

```bash
k3s kubectl -n agent-audit exec deploy/agent-audit-redis -- \
  redis-cli XADD agent-audit:tasks '*' \
    task_id audit-20260505-001 \
    full_prompt 'Check AGENTS.md and audit 0x0000000000000000000000000000000000000000 on eth. Focus on upgradeability and authz.' \
    image registry.example.com/agent-audit:0.1
```

Track jobs and pods from `k3s` directly:

```bash
k3s kubectl -n agent-audit get jobs,pods -w
```

Inspect one Job in detail:

```bash
k3s kubectl -n agent-audit describe job agent-audit-audit-20260505-001
```

Use `Job.status` and `Pod.status` as the source of truth for runtime state. Redis is input-only.

## Operational Notes

- `task_id` is the idempotency key. The dispatcher derives a stable Job name from it and will not create duplicates when the Job already exists.
- `FULL_PROMPT` is injected into the Job pod as an environment variable. The runner image no longer accepts `address/chain/instructions` fields.
- `runs/` lives on an `emptyDir` mounted at `/opt/agent-audit/runs`, which is sufficient for a single-node one-shot Job lifecycle.
- `ttlSecondsAfterFinished` is enabled so finished Jobs and Pods clean themselves up automatically.
- Runner Job settings such as image, TTL, resources, pull policy, and `runs/` volume size live in [runner-configmap.yaml](/Users/lunaticabs/code/agent-audit/k3s/runner-configmap.yaml).
- The current runner template requests `500m` CPU and `1Gi` memory, with limits of `2000m` CPU and `4Gi` memory.
- Dispatcher settings such as Redis stream, consumer group, and block timeout live in [dispatcher-configmap.yaml](/Users/lunaticabs/code/agent-audit/k3s/dispatcher-configmap.yaml).
- The dispatcher does not enforce a separate queue-level concurrency cap. Effective parallelism comes from `k3s` scheduling plus the runner Job `requests`/`limits` in [runner-configmap.yaml](/Users/lunaticabs/code/agent-audit/k3s/runner-configmap.yaml).
