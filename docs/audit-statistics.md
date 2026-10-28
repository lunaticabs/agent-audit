# 审计统计：指标口径与 k3s 落地方案

本文说明一轮审计结束后统计什么、为什么这样统计，以及在 k3s 上如何收集与聚合。
工具实现见 [tools/audit_stats/README.md](../tools/audit_stats/README.md)。

## 1. 约束

- **run 目录是唯一真相。** 没有 MongoDB、没有中央队列状态。`runs/<run_id>/` 里的
  `input/`、`reports/`、`artifacts/`、`test/` 是审计证据；统计只能从这些文件派生。
- **报告是 agent 写的。** `final_report.json` 是不可信输入：字段会漂移、严重度词表会
  漂移、证据路径可能不存在。统计工具必须归一化、记录异常，并且永不因为报告坏了而崩溃。
- **统计是派生物。** `stats/metrics.json` 可以随时删除重建；它不进入 git，也不影响审计退出码。
- **k3s 的 run 卷是 node-local hostPath。** 没有共享存储时，任何"中央 pod 直接扫
  `/var/lib/agent-audit/runs`"的方案都只对单节点成立；多节点必须按节点收集再汇总。

## 2. 一份 run 里真正可算的东西

| 数据 | 位置 | 可支撑的指标 |
| --- | --- | --- |
| 运行元数据 | `input/run_meta.json` | 状态、退出码、模型、推理档位、起止时间、时长 |
| 结构化报告 | `reports/final_report.json` | 漏洞数、严重度、置信度、假设、局限、声明的覆盖 |
| 证据引用 | 报告 `findings[].evidence` | 证据解析率、缺失/越界引用 |
| 复现命令 | 报告 `findings[].reproduction` | 复现是否落到具体测试、测试是否通过 |
| Foundry harness | `test/**/*.t.sol` | 测试数、fuzz 测试、是否 fork、测试名清单 |
| forge 输出 | `artifacts/*forge*.log` | pass/fail/skip、fuzz runs、与 harness 的一致性 |
| 工具产物 | `artifacts/slither*`、`heimdall/`、`echidna*`、`trace_*` | 工具真实使用情况、静态信号数 |
| 执行痕迹 | `artifacts/trace_*.log` 里的 `地址::函数名(...)` | 目标函数级覆盖（trace 来源） |
| ABI | `sources/*.abi.json`、`artifacts/etherscan_getabi.json` | 目标函数全集、覆盖分母 |
| 源码/字节码 | `sources/`、`artifacts/bytecode_match.json` | 源码可得性、字节码匹配结论 |
| 体积 | 目录大小 | 单轮成本代理（artifacts/sources 字节数） |

**取不到的东西要诚实标注。** 目前 token / 费用不在 run 目录里：DSH 的会话日志在
`DSH_HOME`，k3s Pod 结束后丢失。要么在容器退出前把 usage 摘要写进 `stats/usage.json`，
要么在 DSH 适配器层计数。统计工具预留了字段位置，但在拿不到之前不编造。

## 3. 指标目录

### A. 产出（"发现了多少"）

- 漏洞总数；按严重度（critical/high/medium/low/informational/unknown）分布；
  按置信度（confirmed/likely）分布。
- 每轮平均漏洞数、单轮最高严重度分布。
- 高危+（critical+high）漏洞数；有高危发现的 run 数。
- `hypotheses` 数与 `limitations` 数：模型如何自我修正、如何声明边界。
- 按合约地址去重后的漏洞数（同一地址多次审计取最严重一次）。

### B. 证据与复现（"结论有多硬"）

