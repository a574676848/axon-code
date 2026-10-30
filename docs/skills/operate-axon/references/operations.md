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
axon memory search <query> [--image <URL_OR_PATH>]
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

### Cron 计划表达式

`--schedule` 接受三类写法，字段层面可自由组合，没有预置组合白名单。

固定间隔：`interval:<数值><单位>`，单位取 `ms`、`s`、`m`、`h`，例如 `interval:30m`。

一次性任务：`once:YYYY-MM-DDTHH:MM:SS`，按本机时区解析，不接受时区后缀。

cron 计划：`[日历前缀:]分 时 日 月 星期`。五个字段各自支持：

| 写法 | 含义 | 例子 |
|---|---|---|
| `*` 或 `?` | 任意值 | `0 9 * * *` |
| `5` | 单值 | `30 9 * * *` |
| `1-9` | 闭区间 | `0 1-9 * * *` 每天 1 点到 9 点每个整点 |
| `*/15` | 从字段下界起按步长取值 | `*/15 * * * *` 每 15 分钟 |
| `9-18/2` | 区间内再按步长取值 | `0 9-18/2 * * *` 取 9、11、13、15、17 点 |
| `9,12-14` | 逗号组合单值与区间 | `0 9,12-14 * * *` |
| `9/2` | 单值到字段上界按步长取值 | `0 9/2 * * *` 取 9、11、13、15、17、19、21、23 点 |

以上是常用写法，不是穷举；字段解析接受任意上述写法的逗号组合。

星期取 `0` 到 `6`，`0` 表示周日。

`日` 与 `星期` 的判定规则：一侧为 `*` 或 `?` 时只按另一侧判定，`0 9 1 * *` 是每月 1 号，`0 9 * * 1` 是每周一；两侧都具体时任一满足即触发，`0 9 1 * 1` 是每月 1 号或每周一。`?` 与 `*` 等价，都表示该字段不限定。

日历前缀可选，按中国日历筛选日期，可与任意字段组合（`weekday` 只按周一到周五，不读年历）：

| 前缀 | 含义 |
|---|---|
| `workday` | 工作日：周一到周五，排除法定节假日，包含调休上班日 |
| `weekday` | 周一到周五，不看节假日 |
| `holiday` | 法定节假日 |
| `weekend` | 周六周日，排除调休上班日 |

组合示例：

```text
workday:0 8-20 * * *       工作日 8 点到 20 点每个整点
workday:*/30 9-18 * * *    工作日 9 点到 18 点每半小时
workday:0 9 * * 1          前缀与星期字段同时生效：工作日里的周一 9 点
holiday:0 10 * * *         法定节假日 10 点
weekend:0 9 * * *          周末 9 点
0 0 1 * *                  每月 1 号零点
```

标准别名：`@hourly`、`@daily`、`@midnight`、`@weekly`、`@monthly`、`@yearly`、`@annually`。别名只压缩时间，不带日历语义，可叠前缀，例如 `workday:@daily`；别名不接受时刻后缀，需要固定时刻就直接写五段 cron。

日历按运行设备本地时区判定，年历数据取自 holiday-cn 数据集。用 `axon cron holiday sync [--year <年份>]...` 拉取年历，`--year` 可重复，省略时同步当前年与次年；`axon cron holiday status` 查看已同步年份、数据目录与数据源。未同步的年份按周末规则降级，不会凭猜测判出节假日；因此 `holiday:` 前缀需要对应年份已同步，未同步的年份该前缀不会触发。

### 定时任务的通知目标

每轮运行上下文都会给出三个事实字段，直接用它们，不要自己猜或抄历史：

| 字段 | 含义 |
|---|---|
| `session_channel` | 当前会话车道所属渠道 |
| `session_destination` | 当前会话车道目标 |
| `notification_chat_id` | 本机全局配置的通知飞书会话；未配置时该字段不出现 |

两种典型用法：

- 让任务执行完后**唤醒当前会话**继续跟进（查结果、补后续动作）：`--wake session:<session_channel>:<session_destination>`。
- 让任务执行完**直接通知用户**（不跑回合、不消耗模型）：`--wake feishu:<notification_chat_id>`。

判定顺序：

1. 用户明确说了投到哪里，就按用户说的写。
2. 用户没有特意指明时，用 `--wake feishu:<notification_chat_id>` 作为默认目标。
3. 上下文里没有 `notification_chat_id`（本机未配置）时，才省略 `--wake`。

其他口径：

- 省略 `--wake` 时任务执行完只写运行记录，不产生任何投递。
- 改或清空已有目标用 `axon cron update <JOB_ID> --wake <目标>` 或 `--clear-wake`；`axon cron list` 的 `wake` 列可核对当前值。
- `session_channel` 只支持 `feishu` 与 `desktop`。TUI 的车道标识按进程随机生成，任务触发时已失效，因此不接受 `session:tui:...`。
- `notification_chat_id` 是用户级全局配置，后端不会隐式采用；是否把它写成任务的 `wake` 由 Agent 在创建时决定。
