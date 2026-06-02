# agent-audit

`agent-audit` 是一个 agent-first 的智能合约审计系统。项目把一次 EVM 合约审计拆成四层：

1. `src/` 下的 Rust harness CLI 负责创建 run、拉取源码、准备工具工程、聚合审计材料并归档。
2. `docker/` 下的容器化审计环境把 CLI、Codex SDK、Codex CLI 和常用合约安全工具打包成单次审计 runner。
3. `k3s/` 下的容器级并行部署系统用 Redis 队列和 Kubernetes Job 批量运行多个 runner。
4. `eval_docker/` 与 `EVMbench/` 下的评估系统复用审计环境，但改成面向 EVMbench Detect 的本地仓库评测适配器。

根 README 介绍整体结构和工作流程。更细的构建、部署和运行细节见各子目录 README。

## 系统结构

| 目录 | 角色 | 主要产物 |
| --- | --- | --- |
| `src/` | 本地 harness CLI | `agent-audit` 二进制、`runs/<run_id>/` 工作区、JSON 步骤输出 |
| `docker/` | 生产审计 runner 镜像 | `agent-audit:<tag>`，一次容器运行处理一个完整 prompt |
| `k3s/` | 批量并行执行层 | Redis Stream、dispatcher、每个任务一个 Kubernetes Job |
| `eval_docker/` | EVMbench 专用评测镜像 | `agent-audit-eval:<tag>`，输出 `submission/audit.md` |
| `EVMbench/` | EVMbench overlay | `agent-audit-codex` adapter 与 Dockerfile patch 脚本 |
| `dispatcher/` | k3s dispatcher 程序 | `agent-audit-dispatcher` 镜像，把 Redis 消息转成 Job |

整体数据流：

```text
生产单次审计:
  prompt -> docker runner -> Codex agent -> agent-audit CLI -> runs/<run_id>/ -> sync-run -> Mongo

生产批量审计:
  address list -> Redis Stream -> dispatcher -> Kubernetes Jobs -> docker runner -> Mongo

EVMbench Detect:
  EVMbench audit repo -> eval runner -> Codex agent + local tools -> submission/audit.md
```

## 1. `src/`: Harness CLI

`src/` 是审计系统的数据准备和归档核心。它不直接替 agent 做审计判断，而是为 agent 准备可重复读取的材料，并用结构化 JSON 记录每一步状态。

CLI 入口：

```bash
agent-audit <subcommand>
```

本地开发时也可以直接运行：

```bash
cargo run --bin agent-audit -- <subcommand>
```

当前命令：

```text
agent-audit init-run --chain <chain> --address <address>
agent-audit fetch-source --run-id <run_id>
agent-audit run-dependency --run-id <run_id>
agent-audit prepare-slither --run-id <run_id>
agent-audit prepare-tooling --run-id <run_id>
agent-audit aggregate-materials --run-id <run_id>
agent-audit sync-run --run-id <run_id>
```

核心职责：

- 创建 `runs/<run_id>/` 工作区和 `input/request.json`、`input/run_meta.json`。
- 通过 `AGENT_AUDIT_SOURCE_API_BASE` 等配置拉取目标合约源码。
- 做依赖发现和依赖分析，写入 `artifacts/dependency_*.json`。
- 准备 Slither、Foundry、Echidna 等工具工作区和 build manifest。
- 聚合审计材料到 `reports/materials_manifest.json`。
- 通过 `sync-run` 将 run 元数据和文件同步到 MongoDB。

常用配置来自项目根目录 `.env`：

```text
AGENT_AUDIT_DEFAULT_CHAIN
AGENT_AUDIT_SOURCE_API_BASE
AGENT_AUDIT_SOURCE_API_KEY
AGENT_AUDIT_SOURCE_HEADERS_JSON
AGENT_AUDIT_RPC_URL
AGENT_AUDIT_MONGO_URI
AGENT_AUDIT_MONGO_DB
AGENT_AUDIT_RUNS_DIR
```

