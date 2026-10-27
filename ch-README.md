# agent-audit

`agent-audit` 是一个 agent-first 的智能合约审计系统。DeepSeek Harness 在容器内自主完成一次审计；容器已经打包链上数据访问、源码获取和安全工具。两种部署方式共用同一个镜像：

- **单次审计**：`docker/run.sh` 为一个完整 prompt 拉起一个容器。
- **并行审计**：k3s dispatcher 把 Redis 消息转换成一次性 Kubernetes Job。

没有数据库归档，没有 Rust 审计 CLI，也没有固定审计流水线。run 目录就是持久化产物；agent 通过 Etherscan、RPC 和已安装工具直接工作。

## 本次重构

- agent 从 Codex 换成 **DeepSeek Harness headless CLI 模式**，API 换成 DeepSeek 官方接口（`deepseek-official`），不再使用 OpenRouter。
- 删除旧 Rust `agent-audit` 流水线（`src/`）。源码拉取、依赖启发式、材料聚合和 Mongo 归档全部移除，改成 agent 直接调用 Etherscan v2 / RPC，加一个很小的容器 entrypoint。
- 删除 `eval_docker/` 和 `EVMbench/`。这个仓库只做审计，不做 agent 评测。
- 删除按工具/流程拆分的 skill，只保留一个主 `evm-audit` skill 和可用工具声明。审计方法由 agent 自己决定。
- 删除 MongoDB 归档。每次审计写入 `runs/<run_id>/`；Docker 直接 bind mount，k3s 使用节点本地 hostPath。
- 保留原有链集合、容器工具列表和 k3s 大规模部署能力。

## 架构

```text
单次审计：
  prompt -> runner 容器 -> DSH headless -> Etherscan/RPC/工具
         -> runs/<run_id>/ -> 宿主机 runs/（bind mount）

批量审计：
  地址列表 -> Redis Stream -> dispatcher -> Kubernetes Job
           -> 同一个 runner 镜像 -> /var/lib/agent-audit/runs/<run_id>
```

| 目录 | 作用 |
| --- | --- |
| `docker/` | 运行镜像、单次审计入口、DSH 指令和主 skill |
| `k3s/` | Redis 队列、dispatcher Deployment、runner Job 模板、hostPath runs |
| `dispatcher/` | Rust 实现的 Redis → Kubernetes Job 桥 |
| `scripts/` | 批量入队脚本和地址列表 |
| `xtask/` | 格式化、lint、测试检查 |
| `docs/` | 长报告 |

## 快速开始：单次审计

需要 Docker 和 DeepSeek API key；强烈建议再配置 Etherscan key 和 archive RPC，闭源合约与 fork 测试需要 RPC。

```bash
cp .env.example .env
# 填写 DEEPSEEK_API_KEY、ETHERSCAN_API_KEY、RPC_URL
./docker/build.sh
./docker/run.sh --prompt "Audit 0x0000000000000000000000000000000000000000 on eth."
```

容器只接受一个完整 prompt，通过 `FULL_PROMPT` 或 `--prompt` 传入。目标地址、链、范围和输出要求都由调用方写进 prompt。细节见 [docker/README.md](docker/README.md)。

完成后的 run 目录：

```text
input/prompt.txt              # 调用方请求
input/request.json            # 规范化请求
input/run_meta.json           # 状态、退出码、耗时
artifacts/                    # agent 产出的证据
sources/                      # 可选：拉取到的源码
reports/final_report.json     # 必须的结构化报告
reports/final_answer.md       # DSH stdout
logs/agent.stdout.log
logs/agent.stderr.log
```

entrypoint 在退出时自动写 metadata、保存 stdout 并 `sync`。agent 不需要、也没有持久化命令可调用。

## 审计能力

**开源/已验证合约。** agent 直接调用 Etherscan v2 获取源码、ABI、编译器设置、proxy 信息和历史交易；依赖由 agent 自己从源码、仓库和链上数据解析。按需使用 `slither`、`forge`、`cast`、`anvil`、`solc-select`、`echidna`。

**闭源/未验证合约。** agent 获取 runtime bytecode，解析 proxy 实现，使用 Heimdall 反汇编/反编译，恢复 selector，并用 Foundry 或 Echidna 在 fork 上验证具体经济性质。反编译结果只作为启发式证据。

