# 故障排查

本章帮助你解决使用 Axon 时遇到的常见问题。

## 安装问题

### Windows 安装失败

**症状**：PowerShell 安装脚本执行失败。

**解决方案**：

1. 确认使用 PowerShell 7+：

   ```powershell
   $PSVersionTable.PSVersion
   ```

2. 若版本低于 7.0，先安装 PowerShell 7：

   ```powershell
   winget install Microsoft.PowerShell
   ```

3. 允许脚本执行：

   ```powershell
   Set-ExecutionPolicy -ExecutionPolicy RemoteSigned -Scope CurrentUser
   ```

Axon Windows Daemon 使用当前用户最高权限 Task Scheduler 任务 `AxonDaemon`。任务以 `InteractiveToken + HighestAvailable` 登录启动，隐藏 PowerShell 包装器等待 Daemon 退出并透传退出码，`RestartOnFailure` 负责失败重试。Shell 直接继承 Daemon 的用户级 PATH、`USERPROFILE` 和提升权限，不使用 SYSTEM，也不依赖 `axon run` 启动 Worker。

注册与更新任务需要同一用户的管理员授权；源码安装器会请求 UAC，并保留已有 `~/.axon/config.toml` 与凭据。请勿输入其他管理员账号。停止与覆盖安装不需要提权：`axon daemon stop` 写停止请求让 Daemon 自身 drain 退出（等待窗口 15 秒），只有它未响应而必须强制结束进程树时才需要管理员权限。查看实际任务定义与恢复策略使用 `schtasks /query /tn AxonDaemon /xml`。用户未登录时外部渠道不可用。

### Linux / macOS 安装失败

**症状**：curl 安装脚本权限错误。

**解决方案**：

```bash
mkdir -p ~/.axon/bin
curl -LO https://github.com/a574676848/axon-code/releases/latest/download/axon-x86_64-unknown-linux-gnu.tar.gz
tar xzf axon-x86_64-unknown-linux-gnu.tar.gz
mv axon ~/.axon/bin/
chmod +x ~/.axon/bin/axon
```

### PATH 未生效

**症状**：安装后运行 `axon` 提示 `command not found`。

**解决方案**：安装器会自动将 CLI 目录和稳定 launcher 目录写入用户 PATH。当前已经打开的终端不会重新读取环境变量，请重新打开终端后验证：

```bash
which axon
axon --version
```

Windows 可在当前 PowerShell 临时刷新：

```powershell
$env:Path = "$HOME\.axon\bin;$env:Path"
axon --version
```

Linux/macOS 安装器会将 `$HOME/.axon/bin`（或自定义 `AXON_INSTALL_DIR`）写入当前 shell 的 profile；重新打开终端即可生效。

## 配置问题

### Provider 配置错误

**症状**：`axon doctor` 报告 Provider 检查失败。

**解决方案**：

1. 检查配置文件：

   ```bash
   cat ~/.axon/config.toml
   ```

2. 验证必需字段：

   ```toml
   [[providers]]
   name = "openai"
   protocol = "openai"
   base_url = "https://api.openai.com/v1"
   api_key_env = "OPENAI_API_KEY"

   [[models]]
   name = "gpt-4o"
   id = "gpt-4o"
   provider = "openai"
   context_window = 128000
   ```

3. 重新配置：

   ```bash
   axon setup
   ```

### API Key 无效

**症状**：API 调用失败，提示 `401 Unauthorized`。

**解决方案**：

1. 确认配置引用的环境变量名，并检查当前进程是否已设置该变量。不要打印密钥值：

   ```bash
   axon config show
   test -n "${OPENAI_API_KEY:-}" && echo "OPENAI_API_KEY 已设置"
   ```

2. 验证 API Key：

   ```bash
   # OpenAI
   curl https://api.openai.com/v1/models -H "Authorization: Bearer $OPENAI_API_KEY"

   # Anthropic
   curl https://api.anthropic.com/v1/messages -H "x-api-key: $ANTHROPIC_API_KEY"
   ```