`init-run` 会执行完整准备流程：拉源码、依赖分析、准备工具工程、聚合材料。其他子命令用于对已有 run 单步重跑或补充材料。

## 2. `docker/`: 容器化审计环境

`docker/` 将生产审计所需环境封装为一个 runner 镜像。镜像内包含：

- `agent-audit` CLI
- Node.js、`@openai/codex-sdk`、`codex`
- Slither、`solc-select`
- Foundry: `forge`、`cast`、`anvil`
- Echidna
- 容器专用 `.codex` 配置和审计技能

构建：

```bash
./docker/build.sh
```

运行单次审计：

```bash
./docker/run.sh --prompt "Check AGENTS.md and audit 0x0000000000000000000000000000000000000000 on eth."
```

也可以直接调用 Docker：

```bash
docker run --rm \
  -v "$(pwd)/.env:/opt/agent-audit/.env:ro" \
  -e FULL_PROMPT="Check AGENTS.md and audit 0x0000000000000000000000000000000000000000 on eth." \
  agent-audit:0.1
```

生产 runner 只接受一个完整 prompt：

- `FULL_PROMPT` 环境变量
- 或 `--prompt "..."` 参数

容器不会自行拼接 `address`、`chain`、`instructions`。调度方需要提前把目标、链、审计要求和输出要求写进 prompt。容器启动后，Codex agent 会读取 `AGENTS.md`，按需要调用 `agent-audit` CLI 和安全工具，最终完成审计与归档。

更多细节见 [`docker/README.md`](docker/README.md)。

## 3. `k3s/`: 容器级并行部署

`k3s/` 是单服务器 Kubernetes 部署方案，用来把多个单次 runner 并行化。它不改变审计逻辑，只负责把任务分发成多个一次性 Job。

拓扑：

```text
Redis Stream
  -> agent-audit-dispatcher Deployment
  -> one Kubernetes Job per task
  -> agent-audit runner container
  -> sync-run / Mongo archive
```

关键组件：

- `k3s/redis.yaml`: 输入队列。
- `dispatcher/`: Redis Stream 到 Kubernetes Job 的桥接程序。
- `k3s/dispatcher-*.yaml`: dispatcher 的 Deployment、RBAC、配置和 Secret。
- `k3s/runner-configmap.yaml`: runner Job 模板，包括镜像、资源、TTL、`runs/` volume。
- `k3s/runner-secret.yaml`: runner 所需 API key、source/RPC/Mongo 配置。

部署入口：

```bash
k3s kubectl apply -f k3s/namespace.yaml
k3s kubectl apply -f k3s/runner-secret.yaml
k3s kubectl apply -f k3s/dispatcher-secret.yaml
k3s kubectl apply -k k3s/
```

提交单个任务：

```bash
k3s kubectl -n agent-audit exec deploy/agent-audit-redis -- \
  redis-cli XADD agent-audit:tasks '*' \
    task_id audit-20260505-001 \
    full_prompt 'Check AGENTS.md and audit 0x0000000000000000000000000000000000000000 on eth.' \
    image ghcr.io/lunaticabs/agent-audit:main
```

批量提交地址列表：

```bash
k3s kubectl -n agent-audit port-forward svc/agent-audit-redis 6380:6379
python3 scripts/enqueue_redis.py \
  --chain eth \
  --address-file scripts/addresses/addrs.txt \
  --host 127.0.0.1 \
  --port 6380 \
  --image ghcr.io/lunaticabs/agent-audit:main
```

运行状态以 Kubernetes Job/Pod 为准：

```bash
k3s kubectl -n agent-audit get jobs,pods -w
```

Redis 是输入队列，不是最终状态存储。审计证据和 run 文件通过 runner 内部的 `sync-run` 流程写入 MongoDB。

更多细节见 [`k3s/README.md`](k3s/README.md)。

