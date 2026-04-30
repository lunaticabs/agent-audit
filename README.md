# agent-audit

`agent-audit` 是一个 `Codex-first` 的 EVM 合约审计工作区。

## 当前原则

- CLI 现在由 Rust 实现，负责把材料准备出来
- 运行方式是直接调用 `agent-audit`
- 本地开发时可以用 `cargo run --bin agent-audit -- <subcommand>`
- 代码修改后统一用 `cargo xtask check` 跑 `fmt`、`clippy`、`test`
- chain checks、Slither、Echidna、forge、cast、anvil 这类工具由 agent 自己决定是否直接调用
- 审计结论、深入验证顺序、是否调用这些工具，交给 agent 决定

## 开发检查

- 推荐命令：`cargo xtask check`
- 等价命令：`cargo run -p xtask -- check`
- 固定顺序：
  - `cargo fmt --all --check`
  - `cargo clippy --workspace --all-targets --all-features -- -D warnings`
  - `cargo test --workspace --all-features`

agent 在每轮代码修改后都应该先运行这条命令，再汇报结果；如果失败，至少要说明失败的是哪一步。

## 项目结构

- `src/main.rs`: 二进制入口，只负责启动 CLI
- `src/lib.rs`: crate 模块树
- `src/cli/`: clap 参数定义和命令执行入口
- `src/services/`: 核心业务服务，如 pipeline、source provider、Mongo 同步
- `src/analysis/`: 依赖发现和依赖分析逻辑
- `src/models/`: 共享数据结构
- `src/output.rs`: CLI JSON 输出和退出码约定
- `src/config.rs`: `.env` 和运行时配置加载
- `src/workspace.rs`: run workspace 与锁管理