3. 通过交互式向导重新设置 API Key：

   ```bash
   axon setup
   ```

### MCP 服务器无法启动

**症状**：`axon mcp list` 显示服务器离线。

**解决方案**：

1. 检查 MCP 配置：

   ```bash
   cat ~/.axon/.mcp.json
   ```

2. 手动测试 MCP server：

   ```bash
   # Node.js
   npx -y @modelcontextprotocol/server-filesystem /tmp

   # Python
   python -m mcp_server_filesystem /tmp
   ```

3. 修正 `command` / `args` 中的可执行文件路径。

## 运行时问题

### Axon 无法启动

**解决方案**：

1. 检查详细日志：

   ```bash
   AXON_LOG_LEVEL=debug axon
   ```

2. 运行预检：

   ```bash
   axon doctor
   ```

3. 检查网络与配置：

   ```bash
   curl -I https://api.openai.com
   ```

### 任务无法提交

**症状**：TUI 对话区出现「运行时失败」，`原因` 为 `连接 Daemon IPC 失败：系统找不到指定的文件。`，或 `axon run --format json` 输出 `run_failed` 事件。

**解决方案**：

1. 确认 Daemon 服务状态：

   ```bash
   axon daemon status
   ```

2. 服务未运行时拉起（已注册的 Windows 用户任务可由当前用户请求启动）：

   ```bash
   axon daemon start
   ```

3. Daemon 正在重启窗口内提交的请求不会被持久化，服务就绪后重新提交同一请求即可。

### 工具调用被禁止

**症状**：工具没有执行，回执状态为 `blocked`，并包含明确的禁止原因。

**解决方案**：工具无需人工审批。若调用被拒绝，请检查工具回执中的 `Forbidden` 原因、参数约束与运行时安全边界；其他合法调用均以 `Allowed` 直接执行。

### Shell 输出被截断

**症状**：Agent 收到的 Shell 结果只包含有界片段，并标记为 `truncated`。

**处理方式**：这是上下文预算保护。完整 Shell 输出会写入 artifact；工具回执中的
`output_ref.readPath` 是可直接传给 `builtin.read_file` 的绝对路径。Agent 可以结合
`offset` 和 `limit` 分页读取完整内容，例如：

```json
{"path":"<output_ref.readPath>","offset":1,"limit":2000}
```

不要根据 `archiveRef.relativePath` 猜测工作区路径；该字段相对于 Axon 运行态归档根目录，
应优先使用回执提供的 `output_ref.readPath`。

### 命令被资源围栏终止

**症状**（Unix）：命令的退出码为 `128 + 25 = 153`，或 shell 打印 `File size limit exceeded`；
**症状**（Windows）：`cmd.exe` 报 `The process cannot be created because the job has reached its active process limit`。
两者都表示命令撞上了子进程资源围栏，而不是命令本身写错。

**成因**：Axon 对 shell 工具启动的子进程设置了内核级资源上限（默认单文件 4 GiB、进程数 4096、打开文件数 4096；地址空间默认不限制）。正常构建与测试远低于这些值，触发通常意味着命令确实失控：`yes > file`、无限写日志、fork 炸弹变体或递归复制。

**排查步骤**：

1. 确认是不是围栏导致：把命令的写目标换成小文件重跑，若成功即为写入量问题。

   ```bash
   head -c 1000000 /dev/zero > /tmp/probe.bin
   ```

2. 需要临时放下围栏时，在启动 Axon 的 shell 中设置对应变量后重跑。逐维度关闭比整体关闭更有定位价值：

   ```bash
   # 只放开单文件大小（单位：字节）
   AXON_PROCESS_RLIMIT_FSIZE_BYTES=off axon run "…"

   # 只放开进程数；容器里跑 `make -j`、并行测试或包管理器时常用
   AXON_PROCESS_RLIMIT_NPROC=off axon run "…"

   # 只放开打开文件数；watch 模式、语言服务器或终端多路复用场景常用
   AXON_PROCESS_RLIMIT_NOFILE=off axon run "…"
   ```

   取值为 `off` 或 `0` 表示该维度不限制；也可以在同一个 shell 里 `export` 后长期生效。这些变量**不需要重新编译**，但必须由启动 Axon 的进程环境提供——`~/.axon/.env` 只承载 provider 凭据，其中的围栏变量不会被读取。