- **证据解析率**：先剥离 `:行号`、行号区间和尾注，再判断报告引用的路径是否真的存在于 run 目录内。
- **无证据发现数**：`findings` 里 `evidence` 为空的条目。
- **复现链接率**：`reproduction` 提到的测试名是否存在于 harness。
- **复现通过率**：链接到的测试是否在 forge 日志里 PASS。
- **fork 复现覆盖率**：只统计"确有 fork 证据"的 run（`createSelectFork`/`createFork`/
  `forkBlockNumber`/`envString("RPC_URL")`/`--fork-url`/`pinned_block.json`）。
  本地 anvil 上跑通不算 fork 复现，两者必须分开。
- **声明 vs 观察的覆盖**：报告说做了源码审阅/静态分析/fork 测试/模糊测试，磁盘上是否
  有对应产物。不一致记为 overclaim / underclaim。
- **静态信号**：slither 结果条数、按 impact 分布；工具 JSON 与 stdout 是否一致。
- **fuzz 强度**：fuzz 测试数、实际执行的 runs 数。
- **函数覆盖**：目标 ABI 里有多少函数被 trace 实际调用过、多少只被 harness 静态引用。
  `coverage` 取两者并集（上界），`trace_coverage` 只信 trace（下界）。state-changing
  函数单独统计。ABI 候选按“与 trace/harness 调用重叠 → `sources/abi.json` →
  目标地址/名称提示 → 规模”排序，避免选到无关的大 ABI。这是行/分支覆盖的“可执行替代”，
  不要求源码进 Foundry 项目。

### C. 效率与成本（"一轮值不值"）

- 时长：p50/p90/max/total；按模型、镜像、链分层。
- 失败率：`run.status=failed` 的比例、无报告比例、schema 不可用比例。
- 单位产出成本：每轮耗时、每个高危发现的耗时（在 token 数据到位前先用秒数）。
- 产物体积：artifacts/sources 文件数与字节数。
- 后续：token 输入/输出、单轮费用、每次工具调用成本。

### D. 质量与可信度（"数字能不能信"）

- schema 分布：current / legacy / partial / invalid。
- 异常码计数：`report_missing`、`evidence_missing`、`evidence_outside_run`、
  `schema_legacy`、`declared_coverage_overclaim`、`slither_result_mismatch`、
  `target_address_mismatch`、`duration_inconsistent` 等。
- 重复 finding ID、报告 run_id 与目录名不一致、prompt 地址与报告地址不一致。
- legacy 报告占比（跨期比较必须先分层）。
- **跨 run 稳定性**：同一 `(chain, address)` 的多次审计一一配对。优先用 `function`
  字段锚定（函数集合有交集且标题相似度 ≥ 0.35），无函数锚定时标题相似度需 ≥ 0.5；
  标题相似度取 token Jaccard、overlap coefficient、sequence ratio 的最大值。
  算 `matched / (A+B-matched)` 重叠率和严重度 rank 漂移；按 model/image/prompt hash
  是否相同拆成 like-for-like 与 changed conditions。
- 后续：已知模板化问题占比（approve race、零地址转账等）用于区分"真发现"和"套路项"。

### E. 批次与规模化（"这一轮跑得怎么样"）

- 漏斗：发现 run → 有 metrics → 有报告 → 可解析 → schema 可用 → 可评估 →
  有发现 → 有高危。
- 轮次完成率：`--expected N` 对比实际产出；缺失 run 清单。
- 唯一目标数、重复审计数。
- 分层：model、reasoning_effort、image、chain、verified、schema grade。
- 后续：队列等待时间（需要 dispatcher 记录 Job 创建时间）、每批次成本。

## 4. 口径与陷阱

1. **分母必须是"可评估 run"**：报告可解析、schema 为 current/legacy、run 未失败、
   报告状态不是 `blocked`。缺报告、坏 JSON、运行失败的 run 只进漏斗，不进比率。
2. **严重度是提示词驱动的。** 历史归档里 `critical` 占 74%，是旧 prompt 要求
   "命中优先类一律 very high"的结果。任何严重度分布都要标注产生它的 prompt/模型版本。
