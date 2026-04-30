# agent-audit

`agent-audit` 是一个 `Codex-first` 的 EVM 合约审计工作区。

## 当前原则

- CLI 现在由 Rust 实现，负责把材料准备出来
- 运行方式是直接调用 `agent-audit`
- 本地开发时可以用 `cargo run --bin agent-audit -- <subcommand>`
- chain checks、Slither、Echidna、forge、cast、anvil 这类工具由 agent 自己决定是否直接调用
- 审计结论、深入验证顺序、是否调用这些工具，交给 agent 决定
