# axon-cli

`axon-cli` 是 Axon Coding Agent 的命令行入口。它负责加载配置、组装运行时、执行 Agent 请求，并提供会话、诊断、扩展、Daemon、Remote 与 Cron 命令。

## 本地运行

```bash
cargo run -p axon-cli -- "检查当前工作区"
cargo run -p axon-cli -- run "生成单元测试建议"
cargo run -p axon-cli -- session list
cargo run -p axon-cli -- doctor
```

## 主要命令

- `run`：非交互执行任务。
- `session`、`resume`、`stop`：管理和恢复会话。
- `setup`、`providers`、`models`、`config`：管理本机配置与模型。
- `mcp`、`skills`、`image`、`lsp`：扩展与工具入口。
- `daemon`、`remote`、`cron`：后台运行宿主、远程接入与计划任务。
- `doctor`、`usage`：诊断与使用统计。

完整参数见仓库 [CLI 参考](../../docs/cli.md)。