3. **复现 ≠ 真值。** 测试通过只说明测试通过；fork 复现率要和证据解析率一起看。
4. **四种"覆盖率"必须分开命名**：证据覆盖率（引用可解析）、复现覆盖率（发现被测试复现）、
   函数覆盖率（ABI 函数被执行，trace 来源）、行/分支覆盖率（`forge coverage`）。
   混用会让数字失去意义；本工具只实现前三种。
5. **证据存在 ≠ 证据支持结论。** 工具只验证路径存在与可读，不判断文件内容是否支撑断言。
6. **稳定性 ≠ 正确率。** 两次高度重叠也可能两次都错；它只说明可重复，必须配人工抽样。
7. **跨期不可直接比较。** 工具链、模型、目标构成（verified 比例）变化时，先分层再比较。

## 5. 工具实现

两个阶段，职责分离：

### `collect`（每个 run 一次，在容器退出时）

入口：`docker/entrypoint.sh` 的 `finalize()`，在写完 `run_meta.json` 之后、持久化拷贝之前。

- 纯 stdlib，无网络、无子进程；只读 run 目录 + 写 `stats/metrics.json`。
- 原子写（临时文件 + `os.replace`），被 SIGKILL 也不会留下半截 JSON。
- 任何异常只写 `logs/stats.log`，不改变审计退出码；`AGENT_AUDIT_STATS=0` 可关闭。
- 报告字段全部归一化；未知严重度记 `unknown` 并产生异常，不丢分母。
- 证据路径做 realpath 包含性校验，拒绝绝对路径、`..`、越界软链。
- 文本读取上限 8 MB，避免异常大文件拖垮收集。

### `report`（一轮一次，离线）

- 只读各 run 的 `stats/metrics.json`，不读 `artifacts/`（除非 `--backfill`）。
- 输出 `summary.json`、`REPORT.md`、`runs.csv`、`findings.csv`、`stability.csv`。
- 幂等：同一批 metrics 反复聚合结果一致（除 `generated_at`）。
- 缺失 metrics 进漏斗并列出样本；`--backfill` 在 run 目录可达时就地重算。

### 为什么不是"扫报告"的脚本

- 报告解析、证据校验、forge 日志解析、fork 信号判断都很贵；放在容器退出时做一次，
  中央只处理 KB 级 JSON，聚合成本与产物体积解耦。
- metrics 是 schema 版本化的；工具升级后 `collect --force` 重算，历史结论可追溯。
- 中央聚合不接触 node-local 的大目录，多节点、离线、对象存储都能接。

## 6. k3s 落地

```text
Redis -> dispatcher -> runner Job -> runs/<run_id>/stats/metrics.json (hostPath)
                                           |
                     per-node copy / CronJob / DaemonSet
                                           |
                                   central runs/_stats/
```

### 单节点（当前形态）

`runner Job` 的 hostPath 已经挂载 `/var/lib/agent-audit/runs`，metrics 与证据同目录。
在节点上跑：

```bash
PYTHONPATH=/opt/agent-audit/tools python3 -m audit_stats report \
  --runs-root /var/lib/agent-audit/runs --expected 500 \
  --out /var/lib/agent-audit/reports/round-<date>
```

或部署一个 CronJob（挂同一个 hostPath）定时生成日报/周报。

### 多节点

hostPath 不共享，按节点收集。推荐"只拉 metrics"的稀疏 rsync：

```bash
rsync -a --include='*/' --include='stats/' --include='stats/metrics.json' \
  --exclude='*' root@nodeN:/var/lib/agent-audit/runs/ /central/runs/
```

然后中央 `report --runs-root /central/runs`。也可以用 DaemonSet 在每个节点监听
`*/stats/metrics.json` 并推到对象存储；Redis/Postgres 都不是必需组件。

### 批次关联