**链。** 保留原有 alias 集合：Ethereum 及测试网、BNB、Polygon、Base、Arbitrum、OP、Avalanche、Linea、Blast、Scroll、Mantle、Gnosis、Celo、zkSync、opBNB、Moonbeam/Moonriver、BitTorrent、Fraxtal、Sonic、Sei、Taiko、Unichain、World Chain。完整表见 [docker/dsh/skills/evm-audit/references/chains.md](docker/dsh/skills/evm-audit/references/chains.md)。

**工具。** 镜像保留 Node/DSH、Slither、`solc-select`、Foundry（`forge`/`cast`/`anvil`）、Echidna、Heimdall，并加入 `jq` 方便直接处理 JSON API。工具列表和命令写在主 skill 里；不再有 slither/forge/anvil/echidna 等独立 skill。

**数据访问。** agent 直接拿到 `ETHERSCAN_API_KEY`、`ETHERSCAN_API_URL`、`RPC_URL`，用 `curl`/`cast` 调用。旧的依赖启发式发现和 source-provider CLI 不再参与。

## Skill 设计

`docker/dsh/skills/evm-audit/SKILL.md` 是唯一审计 skill。

根文件只是一个很短的 router：工作区/输出约定、几个非显而易见的检查点、以及 references 指针。细节按需加载：

- `references/tools.md` — Etherscan v2、RPC、Foundry、Slither、Echidna、Heimdall；
- `references/report.md` — 报告结构和证据要求；
- `references/chains.md` — 支持的链别名和 chain ID。

主 skill 就是本次 run 的入口。DSH 把 skill 放进 catalog，审计任务通过 description 触发它；没有 role prompt、没有强制 workflow，镜像内也没有单独的 AGENTS.md。根文件保持很短，只做路由，工具、链和报告细节放在 references 里。

## k3s 部署

```bash
cp k3s/runner-secret.example.yaml k3s/runner-secret.yaml
cp k3s/dispatcher-secret.example.yaml k3s/dispatcher-secret.yaml
# 填写 DEEPSEEK_API_KEY、ETHERSCAN_API_KEY、RPC_URL
sudo mkdir -p /var/lib/agent-audit/runs
k3s kubectl apply -f k3s/namespace.yaml
k3s kubectl apply -f k3s/runner-secret.yaml
k3s kubectl apply -f k3s/dispatcher-secret.yaml
k3s kubectl apply -k k3s/
```

提交单个任务：

```bash
k3s kubectl -n agent-audit exec deploy/agent-audit-redis -- \
  redis-cli XADD agent-audit:tasks '*' \
    task_id audit-20261026-001 \
    full_prompt 'Audit 0x0000000000000000000000000000000000000000 on eth.'
```

批量提交：

```bash
python3 scripts/enqueue_redis.py \
  --chain eth \
  --address-file scripts/addresses/addrs.txt \
  --image ghcr.io/lunaticabs/agent-audit:v2.0.0
```

每个 Job 挂载节点本地 `hostPath` 到 `/opt/agent-audit/runs`，Pod 清理后 run 仍在。完整部署、secret 和多节点注意事项见 [k3s/README.md](k3s/README.md)。

## 开发检查

Rust 侧只剩 dispatcher 和 xtask：

```bash
cargo xtask check
```

等价命令：

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

Docker 打包检查：

```bash
docker build -f docker/Dockerfile --target smoke-test -t agent-audit:smoke-test .
```

## 历史结果

旧流水线对 8,255 个以太坊主网合约做了端到端扫描，确认 6,466 个漏洞，并在 432 个合约上基于 fork 复现了攻击。报告保留在 [docs/chain-scan-report.md](docs/chain-scan-report.md)。

## 安全说明

- 容器就是隔离边界。DSH 在容器内拥有完整文件访问权限，且不需要交互式审批。k3s 下所有 Job 共享节点本地 runs 根目录，因此 runner 可以读取同节点的其他 run；需要租户隔离时改用 per-run volume。
- `.env` 保存 DeepSeek、Etherscan、RPC 凭据。不要提交，也不要挂载到不信任的容器。
- DSH home patch 已关闭 session-log 上传和 package-inventory 上传扩展；遥测默认关闭。
- `runs/` 包含审计证据，可能包含敏感目标信息。
