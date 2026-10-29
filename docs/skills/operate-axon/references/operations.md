# Axon 运行操作

## 执行 Agent 任务

```text
axon --cwd <path> run --model <model> --tool <qualified-name> <prompt>
axon run --format json <prompt>
axon stop --session <session-id>
axon run --continue <prompt>
```

- 先用 `axon run --help` 确认当前版本支持的选项。
- 有界验证时限制 `--tool`，并用精确、单一的任务描述要求 Agent 只调用该工具一次。
- 自动化 run 读取 JSONL 直至终态；之后用 `axon debug session read <thread-id>` 检查工具调用、回执和后续模型响应（`show|delete` 收 session_id，`read|resume` 收 thread_id，两者不可互换）。

## 运行时工具

- `builtin.invoke_skill` 只传 `skill_name`，用于加载 Skill 操作说明。加载后再调用其中说明的实际工具。
- `builtin.shell_command` 的 `command` 与 `intent` 都必填：`intent` 是一句话说明这次命令要达成什么目的（写目标、不复述命令原文、40 字以内），它是这次执行面向用户的说明。`workdir` 按需提供；只有命令需要交互终端时才传 `tty: true`。`command` 自身是脚本内容，不再包一层 shell；JSON 字符串内的双引号要转义。简单 Python 检查优先使用单行 `python -c`，多行脚本直接用当前 shell 的承载形式（POSIX 的 heredoc、PowerShell 的 here-string），运行时不预检脚本内容，语法与方言错误由 shell 自身回报。TTY 会立即返回 `session_id`；普通命令超过首次等待窗口后也返回 `session_id`，且进程继续运行。
- 长时间运行的命令通过 `builtin.write_stdin` 使用仍在运行的 `session_id` 续接，`intent` 与 `shell_command` 同为必填。只提供 `session_id` 时，调用会等待新输出或进程终态，也可能在本次等待窗口结束时返回；这不会终止后台进程。但前台会话有 10 分钟空闲回收窗口：距创建或最后一次 `write_stdin` 触碰满 10 分钟会被终止（退出码 `130`）并注入 `shell-session-reclaimed` 通知，因此挂起后去做别的事必须在 10 分钟内至少轮询一次，或改用 `run_in_background`。可为纯等待设置更长的 `yield_time_ms` 以减少轮询回合；提供 `chars` 可输入，设置 `ctrl_c: true` 可中断。
- 文件查找、读取与编辑操作分别使用当前可见的 `glob`、`search`、`read_file` 和 `edit` 工具；需要看图时同样使用 `read_file`（PNG/JPEG/WebP/GIF 走图片分支，只用 `path`，图片随回执交给模型，并自动归一化为单帧 8-bit sRGB、按像素与字节预算降采样；源图上限 50 MiB / 6400 万像素 / 长边 65535 px，超限时要求先缩小）。`glob` 与 `search` 的 `intent` 必填（写法与长度同 `shell_command`）。`edit` 有两种直接输入：精确修改已读取文件时，用回执的 `[path#TAG]` 加 Hashline 操作；新建文件或整文件重写时，用 `*** Add File:`（目标已存在即按整文件重写处理，不要再配 `*** Delete File:`）；删除、移动/重命名与必须原子的跨文件修改时，用 `*** Begin Patch` 包裹 PathDiff 的 `*** Delete File:`、`*** Update File:` 和可选 `*** Move to:`（以及新建场景的 `*** Add File:`）。两种格式都不使用 JSON；修改后用测试、读取或编辑回执诊断验证。
- 复杂或多阶段任务用 `builtin.update_plan` 提交 3 到 7 个可验证结果的完整计划快照：`intent` 必填（写法与长度同 `shell_command`），最多一个 `in_progress`，`completed` 必须附验证证据，不做单步骤增量修改。
- 在关键决策或缺失必要输入时调用 `builtin.ask_user_question` 向用户提问，该工具为主 turn 专属，子代理不装配。预算内等待用户答复，超时自动挂起任务并释放输入坞；用户答复后作为新输入继续任务。
- 需要在本会话既往归档窗口中回查细节时使用 history 四件套：`builtin.history_list_windows` 可按 `limit`、`recent_first` 列举归档窗口；`builtin.history_list_items` 可按 `window_id`、`role`、`tool_namespace`、`tool_name` 筛选条目，并用 `limit`、`recent_first`、`max_chars_per_item` 控制回执；`builtin.history_read_item` 必须同时提供历史工具返回的 `window_id` 与 `item_id`，可用 `offset_chars`、`limit_chars` 分段读取；`builtin.history_search_contents` 使用非空、区分大小写的字面量 `query` 检索，可用 `window_id`、`role`、`tool_namespace`、`tool_name`、`limit`、`recent_first` 筛选。其他筛选字段省略或传 `null` 表示不限，`role` 区分大小写；正整数限制不能传 0，未知窗口的列表与检索返回空结果。
- 需要 MCP 外部上下文但不确定 URI 时，先用 `builtin.list_mcp_resources` 列出可用资源，再用 `builtin.read_mcp_resource` 读取；两者都是只读能力。
- 记忆窗口由运行时自动管理：`builtin.get_context_remaining` 查询距自动换窗的剩余 token 与数值来源（Provider 实测或本地估算）。临近换窗时，先用 `builtin.notes_write_file(path, text, intent)` 整文件替换或用 `builtin.notes_append_to_file(path, text, intent)` 原样追加，两者 `intent` 必填（写法与长度同 `shell_command`）；用 `builtin.notes_read_file(path, start_line, stop_line)` 按全部或 1-based 行范围读取，行号为负时从文件末尾回数；用 `builtin.notes_list_files_by_prefix(prefix, max_results, file_order_by, file_order)` 按虚拟路径前缀列举文件，其中排序字段为 `name`、`created_at`、`updated_at`，方向为 `ascending` 或 `descending`；用 `builtin.notes_search_contents(query, max_matches_per_file, max_files, path_prefix, recent_file_first)` 按非空、区分大小写的字面量检索。除 `path`、`text` 外的字段均可省略；只有 `prefix`、`start_line`、`stop_line`、`path_prefix` 按 schema 支持显式 `null`，`max_results`、`max_files` 最大为 1000，正整数限制不能传 0。便签路径只能是当前会话便签目录内的相对虚拟路径，单文件最多 1,000,000 UTF-8 字节，接近上限时新建文件。确认写入回执成功后再调用 `builtin.new_context`；该工具在调用边界校验保存代际，未保存时只拒绝本次调用并返回可执行指令，不会开启新窗口，可在同一 turn 内先补写便签再重试。写入失败时换窗 fail-closed，必须先修复保存再重试。换窗后先回读 notes 五件套，再按需使用 history 四件套补齐旧窗口细节。
- Goal 操作使用语义明确的工具：`builtin.goal_create` 建立跨轮承诺，`builtin.goal_list` 分页检索可见 Goal，`builtin.goal_get` 查看指定 Goal 的进度、Execution 和 Result，`builtin.goal_start` 启动尚未执行或继续等待、暂停的目标，`builtin.goal_update` 携带 `expectedRevision` 更新，`builtin.goal_cancel` 携带 `expectedRevision` 与非空原因持久取消。Goal 无审批等待状态；高风险操作的执行边界由工具自身的二元执行权限（`Allowed` / `Forbidden`）与远控能力门裁决。普通回答、澄清和本轮内即可收尾的工作不必调用。不要把所有 Goal 无界注入上下文；用户明确询问时才分页读取，并按当前 Workspace 和 disclosure policy 解释结果。