## 4. `eval_docker/` 和 `EVMbench/`: 评估系统

评测系统面向 EVMbench Detect。它复用生产镜像里的 Codex 和安全工具，但数据源从“链上地址 + 源码服务”切换为“EVMbench 挂载的本地审计仓库”。

`eval_docker/` 提供评测 runner：

```bash
./eval_docker/build.sh
APIAPI_API_KEY=... ./eval_docker/run.sh --audit-dir /path/to/audit
```

评测 runner 的规则：

- Codex 工作目录是 benchmark 提供的 audit repo。
- 最终输出必须写到 `submission/audit.md`。
- 生产地址/链数据面被刻意禁用。
- `agent-audit init-run`、`fetch-source`、`sync-run` 等生产 pipeline 命令在评测环境中被 guard 阻止。
- 允许在 benchmark 本地仓库下创建临时 notes、logs、tool outputs，但 grader 只读取 `submission/audit.md`。

`EVMbench/` 提供 overlay 脚本，把 `agent-audit-codex` adapter 注入到 EVMbench checkout：

```bash
./EVMbench/overlay.sh --evmbench-dir /path/to/evmbench
```

然后在 EVMbench 仓库中构建并运行 Detect：

```bash
cd /path/to/evmbench
uv sync
docker build -f ploit/Dockerfile -t ploit-builder:latest --target ploit-builder .
uv run docker_build.py --split debug

APIAPI_API_KEY=... \
uv run python -m evmbench.nano.entrypoint \
  evmbench.audit_split=debug \
  evmbench.mode=detect \
  evmbench.apply_gold_solution=False \
  evmbench.log_to_run_dir=True \
  evmbench.solver=evmbench.nano.solver.EVMbenchSolver \
  evmbench.solver.agent_id=agent-audit-codex \
  runner.concurrency=1
```

更多细节见 [`eval_docker/README.md`](eval_docker/README.md) 和 [`EVMbench/README.md`](EVMbench/README.md)。

## 工作流程

### A. 本地开发和材料准备

```text
cargo build
  -> agent-audit init-run
  -> runs/<run_id>/input
  -> runs/<run_id>/artifacts
  -> runs/<run_id>/slither_project / foundry_project / echidna_project
  -> runs/<run_id>/reports/materials_manifest.json
```

这个流程用于验证 CLI、source provider、依赖分析和材料聚合是否正确。agent 可以在准备好的 run workspace 中继续调用 Slither、Foundry、Echidna、cast/anvil 等工具。

### B. 单次生产审计

```text
docker runner
  -> Codex reads FULL_PROMPT and AGENTS.md
  -> Codex calls agent-audit CLI to prepare evidence
  -> Codex runs security tools as needed
  -> Codex writes report / findings
  -> agent-audit sync-run archives run files to Mongo
```

这个流程适合手动审计、调试 prompt、验证镜像内容和复现单个目标。

### C. 批量并行审计

```text
address file or external scheduler
  -> Redis Stream message(task_id, full_prompt, image)
  -> dispatcher
  -> Kubernetes Job
  -> same docker runner as single audit
  -> Mongo archive
```

这个流程适合大批量地址扫描。并行度主要由 k3s 调度能力、runner Job 的 CPU/内存 requests/limits 以及节点资源决定。

### D. EVMbench Detect 评测

```text
EVMbench task
  -> mounted local audit repository
  -> eval runner
  -> Codex uses local tools only
  -> submission/audit.md
```

这个流程用于 benchmark，不会访问生产 source/RPC/Mongo 数据面，也不会用地址/链 pipeline 生成审计材料。

## 开发检查

代码修改后推荐运行：

```bash
cargo xtask check
```

等价命令：

```bash
cargo run -p xtask -- check
```

固定检查顺序：

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

文档或部署改动也应至少确认相关 README 和脚本命令仍与子目录文档一致。