- `AGENT_AUDIT_IMAGE` 已由 dispatcher 注入，`metrics.run.image` 可用于按镜像分层。
- `batch_id` 建议加到 Redis 消息、Job annotation 和 Pod env，`metrics.run.batch_id`
  会记录它，用于"这一批 500 个地址跑完多少、产出多少"。
- 提交侧建议同时落一份 manifest（task_id 列表）；`report --expected` 用它的长度，
  缺失 run 会被列出来。

### 失败模式

| 情况 | 表现 | 处理 |
| --- | --- | --- |
| Pod 被 SIGKILL | 无 `stats/metrics.json` | 进漏斗 `missing`；目录还在就 `--backfill` |
| Job TTL 清理 | Pod 删除，run 目录保留 | 无影响，收集发生在退出前 |
| 收集中断 | 无 metrics 或旧 metrics | 原子写保证不出现半截文件 |
| 工具升级 | schema 版本不一致 | `collect --force` 重算，旧 metrics 覆盖 |
| 报告缺失/损坏 | `report.grade=invalid/missing` | 只进漏斗，不进比率 |

### 规模

单 run 的 metrics 通常在 5–40 KB（与 findings 数线性相关）。1 万 run 约 200–400 MB，
全量 JSON 解析在秒级；10 万 run 再考虑 SQLite/Parquet 索引，当前不必引入数据库。

## 7. 当前验证

用仓库里两条真实 run 验证（`runs/weth9-smoke-20261027-1` 与重构前的
`runs/v1_X5_1P-...`）：

- WETH9：4 个发现（1 low、3 informational），14 条证据全部可解析，
  16 个 harness 测试全 PASS，4/4 发现链接到通过的测试，fork 信号
  `createSelectFork` + `pinned_block.json`，fork 复现率 100%，0 异常。
  函数覆盖：11/11（trace ∪ harness，100%），其中 trace 单独 7/11（63.6%），
  state-changing 5/5，fallback 被调用；`name/symbol/decimals/allowance` 只被
  harness 引用、没有留在已保存的 trace 里，正好说明上界/下界为什么要分开看。
- 旧 run：schema 识别为 `legacy`，仍能计入 1 个 medium 发现；无证据、
  无 fork、无 forge 日志，产生 2 条异常但不影响聚合。
- 聚合：2 run，5 发现，高危 0，fork 复现 4/5，证据解析 14/14，函数覆盖 11/11。
- 稳定性：本地 WETH9 run 与 k3s WETH9 run（同地址、同 prompt、同 model）构成真实
  重复审计对：6 个并集 finding 中匹配 4 个（66.7%），2 个严重度 +1，4 对全部由
  `function` 锚定；纯标题 Jaccard 仅 22.2%，说明函数锚定的必要性。
- 20 地址 k3s 批量：20/20 完成、0 失败；146 个发现（5 critical、9 high），
  11/20 run 有高危；fork 复现 71/146；证据 577/577 可解析；函数覆盖 33.2%
  （trace 26.1%、state-changing 39.2%）；p50 15.0m、p90 26.6m。

## 8. 缺口与下一步

1. **token/费用**：在容器退出前从 DSH 会话导出 usage，写 `stats/usage.json`，
   聚合增加 `tokens_in/out`、`cost_usd`、每发现成本。
2. **行/分支覆盖**：只对"源码拷进 Foundry 项目并本地部署"的 run 开白名单，
   `forge coverage` 结果写 `stats/coverage.json`；fork 调链上字节码的 run 不适用。
3. **模板化比例**：维护已知通用问题清单，区分"该目标特有的发现"和"放之四海皆准的观察"。
4. **批次 manifest**：`scripts/enqueue_redis.py` 输出 task_id 清单，供完成率核对。
5. **稳定性扩样**：对重点目标按固定 prompt 重复审计 N 次，给重叠率置信区间，
   并作为模型升级的门禁指标。
6. **人工复核抽样**：按严重度分层抽样，记录复核结论，才能谈 precision。