## HTTP 与 WebSocket 接口

两个面都随 Daemon 提供，写入 config 的 `remote.gateway.*` 决定监听地址，凭据由 `remote.gateway.api_token_env` 指定的环境变量或配置相邻 env 文件提供。

```text
axon config get remote.gateway.http_bind         # Gateway HTTP 面
axon config get remote.gateway.websocket_bind    # Gateway WebSocket 面
axon daemon status                               # 两个面都要求 Daemon 处于 running
```

- 单个接口调用：本 Skill 目录下的 `scripts/gateway-http.ps1` 与 `scripts/gateway-ws.ps1`，两者都会按运行态配置解析监听地址与 token，并打印原始回执。
- 只读优先：先用 GET 路由或 `ping` 确认链路，再考虑 `submit`、`pause`、`interrupt`、`stop`、费率写入等状态变更。
- 完整路由表、帧协议、错误码与排障对照见 [gateway-http.md](gateway-http.md) 与 [gateway-websocket.md](gateway-websocket.md)。

## Session 和 Memory

```text
axon debug session list
axon debug session show <session-id>
axon debug session read <session-id>
axon debug session replay <session-id>
axon memory stats
axon memory search <query>
axon memory inspect <memory-id>
```

- Session 的删除、修复和 fork 会改变数据或执行上下文，须先取得用户授权；`axon debug session` 是只读优先的运维诊断面，用户不需要也不应管理 session_id。
- 只有明确、可复用且有证据的结论才写入 Memory。归档、隔离、遗忘、导入或其他治理动作前，先确认范围并获得授权。
- Memory 的 JSON 记录包含稳定的 `memoryKind` 字段，取值为 `profile`、`preference`、`experience`、`knowledge` 或 `procedure`；Personal 只允许前两类，Workspace 只允许后三类。
- 打开旧版本写入的 Memory 库时，缺少 `memoryKind` 列的 `memory_records` 会被整体重命名为 `memory_records_legacy_<毫秒时间戳>` 并释放其索引名，随后建出目标态新表；旧行不删除也不猜测类型。`GET /api/v1/memory/stats` 的 `legacyRecordTables` 会列出这些归档表，界面以告警条呈现。
- `axon memory consistency` 同时核对两个方向：记录到投影的缺口，以及投影到记录的残留（`orphan: projections=… text_documents=…`）。后者覆盖「记录已不存在但向量投影、Tantivy 文档或 HNSW 向量仍在」的历史残留；`--repair` 会清理这类残留并用完整投影集整体替换物理向量索引。
- 桌面记忆管理通过 Gateway `/api/v1/memory` 使用 `memoryKind`、`scope`、有界游标分页和当前作用域统计；`scope` 取值为 `all`、`personal`、`workspace` 或 `workspace:<namespace>`，实际存在的 scope 由 `/memory/scopes` 列出。`POST /memory/{memoryId}` 以 `supersedesMemoryIds` 形成修改关系，`POST /memory/{memoryId}/forget` 写入 tombstone，`DELETE /memory/{memoryId}` 执行物理删除。仓库类型（`experience`、`knowledge`、`procedure`）写入时必须提供 `workspacePath` 文件夹绝对路径，请求体可以是单条对象、数组或 `{"records": [...]}` 包装体。客户端不得提交 `userRef` 或 `namespaceRef`，回执中的 `needsRepair` 表示索引投影清理状态。

## Daemon、Cron 和 Remote

```text
axon daemon status
axon daemon health
axon cron list
axon remote status
axon remote feishu-status
```

- 运行态判据：`axon daemon status` 只回 `installed` 与 `running`；`axon daemon health` 才给出 `host.status`、`executionBacklog.pending`、`deliveries.backlog.pending` 与 `streamSubscribers`。
- 优雅收口顺序：先 `axon daemon drain [--reason <text>]`，再用 `axon daemon health` 确认两个积压字段归零，最后才 `axon daemon stop`。`stop|restart` 先写停止请求让 Daemon 自行收口（等待窗口 15 秒），未响应才禁用并结束计划任务，Windows 上再强制结束进程树并中断在跑的任务。
- 服务启停、创建或运行 Cron、远控投递和凭据变更都会产生状态变化。先检查目标状态和命令帮助，完成后重新读取状态或对应回执。
