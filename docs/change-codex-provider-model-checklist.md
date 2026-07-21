# 在切换 Codex API 提供商和模型名称时的检查清单

本清单针对项目中 Codex runner 使用的 API provider、model 名称和相关密钥变量。
当前生产 Docker 配置主要来自 `docker/.codex/config.toml`，EVMbench/eval 配置主要来自
`eval_docker/.codex/config.toml`、`eval_docker/start.sh` 和
`EVMbench/agents/agent-audit-codex/config.yaml`。本地项目级配置在 `.codex/config.toml`。

Codex runner 的 Node 入口通常不会直接硬编码 provider/model；它会把打包进镜像的
Codex config 复制到 `CODEX_HOME/config.toml`，然后由 `@openai/codex-sdk` 启动 Codex。
因此，切换 provider/model 时要同时检查“打包配置”和“运行时已经存在的
`CODEX_HOME/config.toml`”。

## 变更前确认

- [ ] 明确目标运行路径：本地 Codex、生产 `docker/` runner、`k3s/` runner、`eval_docker/` runner、EVMbench overlay，还是全部路径。
- [ ] 明确新的 provider id、provider `name`、`base_url`、密钥环境变量名和 model 名称。
- [ ] 确认 `base_url` 与当前 Codex 配置格式兼容，通常应是 provider 的 OpenAI-compatible `/v1` endpoint。
- [ ] 如果继续使用 `wire_api = "responses"`，确认新 provider 支持 Responses API；否则先验证 Codex 版本和 provider 能力。
- [ ] 确认新 model 支持项目需要的能力：代码编辑、工具调用、多轮上下文、足够上下文窗口和 reasoning effort。
- [ ] 确认新 provider 的速率限制、并发限制、计费模型和失败重试策略能支撑 k3s 并行 Runner Job。
- [ ] 确认密钥轮换方式：旧密钥变量是否保留，新密钥变量是否要同步到 `.env`、Kubernetes Secret、CI/EVMbench secrets。
- [ ] 如果 provider 域名变化，确认 EVMbench `gateway_sni_hosts`、集群 egress 规则或防火墙允许新域名。
- [ ] 确认不会把 API key、真实 Secret、日志中的 Authorization header 或 provider token 提交到 Git。

## 需要检查的项目内位置

- [ ] `.codex/config.toml`: 本地项目级 Codex 默认 model。
- [ ] `docker/.codex/config.toml`: 生产 Docker runner 的 Codex provider/model 配置。
- [ ] `docker/Dockerfile`: smoke-test 中有 `model_provider = "apiapi"` 的 grep 断言；provider 改名时必须更新。
- [ ] `docker/run.sh`: 当前检查 `.env` 中存在 `APIAPI_API_KEY`；如果 `env_key` 改名，要同步更新检查逻辑和提示。
- [ ] `docker/README.md`: 更新 API key、provider、model 或运行示例。
- [ ] `.env` 和 `.env.example`: 更新 provider 密钥变量名和示例值。真实 `.env` 不应提交。
- [ ] `k3s/runner-secret.example.yaml`: 更新 runner Secret 示例中的 provider 密钥变量。
- [ ] `k3s/runner-secret.yaml` 或集群中的 `agent-audit-runner-env` Secret: 更新真实密钥变量和值，但不要提交敏感文件。
- [ ] `k3s/README.md`: 更新 Secret 准备说明和 provider key 名称。
- [ ] `eval_docker/.codex/config.toml`: EVMbench/eval 镜像内 Codex provider/model 配置。
- [ ] `eval_docker/start.sh`: 更新 `MODEL` 默认值、help 文本、fallback `config.toml` heredoc 和 `--model "${MODEL}"` 相关默认行为。
- [ ] `eval_docker/run.sh`: 当前要求并透传 `APIAPI_API_KEY`；如果密钥变量改名，需要更新校验和透传列表。
- [ ] `eval_docker/Dockerfile`: smoke-test 中有 `model_provider = "apiapi"` 的 grep 断言；provider 改名时必须更新。
- [ ] `eval_docker/README.md`: 更新运行示例和环境变量。
- [ ] `docker/codex-runner/agent-audit-run.mjs` 与 `eval_docker/codex-runner/agent-audit-run.mjs`: 通常不需要改 provider/model，但要确认运行时 config 复制逻辑不会被已有 `CODEX_HOME/config.toml` 遮蔽。