3. 需要恢复默认值（把某个维度调大而不是关掉）时，直接给具体数值：

   ```bash
   AXON_PROCESS_RLIMIT_FSIZE_BYTES=17179869184 axon run "…"   # 16 GiB
   ```

4. 受限容器里宿主的硬限制可能更低：`setrlimit` 只能降低硬限制，Axon 会静默取两者的较小值，因此配置了更大的值也不会报错，只是不会超过宿主上限。用 `ulimit -a` 或 `prlimit --pid $$` 查看宿主实际值。

围栏只覆盖 shell 工具启动的子进程；文件工具等 Rust 侧写入不受 `RLIMIT_FSIZE` 影响。需要区分「artifact 上限截断」与「内核终止进程」时见 [内置工具参数契约](./tool-schema.md#shell-命令格式)。

### 会话无法恢复

**症状**：`axon run --continue` 或 `axon run --session <ID>` 无法接续既有会话。

**解决方案**：

1. 会话由 Axon 自行接续，用户不需要也不应管理 session_id；`--continue` 只用于显式继续最近一次会话。

2. 诊断会话存储（只读运维面）：

   ```bash
   axon debug session list
   axon debug session doctor
   ```

3. 必要时清理损坏会话：

   ```bash
   # 仅删除指定会话事件，不影响其他数据
   axon debug session delete <ID>
   ```

## 性能问题

### 启动慢

**解决方案**：

- 减少 MCP server 数量，或拆分到不同项目。
- 启用本地 cache（默认开启）。
- 升级到最新版获取性能优化。

### 内存占用高

**解决方案**：

- 长会话由 Token Budget 自动换窗；Agent 必须先保存当前窗口的任务便签，旧消息归档后按需回查，不提供 `/compact` 手动入口。
- 长会话定期归档：`axon debug session export <ID>`。

### 会话上下文超限或窗口投影缺失

- 累积对话达到预算时，运行时在发送前通过统一换窗事务收敛上下文；保存便签、replacement 校验或提交失败时立即阻断，不继续发送旧历史。
- “会话已换窗但活动窗口投影缺失”表示持久化状态不完整。保留原始 rollout、便签和归档并修复投影，不删除配置，也不通过自动拼接归档历史绕过错误。
- 当前输入与必需动态上下文本身超过模型预算时，换窗仍不能容纳。减少本轮输入或选择上下文容量足够的模型；不要静默截断内容。
- Provider 返回上下文超限时直接返回错误，不自动重写历史或重放请求。运行时规范见 [内置工具与上下文契约](./tool-schema.md)。

### Provider 响应慢

**解决方案**：

- 使用 `axon providers test` 检查 Provider 连通性。
- 切换到支持 WebSocket 的传输方式或更近的 region。
- 检查本地网络代理配置。

## 日志分析

### 启用详细日志

```bash
# 最详细日志
AXON_LOG_LEVEL=trace axon

# 保存到文件
AXON_LOG_FILE=/tmp/axon.log AXON_LOG_LEVEL=debug axon
```

Daemon 的日志级别同样支持这两个环境变量，且优先于 `[agent].log_level`。服务形态下更直接的
做法是在 `config.toml` 显式设置 `[agent].log_level = "info"` 后重启服务：默认 `warn` 级别下
健康运行的 Daemon 不产生任何日志，`<root>/data/logs/axon-<日期>.log` 会一直保持不存在。

### 日志级别

| 级别 | 适用场景 |
|------|----------|
| `trace` | 深度调试 |
| `debug` | 排查问题 |
| `info` | 一般运行信息 |
| `warn` | 警告（CLI 默认） |
| `error` | 错误 |

### 常见日志模式

**Provider 连接失败**：

```
ERROR [provider] Failed to connect to https://api.openai.com
```

→ 检查网络与 API Key。

**Provider 响应或工具参数解析失败**：

```text
provider_raw_exchange_failed protocol=anthropic ... provider_event_type=Some("response.failed") ... request_body="..." response_tail=["..."]
```

该记录仅在请求、流读取或聚合失败时生成，适用于 OpenAI Responses、Chat Completions 和 Anthropic Messages。`provider_event_type` 是触发失败的 Provider 终态事件类型（如 `response.failed`、`response.incomplete`），非事件类失败为 `None`；同一请求的归因还记录在 `agent_run_send_provider_request 返回 Error` 的 `failure_kind`、`request_id` 与 `elapsed_ms` 上。若响应已经收到协议终止事件，但 tool-call 参数是空 JSON、非法 JSON 或缺少必填字段，Axon 不会执行该调用，也不会把整轮升级为配置错误；它会生成 Forbidden 工具回执，把失败结果交回模型继续修正。只有缺少 tool-call ID/名称、重复 ID 或无法解析协议边界时才进入 `provider_tool_call_incomplete` RuntimeFailure。日志中的 `request_body` 与 `response_tail` 均为有界内容，可能包含用户输入或模型输出，提交 Issue 前必须先脱敏；鉴权 header 不会记录。

Provider 只回一句「无法继续转发上游流」而不给错误码时，Axon 按消息归因成稳定错误码 `upstream_stream_aborted` 并判为上游侧失败，不再写成无法归因的 `unknown`；同渠道最多再试一次，其余预算留给换渠道。

如果 Provider 返回 `HTTP 400`，且错误同时包含 `REQUEST_BODY_INVALID` 与 `Invalid tool use format.`，通常表示当前渠道无法接受本轮工具历史格式。即使本轮已经产生可见输出，Agent Runtime 也会将该渠道标记为失败并切换到渠道链中的下一个渠道；这不是同一渠道的无限重试。

其他已有可见输出的错误按副作用边界分流：本轮还没有写过文件（工具回执里没有任何落盘写入），且失败属于可转移的瞬时传输错误时，Runtime 同样会切换到下一个渠道续跑，已经投影的内容与已经落地的工具结果都不会重放；本轮已经写过文件，或失败是参数与鉴权一类永久错误时，运行保留原始失败并结束，等人工裁决。没有备用渠道时同样保留原始失败。

**MCP server 超时**：

```
WARN [mcp] Server 'filesystem' timeout after 5s
```

→ 检查 MCP server 是否正常运行。

**工具调用被拒绝**：

```
INFO [tools] Tool call 'shell_exec' rejected by user
```

→ 正常，用户拒绝了工具调用。

**请求限流**：

遇到 429 时检查响应中的 `Retry-After`、`x-ratelimit-remaining-requests` 或 `ratelimit-remaining`。Axon 按 provider endpoint 的 origin 独立限流，一个上游耗尽配额不会阻塞其他域名。错误日志中的 `request_id` 可来自 `x-request-id`、`openai-request-id`、`request-id` 或 `cf-ray`，应优先用它与供应商日志关联。

## 获取帮助

如果以上方法无法解决问题：

- 查看 [文档中心](./README.md)
- 运行 `axon doctor --verbose`；需要复现启动问题时使用 `AXON_LOG_LEVEL=debug axon`
- 提交 [GitHub Issue](https://github.com/a574676848/axon-code/issues)，附上 `axon --version`、`axon doctor` 输出与详细日志
- 参与 [GitHub Discussions](https://github.com/a574676848/axon-code/discussions) 讨论

## 相关文档
- [快速开始](./quickstart.md)
- [配置指南](./configuration.md)
- [MCP 集成](./mcp.md)
