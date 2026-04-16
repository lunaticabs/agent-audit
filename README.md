# agent-audit

`agent-audit` 是一个 `Codex-first` 的 EVM 合约审计工作区。

边界现在是：

- Python CLI 只负责准备审计材料
- skills 负责告诉 agent 什么时候用哪个工具
- agent 自己决定审计顺序、关注点和是否继续做动态验证

仓库内 CLI 当前只做这些确定性步骤：

- 初始化 `runs/<run_id>/`
- 抓已验证源码
- 生成轻量 IR
- 运行依赖发现与依赖专项分析
- 聚合中立材料清单

它不再负责：

- 保存或注入额外提示词
- 一键替 agent 编排整条审计流程
- 生成“最终审计结论”式报告

## Quick Start

1. 创建虚拟环境并安装依赖

```bash
uv venv --python 3.11
source .venv/bin/activate
uv sync
```

2. 配置环境变量

```bash
cp .env.example .env
```

`fetch-source` 现在支持更多 Etherscan V2 系链。通常你可以继续使用同一个 V2 API base URL，
只通过 `--chain` 或 `input/request.json` 里的链名切换目标链。

例如常见主网别名包括：

- `eth`
- `base`
- `arb`
- `op`
- `polygon`
- `bsc`
- `avax`
- `linea`
- `blast`
- `scroll`
- `mantle`
- `gnosis`
- `celo`
- `zksync`

也支持常见测试网别名，例如：

- `sepolia`
- `base-sepolia`
- `arb-sepolia`
- `op-sepolia`
- `amoy`
- `fuji`
- `linea-sepolia`
- `scroll-sepolia`

3. 分步准备审计材料

```bash
UV_CACHE_DIR=/tmp/uv-cache uv run agent-audit init-run \
  --chain eth \
  --address 0x0000000000000000000000000000000000000000

UV_CACHE_DIR=/tmp/uv-cache uv run agent-audit fetch-source --run-id <run_id>
UV_CACHE_DIR=/tmp/uv-cache uv run agent-audit build-ir --run-id <run_id>
UV_CACHE_DIR=/tmp/uv-cache uv run agent-audit run-dependency --run-id <run_id>
UV_CACHE_DIR=/tmp/uv-cache uv run agent-audit aggregate-materials --run-id <run_id>
```

`fetch-source` 成功后会自动准备 `slither_project/` 兼容工作区。

如果后面需要手动重建这个工作区，再执行：

```bash
UV_CACHE_DIR=/tmp/uv-cache uv run agent-audit prepare-slither --run-id <run_id>
```

4. 在 Codex 中使用 workflow skill，再按需调用独立工具 skills

```text
Use $smart-contract-audit to inspect 0x0000000000000000000000000000000000000000 on eth.
Prepare review materials step by step, inspect runs/<run_id>/reports/materials_manifest.json,
and decide which tools to run next.
```

主入口说明现在在：

- `AGENTS.md`

当前和 Slither 稳定运行相关的设计文档：

- `docs/slither_project_design.md`

## 技能结构

- `AGENTS.md`
  主 workflow 入口
- `.agents/skills/audit-workspace/`
  初始化 run 工作区
- `.agents/skills/audit-source-fetch/`
  verified source 抓取
- `.agents/skills/audit-chain-checks/`
  直接 `cast` 链上检查
- `.agents/skills/audit-ir-builder/`
  轻量 IR 生成
- `.agents/skills/audit-dependency-scan/`
  外部依赖发现与依赖专项分析
- `.agents/skills/audit-slither/`
  Slither 静态分析
- `.agents/skills/audit-materials/`
  中立材料聚合
- `.agents/skills/audit-echidna/`
  Echidna 直接使用说明
- `.agents/skills/foundry-forge/`
  Foundry `forge` 的 build/test/script 使用说明
- `.agents/skills/foundry-cast/`
  Foundry `cast` 的 RPC/ABI/链上交互使用说明
- `.agents/skills/foundry-anvil/`
  Foundry `anvil` 的本地节点和 fork 使用说明
- `.agents/skills/foundry-chisel/`
  Foundry `chisel` 的 Solidity REPL 使用说明

## 主要产物

- `input/request.json`
- `artifacts/source_bundle.json`
- `ir/contracts.json`
- `ir/functions.json`
- `ir/privilege_matrix.json`
- `artifacts/dependency_findings.json`
- `reports/materials_manifest.json`
- `slither_project/build_manifest.json`

## 当前原则

- 仓库侧 finding 材料直接看 `artifacts/dependency_findings.json`
- Python 只负责把材料准备出来
- chain checks、Slither、Echidna、forge、cast、anvil 这类工具由 agent 自己决定是否直接调用
- 审计结论、深入验证顺序、是否调用这些工具，交给 agent 决定

TODO:
测试非顶级模型的审计效果
