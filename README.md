# agent-audit

`agent-audit` 是一个 `Codex-first` 的 EVM 合约审计工作区。

## 当前原则

- Python 只负责把材料准备出来
- chain checks、Slither、Echidna、forge、cast、anvil 这类工具由 agent 自己决定是否直接调用
- 审计结论、深入验证顺序、是否调用这些工具，交给 agent 决定
