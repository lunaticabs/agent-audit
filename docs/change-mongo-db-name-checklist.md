# 在切换写入数据库名称时的修改检查清单

本清单针对 `agent-audit sync-run` 写入 MongoDB 的 database 名称，也就是
`AGENT_AUDIT_MONGO_DB`。`AGENT_AUDIT_MONGO_URI` 负责连接与认证，
`AGENT_AUDIT_MONGO_RUNS_META_COLLECTION` 和
`AGENT_AUDIT_MONGO_RUNS_FILES_COLLECTION` 负责 collection 名称。当前默认
database 是 `agent_audit`，默认 collection 是 `runs_meta` 和 `runs_files`。

Redis 只是任务输入队列，不是最终状态存储。只切换 MongoDB database 名称时，
通常不需要修改 Redis stream、consumer group 或 dispatcher 队列配置。

## 变更前确认

- [ ] 明确旧 database、新 database、切换时间窗口和回滚窗口。
- [ ] 确认本次是只切换 database，还是同时切换 MongoDB URI、认证用户或 collection 名称。
- [ ] 确认 `AGENT_AUDIT_MONGO_URI` 使用的账号对新 database 有写入、创建索引和读取权限。
- [ ] 确认新 database 名称符合当前 MongoDB 部署的命名规则，避免空值、路径字符、控制字符和容易混淆的大小写。
- [ ] 决定新 database 是空库启动，还是需要从旧 database 迁移 `runs_meta` / `runs_files`。
- [ ] 如果需要保留历史数据，先备份旧 database，记录备份路径、时间和校验方式。
- [ ] 检查是否有正在运行的 k3s Runner Job、手动 Docker runner 或本地 `sync-run` 正在写入旧 database。
- [ ] 确认外部查询脚本、仪表盘、导出任务或分析任务是否硬编码了旧 database 名称。
- [ ] 确认不会把真实 `.env`、`k3s/runner-secret.yaml` 或任何含密钥的值提交到 Git。

## 需要检查的项目内位置

- [ ] `src/config.rs`: `AGENT_AUDIT_MONGO_DB` 的默认值。如要改变默认 database，而不是只通过环境变量切换，需要同步修改这里。
- [ ] `.env`: 本地和 `docker/run.sh` 挂载进容器的运行时配置，需要设置新的 `AGENT_AUDIT_MONGO_DB`。
- [ ] `.env.example`: 如果示例默认值也要变化，更新这里。
- [ ] `k3s/runner-secret.example.yaml`: 更新示例中的 `AGENT_AUDIT_MONGO_DB`。
- [ ] `k3s/runner-secret.yaml` 或集群中的 `agent-audit-runner-env` Secret: 更新真实部署值，但不要提交敏感文件。
- [ ] `README.md` 和 `ch-README.md`: 如果默认值、部署说明或示例发生变化，更新文档。
- [ ] `k3s/README.md`: 如果 k3s secret 准备步骤或数据库说明发生变化，更新部署说明。
- [ ] `src/services/pipeline/*.rs` 的测试配置: 如果默认 database 从 `agent_audit` 改名，更新测试里的硬编码 `AppConfig`。
- [ ] 任何新增脚本、CI 配置或数据导出配置: 用 `rg` 确认没有遗漏旧 database 名称。

建议搜索：

```bash
rg --hidden -n "AGENT_AUDIT_MONGO_DB|agent_audit|runs_meta|runs_files|MONGO_URI|MONGODB_URI" \
  --glob '!target/**' --glob '!Cargo.lock'
```

## 代码路径确认

- [ ] `src/services/run_sync.rs` 通过 `client.database(&config.mongo_db)` 选择写入 database。
- [ ] `runs_meta` 写入 run 级元数据，`runs_files` 写入 run 文件内容。
- [ ] `_id` 由 `run_id` 或 `run_id:rel_path` 组成；切换 database 后不会自动和旧 database 去重。
- [ ] `sync-run` 会在目标 collection 上创建索引；新 database 第一次写入时需要有创建索引权限。
- [ ] `AGENT_AUDIT_MONGO_MAX_INLINE_FILE_BYTES` 不受 database 名称影响，但同一轮切换时应确认新环境沿用期望值。

## 切换步骤

- [ ] 暂停或排空正在写入旧 database 的任务。k3s 场景下，先确认没有活跃 Runner Pod 正在执行写入。
- [ ] 更新本地 `.env` 或部署 Secret 中的 `AGENT_AUDIT_MONGO_DB`。
- [ ] 如更改了 `k3s/runner-configmap.yaml` 中的 Secret 名称或 runner image，重启 dispatcher；只改 Secret 内容时，确保后续新建 Runner Pod 使用新 Secret。
- [ ] 如使用 Docker 镜像运行，确认容器挂载的是更新后的 `.env`。
- [ ] 如改变了代码默认值或示例文件，提交前同步更新相关测试和文档。
- [ ] 提交一个小规模 smoke run，执行到 `sync-run`。
- [ ] 在新 database 中确认最新 run 写入了 `runs_meta` 和 `runs_files`。
- [ ] 在旧 database 中确认切换后没有新的 run 继续写入。
- [ ] 记录切换时间、操作者、新旧 database、镜像 tag、Git commit 和验证 run_id。

## 验证清单

- [ ] `cargo test` 通过。
- [ ] `rg` 搜索结果中旧 database 名称只保留在迁移记录、历史说明或明确的回滚文档里。
- [ ] MongoDB 新 database 中存在预期索引：
  `runs_meta` 至少包含 `created_at`、`target.chain` / `target.address`、`status`、`has_final_report` 相关索引；
  `runs_files` 至少包含 `run_id + rel_path` 唯一索引、`run_id + kind`、`sha256`。
- [ ] 新 run 的 `runs_meta.file_count` 与 `runs_files` 中对应 `run_id` 的文件数量一致。
- [ ] Runner 日志没有 `AGENT_AUDIT_MONGO_URI is not configured`、权限不足、认证失败或创建索引失败。
- [ ] 如使用 k3s，确认新建 Job 的 Pod 日志显示任务完成，并且对应 run 已写入新 database。

## 回滚清单

- [ ] 将 `AGENT_AUDIT_MONGO_DB` 恢复为旧 database 名称。
- [ ] 停止或重新提交使用错误 database 的任务，避免同一个 run_id 的证据分散到两个 database。
- [ ] 如已经写入新 database，记录需要迁移、删除或保留的 run_id 列表。
- [ ] 回滚后重新执行一个 smoke run，确认旧 database 恢复写入。
- [ ] 在确认数据完整前，不删除旧 database 或新 database。