建议搜索：

```bash
rg --hidden -n \
  "apiapi|APIAPI_API_KEY|gpt-5\\.4|gpt-5\\.5|llmx\\.chat|apiapi\\.chat|model_provider|model =|base_url|env_key" \
  --glob '!target/**' --glob '!Cargo.lock' --glob '!.git/**'
```

## 修改规则

- [ ] 顶层 `model_provider = "<provider>"` 必须和 `[model_providers.<provider>]` 表名一致。
- [ ] `[model_providers.<provider>].env_key` 必须和运行环境里注入的密钥变量名一致。
- [ ] 如果只改 model 名称，也要同步 `eval_docker/start.sh` 的 `MODEL` 默认值和 EVMbench `config.yaml` 的 `MODEL`。
- [ ] 如果改 provider 域名，要同步 `base_url`、EVMbench `gateway_sni_hosts`、文档示例和任何网络白名单。
- [ ] 如果新 provider 不支持 `model_reasoning_effort = "xhigh"` 或不支持 Responses API，先调整对应配置，再做真实调用验证。
- [ ] 如果 provider 改名，更新 Dockerfile smoke-test grep，避免镜像构建阶段仍断言旧 provider。
- [ ] 如果密钥变量改名，更新 `docker/run.sh`、`eval_docker/run.sh`、`.env.example`、k3s Secret 示例和 EVMbench secrets。
- [ ] 不要更新 `.codex/config.toml`；生产镜像使用的是 `docker/.codex/config.toml`。

## 切换步骤

- [ ] 在独立分支上修改所有配置副本和文档。
- [ ] 删除或替换本地测试环境中旧的 `CODEX_HOME/config.toml`，避免它遮蔽新打包配置。
- [ ] 更新本地 `.env`、CI secrets、Kubernetes Secret、EVMbench secrets 中的新密钥变量。
- [ ] 构建生产 runner smoke-test 镜像。
- [ ] 构建 eval runner smoke-test 镜像。
- [ ] 用最小 prompt 做一次真实 Codex 调用，确认 provider 认证、model 名称和 wire API 都可用。
- [ ] 在 k3s 环境中提交一个测试任务，确认新 Runner Pod 读取到新 Secret 并完成任务。
- [ ] 在 EVMbench/eval 路径中运行一次小样本，确认 `submission/audit.md` 正常生成。
- [ ] 记录切换时间、provider、model、base_url、密钥变量名、镜像 tag 和 Git commit。

## 验证清单

- [ ] `rg` 搜索结果中旧 provider、旧密钥变量和旧 model 只保留在迁移说明或回滚记录里。
- [ ] `docker build -f docker/Dockerfile --target smoke-test -t agent-audit:smoke-test .` 通过。
- [ ] `docker build -f eval_docker/Dockerfile --target smoke-test -t agent-audit-eval:smoke-test .` 通过。
- [ ] `./docker/run.sh` 使用新密钥变量时不再提示旧 `APIAPI_API_KEY`。
- [ ] `./eval_docker/run.sh --audit-dir <sample-audit-dir>` 使用新密钥变量和新 `MODEL` 时可以启动。
- [ ] Codex 日志没有 `401`、`403`、`404 model not found`、`unsupported model`、`unsupported wire API` 或 provider rate limit 的持续失败。
- [ ] k3s 新建 Runner Pod 的环境变量名正确；检查变量名即可，不要打印密钥值。

## 回滚清单

- [ ] 恢复旧 provider/model 配置和旧密钥变量名。
- [ ] 恢复 Dockerfile smoke-test grep 断言。
- [ ] 恢复 `.env`、Kubernetes Secret 中的旧密钥变量。
- [ ] 重新构建并发布旧配置镜像，或把 k3s runner image tag 切回上一版。
- [ ] 清理或覆盖运行时旧的 `CODEX_HOME/config.toml`，确认容器没有继续使用失败配置。
- [ ] 用最小 prompt 做一次真实调用，确认旧 provider/model 恢复可用。
