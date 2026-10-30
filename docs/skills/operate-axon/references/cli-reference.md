# Axon CLI 参考

此文件给出常见运行入口，不替代实时帮助。每次操作已安装 Axon 时先运行：

```text
axon --version
axon --help
axon <command> --help
```

参数名与位置参数一律以该命令的 `--help` 输出为准；下方只列运行态可用的入口与关键必填项。

## 常用命令

```text
axon [--cwd <path>] run [--model <model>] [--tool <qualified-name>] <prompt>
axon run [--model <model>] [--reasoning-effort <effort>] [--format text|json] [--continue|--session <id>] [--file <path>] [--timeout <seconds>] [--tool <qualified-name>] <prompt>
axon stop [--session <session-id>]

axon exec [--timeout-ms <ms>] [--kind command|test] <command...>    # --kind test 的回执带 testPassed
axon config path|env-path|validate|show|edit|set|get ...
axon providers add|remove|test <NAME>       # add: --name --protocol --base-url --api-key-env [--wire-transport --websocket-url]
axon models [--provider <name>] [--format text|json]
axon auth status [--provider <name>]
axon setup ...
axon skills [--summary]
axon mcp list [--limit <n>] | get <SERVER_ID> | add --url <url> [--bearer-token-env-var <ENV>] [--working-dir <dir>] | remove <SERVER_ID> | login [--browser] [--scope <scope>] [--client-id <id>] [--resource <uri>] <SERVER_ID> | logout <SERVER_ID>

axon memory stats|doctor|list|search <QUERY>|inspect <MEMORY_ID> ...
axon memory conflicts|compare <LEFT_MEMORY_ID> <RIGHT_MEMORY_ID>|review|quality|sync-audit ...
axon --format json memory quality
axon [--format text|json] memory skill-evolution-status <session-id>
axon [--format text|json] memory skill-evolution-promote <session-id> <skill-name>
axon memory consolidate|consistency|recover-conflict [MEMORY_IDS]...|approve|approve-pending|archive-pending|quarantine <MEMORY_ID>|forget <MEMORY_ID> ...
axon memory export --output <path> [--limit <n>] [--signing-key-id --signing-seed-hex --encryption-key-id --encryption-key-hex]
axon memory import --input <path> [--yes] [--trusted-public-key-hex --decryption-key-hex]

axon daemon install|uninstall|start|stop|restart
axon daemon status                                  # 只输出 installed / running
axon daemon health [--endpoint <name>]              # 内部健康快照：host、积压、订阅数
axon daemon drain [--endpoint <name>] [--reason <text>]
axon daemon run --root <dir> --config-path <file> [--endpoint <name>]
axon daemon attach [--endpoint <name>] [--sequence <n>] [--generation <id>]
axon daemon schedule --session-id <id> --kind <kind> --prompt <text> [--workspace-id <id>] [--interval-ms <ms>] [--delivery-mode <mode>]

axon persona validate [--file <path>]|status [--json]
axon persona style list [--all]|retire <style-id>
axon persona reset [--yes]

axon doctor [--check <area>] [--verbose] [--fix]
axon cron list [--limit <n>] [--include-removed] | create --name <NAME> --schedule <SCHEDULE> [--prompt <text>] [--skill <name>] [--workdir <dir>] [--no-agent --script <path>] [--wake feishu:<chat_id> | --wake session:<channel>:<destination>] | update <JOB_ID> [--wake ...|--clear-wake] ... | pause|resume|remove|run <JOB_ID> | tick [--limit <n>]
axon remote status|subscriptions|outbox|audit|feishu-status|feishu-worklist|render-feishu|deliver-feishu|execute-feishu-sdk-loop|simulate-feishu ...
axon remote revoke <SESSION_ID> <SUBSCRIBER_ID>
axon lsp servers|workspace-symbol|definition|references|diagnostics ...    # 通用参数：--workspace-root <dir>、--plan-only

axon debug session list [--limit <n>] [--cursor <c>] [--cwd <dir>] [--archived]
axon debug session show|delete <SESSION_ID> | read|resume <THREAD_ID> | replay [--provider-call-id <id>] | output [--max-chars <n>] | export [--output <path>] | fork [--new-thread-id <id>] [--cwd <dir>] | doctor [--repair]
axon debug builtin list
axon debug builtin run --args <json> [--timeout-secs <sec>] <TOOL>
axon debug channel-metrics [--since-hours <HOURS>] [--model <MODEL>] [--provider <PROVIDER>] [--limit <COUNT>]
axon debug mailbox [--owner <id>] [--recipient <id>] [--state <state>] [--limit <n>]
axon debug realtime-metrics [--window <w>] [--minute-limit <n>] [--rebuild-bucket]

axon serve [--host <host>] [--port <port>] [--auth-token-file <path>]    # 默认 127.0.0.1:3000，仅接受 GET
axon usage [--period today|7d|30d|all] [--provider <name>] [--model <name>] [--source all|axon-cli|subagent] [--session <id>] [--run <id>] [--limit <n>] [--refresh] [--format text|json]
axon usage diagnostics [--period <period>]
axon usage audit [--model <name>] [--source <source>] [--status <status>] [--from-ms <ms>] [--to-ms <ms>] [--cursor <c>] [--limit <n>]
axon usage export [--format csv|json] [--output <path>] [--model <name>] [--source <source>] [--status <status>] [--from-ms <ms>] [--to-ms <ms>]
axon pricing list|add|delete --id <ID>|sync|recalculate ...
axon export [--output <path>] [--limit <n>]
axon completion power-shell|bash|zsh|fish
axon package [--target <triple>] [--binary <path>] [--output-dir <dir>] [--archive] [--verify-manifest <path>] [--no-smoke] [--format text|json]
axon upgrade [--manifest-url <url>] [--download] [--apply] [--rollback] [--output-dir <dir>] [--asset <name>] [--format text|json]
```

## TUI 交互命令

在 `axon` 交互界面内可直接执行（输入 `/` 会给出命令与参数提示）：

```text
/help                             查看 TUI 能力与常用命令
/clear                            清空当前会话视图，保留顶部状态栏
/model <model>                    切换下一轮使用的模型
/think                            调整本轮推理强度
/status                           查看当前 TUI 运行状态
/mcp [list|login|logout <server>]  查看 MCP server 与工具清单、登录或登出
/skills [list|toggle]             管理 skills
/exit                             退出 Axon Agent TUI
```

- `/clear`、`/model`、`/think` 在任务运行中不可用；`/exit` 的别名是 `/quit`、`:q`、`exit`、`quit`。
- MCP server 需要 OAuth 时，首屏加载完成后只提示一次并给出 `/mcp login <server>`；Axon 不会自动打开浏览器。
- `/mcp login` 在后台线程执行，不阻塞渲染与输入；失败时回退提示使用 `axon mcp login <server>`。

## 运行与核验

- `axon run --format json` 输出 JSONL。持续读取至 `run_completed` 或 `run_failed`，不要按单个 JSON 对象解析。
- 需要限制一次任务的工具范围时重复传 `--tool <tool-name>`；只传当前可见工具列表中的完整名称。
- 使用 `--cwd <path>` 让 Agent 在指定工作区开始；单条 shell 命令的 cwd 则由 `builtin.shell_command` 的 `workdir` 提供。
- `--format` 既可作为顶层参数（`axon --format json <command> ...`），也可由支持它的子命令自己接收（如 `axon usage`、`axon models`、`axon pricing`、`axon skills`、`axon setup`、`axon providers`、`axon auth`、`axon upgrade`、`axon package`）。子命令帮助里没有 `--format` 的命令（如 `axon config get`、`axon debug session read`）必须用顶层写法。
- 会话诊断的两个标识不可互换：`axon debug session show|delete` 收 `SESSION_ID`，`axon debug session read|resume` 收 `THREAD_ID`。
- `memory skill-evolution-status` 读取 `skill-evolution/state.json` 中指定 session 的当前状态；系统不保留历史状态流水。
- `axon debug channel-metrics` 查询已持久化的渠道性能、缓存与连接指标。`--since-hours` 默认读取最近 24 小时，设置为 `0` 取消时间过滤；可用 `--model`、`--provider` 过滤，`--limit` 控制最多输出的渠道数。只有启用 `channel_balancer.telemetry_persistence_enabled` 后，运行遥测才会写入 `~/.axon/data/channel-metrics`。
- 修改配置、删除会话、执行 Cron、发送远控消息、生成图片、升级或写入 Memory 前，先确认目标和副作用，并在完成后读取回执或 status。
- 命令帮助中出现的 provider、传输、存储或后台行为属于配置和系统细节；Agent 只按命令帮助提供的当前参数执行，不自行构造隐藏设置。
