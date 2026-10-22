---
name: operate-axon
description: 已安装 Axon 的运行时操作手册。用于配置和模型、Agent run 与 session、工具、Skill、Memory、MCP、Daemon、Remote、Cron、诊断与升级；以当前 CLI 帮助和工具回执为准。
---

# Operate Axon

本 Skill 只描述 Agent 在运行时能够观察和操作的 Axon 能力。当前 `axon --help`、目标子命令 `--help`、可见工具、命令回执和 session 记录是事实来源；不推断未显示的参数、工具或行为。

## 开始前

- 用户给出二进制路径、`--config`、`--cwd`、provider、model、session 或输出格式时，原样使用。
- 默认使用 PATH 中的 `axon`。先检查 `axon --version`、`axon --help`、`axon --format json config validate`，再读取目标子命令帮助。
- 自动化优先使用 `--format json`。`axon run --format json` 输出 JSONL，持续读取至 `run_completed` 或 `run_failed`。
- 不打印密钥、token、OAuth 凭据、MCP 原始 header 或 secret 文件内容。

## Agent 工具

- 只调用当前可见工具列表中的完整名称；不要猜别名、大小写或未显示的工具。
- `builtin.invoke_skill` 只接收 `skill_name`，只加载该 Skill 的操作说明；读完后调用说明中列出的实际工具完成任务。不要传 `action`、图片路径或其他任务参数。若加载返回「该 Skill 禁止模型调用」一类失败，表示它不允许由模型加载，不要重复试探。
- `builtin.shell_command` 必须提供 `command` 与 `intent`：`intent` 是一句话说明这次命令要达成什么目的（写目标、不复述命令原文、不加句号、40 字以内），它是这次执行面向用户的说明。`workdir` 按需提供；只有命令实际需要交互终端时才传 `tty: true`。TTY 调用会立即返回 `session_id`；普通命令超过首次等待窗口后也会返回 `session_id`，进程继续在后台运行。随后用 `write_stdin` 继续。`command` 写裸脚本，不要包裹 `bash -lc`、`sh -c`、`zsh -c` 或 `pwsh -Command`。JSON 字符串内的双引号必须转义；简单 Python 检查优先写成单行 `python -c`，多行脚本用当前 shell 的承载形式（POSIX 的 heredoc、PowerShell 的 here-string）并保持缩进：运行时不预检脚本内容，语法与方言错误由 shell 回报。若回执指出 wrapper 参数不符合平台规范，移除外层包装后只重试一次；若 stderr 是脚本语法错误，先修正脚本再重试，禁止原样重复调用。
- `builtin.write_stdin` 每次调用必须提供 `intent`（写法与长度同 `shell_command`），并只能使用仍在运行的 `shell_command` 返回的 `session_id`；提供 `chars` 写入，`ctrl_c: true` 中断，或只提供 `session_id` 等待新输出。纯等待由新输出、进程终态或本次等待窗口结束唤醒；窗口结束不会终止进程，但**距创建或最后一次 `write_stdin` 触碰满 10 分钟**的会话会被运行时空闲回收（退出码 `130`）并注入 `shell-session-reclaimed` 通知：收到后不要再写入该会话，按通知里的归档路径核对输出，需要继续等待就重新执行或改用 `run_in_background`。耗时任务可为纯等待显式设置更长的 `yield_time_ms` 以减少轮询回合；stdin 和 Ctrl-C 交互使用短响应窗口。对交互终端，先在一次调用中写入文本，再在下一次调用中单独写 Enter；不要把整段文本和 Enter 放进同一次写入，避免全屏 TUI 把它识别为粘贴。输出预算由运行时管理，不要主动调整 `max_output_tokens`。
- 文件定位使用 `builtin.glob` 或 `builtin.search`，读取使用 `builtin.read_file`，修改使用唯一的 `builtin.edit`。`glob` 与 `search` 每次调用都必须提供 `intent`（写法与长度同 `shell_command`）。编辑输入格式严格二选一，禁止混用：精确修改已读取文件时使用 Hashline，整个输入必须从回执中的 `[path#TAG]` 开始，下一行写 Hashline 操作（如 `INS.TAIL:`）；新建文件或整文件重写使用 `*** Add File:`（目标已存在即整文件重写，无需先删；此时回执的 `operation` 为 `update` 并回传新的 `[path#TAG]`）；删除、移动/重命名与必须原子的跨文件修改使用 PathDiff，整个输入必须由 `*** Begin Patch` 和 `*** End Patch` 包裹。PathDiff 同一批次中每个源路径和目标路径只能出现一次，同一文件多处修改必须合并到同一个 `*** Update File:` 块，禁止重复 `Update/Add/Delete` 或把 `[path#TAG]` 放入 PathDiff。两种格式都直接发送原始文本，不用 JSON。
- Hashline 的两类容错与恢复：方括号头里混入 `***`、`Update File:` 等 apply_patch 关键字时会被自动剥离；读取后文件被外部改动时，运行时会按「所有锚点位移一致 + 邻域校验」把编辑重放到当前内容上，一旦位移不一致或邻域不符即 fail-closed，回执要求重新读取；收到这类回执必须重新 `read_file` 取新 `[path#TAG]`，不要硬套旧 TAG。
- 复杂、跨模块、高风险或多阶段任务在开始和阶段切换时用 `builtin.update_plan` 提交完整计划快照：必须提供 `intent`（写法与长度同 `shell_command`），3 到 7 个可验证结果、最多一个 `in_progress`，`completed` 必须附实际验证证据；不做单步骤增量修改。
- 在关键决策或缺失必要输入时调用 `builtin.ask_user_question` 向用户提问，该工具为主 turn 专属，子代理不装配。预算内等待用户答复，超时自动挂起任务并释放输入坞；用户答复后作为新输入继续任务。
- 需要过去的用户偏好、项目决策或经验时调用 `builtin.memory_search`；记忆写入统一调用 `builtin.memory_write`。两者是所有 Agent 入口唯一的 Memory 工具，scope 由运行时绑定，禁止直接调用旧查询/写入 API 或操作 Memory SQLite/索引。需要在本会话既往归档窗口中查找命令、报错或早期结论时用 history 四件套：`builtin.history_list_windows` 按 `limit`、`recent_first` 列出窗口；`builtin.history_list_items` 按 `window_id`、`role`、`tool_namespace`、`tool_name` 过滤条目，并用 `limit`、`recent_first`、`max_chars_per_item` 控制回执；`builtin.history_read_item` 必须同时提供历史工具返回的 `window_id` 与 `item_id`，支持 `offset_chars` / `limit_chars` 分段读取，未知窗口会显式报错；`builtin.history_search_contents` 用非空、区分大小写的字面量 `query` 检索，并支持同一组筛选字段与 `limit`、`recent_first`。其他筛选字段省略或传 `null` 表示不限，`role` 区分大小写，正整数限制不能传 0；list/search 遇未知窗口返回空结果。
- MCP 工具 schema 随每轮请求全量下发，直接在工具清单中找到并调用完整工具名。需要 MCP 外部上下文但还不知道 URI 时先用 `builtin.list_mcp_resources` 列出资源，再用 `builtin.read_mcp_resource` 读取。
- 上下文窗口由运行时自动管理：`builtin.get_context_remaining` 使用空参数查询距自动换窗的剩余 token 与数值来源；临近换窗先用 `builtin.notes_write_file(path, text, intent)` 整文件替换或 `builtin.notes_append_to_file(path, text, intent)` 原样追加跨窗口便签（`intent` 必填，写法与长度同 `shell_command`），再用 `builtin.notes_read_file(path, start_line, stop_line)` 按全部或 1-based 行范围读取、`builtin.notes_list_files_by_prefix(prefix, max_results, file_order_by, file_order)` 按路径前缀列举、`builtin.notes_search_contents(query, max_matches_per_file, max_files, path_prefix, recent_file_first)` 按非空且区分大小写的字面量检索。便签内容仅作用于当前会话，不写长期记忆；路径只能是当前会话便签目录内的相对虚拟路径，写入后单文件 ≤ 1,000,000 UTF-8 字节；正整数限制不能传 0，列表和检索的文件上限为 1000；只有 `prefix`、`start_line`、`stop_line`、`path_prefix` 可按 schema 显式传 `null`，其余可选字段省略。确认写入回执成功后，调用 `builtin.new_context`（可选 `reason`）；该工具会在调用边界校验保存代际，未保存时只拒绝本次调用并返回可执行指令、不开启新窗口，可在同一 turn 内先补写便签再重试。写入失败时换窗 fail-closed，必须先保存成功再重试。换窗后先回读 notes 五件套，再用 history 四件套补齐细节。
- 内核工具严格收敛于系统基础原语；公网检索统一由 Skill / MCP 连接器（如 anysearch）扩展，文档与本地多格式文件直接使用 `builtin.read_file` 派生提取或模型多模态输入。`builtin.read_file` 的派生投影覆盖 Notebook、PDF、Word、Excel、PowerPoint、压缩包安全清单与 **SQLite**（`.db`/`.sqlite`/`.sqlite3`，只读打开，投影对象清单 + schema + 每表有界数据行）；派生文档不产生 Hashline 快照，不能直接参与 Hashline 编辑，需要修改时改用 `edit` 的 PathDiff 或 `*** Add File:` 契约。
- 需要卸载独立、聚焦的工作（只读检索、局部摘要、有边界的实现或低风险核验）时用 `builtin.delegate_to_fast`：**默认后台**委派，立即返回 `jobId`，子代理完成时运行时把结果经完成通知回流到下一轮；相互独立的委派放在**同一条 assistant 消息**里批量并发发起；子代理运行期间继续做不依赖它的工作，不要反复 `job_output` 空转或空等。只有下一步动作依赖该结果时才传 `run_in_background: false` 同步等待。`task` 必须是一份完整独立的 prompt（目标、范围、证据、交付要求），因为子代理看不到本次对话。
- 已存在的子代理用 `builtin.send_message` 追加消息，`builtin.list_agents` 巡检当前代理树。`send_message` 的投递语义按目标状态分三种：目标**正在运行**时消息进入它的 mailbox，由它在最近的 step 边界读取（回执 `deliveryState: "delivered"`）；目标**空闲或已恢复**（已终态但未关闭）时消息仍会送达，并由运行时重挂它的会话、开启新轮次（回执 `wakeRequired: true`，`deliveryState: "delivered"`）；目标**不可达**（已关闭或已回收）时降级为排队，回执 `deliveryState: "pending"`，消息已持久化并等待重试，可用 `axon debug mailbox` 查询。**寻址规则写死**：生命周期操作（读输出、等待、终止）一律走 `job_id`：后台子代理的回执带 `jobId`，用 `builtin.job_output` 读取结果与终态、`builtin.job_kill` 完整中断（中止 turn、关闭会话、投影终态并触发完成通知）；协作寻址（发消息、列表）一律走 `AgentPath`。两者不可互传，`job_id` 传给子代理工具或 `AgentPath` 传给作业工具都会被稳定拒绝。
- 需要建立、检索、查看、启动、修改、暂停、继续或取消跨轮 Goal 时，分别调用当前工具列表中的 `builtin.goal_create`、`builtin.goal_list`、`builtin.goal_get`、`builtin.goal_start`、`builtin.goal_update`、`builtin.goal_cancel`。Goal 没有面向用户的启动确认步骤；主 turn 根据用户目标自主决定是否创建和启动，也可自主修改、暂停、继续或取消。Goal 查询必须服从当前 Workspace、audience 和 disclosure policy；写入回执成功后再向用户报告，后台工作不需要本轮同步等待结果，完成后通过 Axon 的正常回流路径通知。短结果直接回流，超过 400 个 Unicode 字符时通过完成通知提供摘要和完整结果文档路径。

## 任务方式

- 简单查询或检查：先选择最窄的只读工具，读取回执后直接回答。
- 本地编码：先定位和读取必要文件，使用 `edit` 或 `shell_command` 完成明确修改，再运行相关检查并读取结果。
- 需要既有操作流程：先调用 `invoke_skill` 加载一个有效 Skill，再遵循其操作说明。Skill 本身不替代实际工具调用。
- 需要外部能力：直接在已下发的工具清单里确认是否可用；清单中没有对应工具时判定该能力不可用并选择已验证的降级路径。不能因工具未出现而臆造调用。
- 有状态变更：先确认目标、范围和副作用；完成后读取工具回执、session、状态或产生的文件进行核验。

## 工具回执

- 工具已返回不等于任务已完成：读取 status、输出和错误，必要时按回执继续下一步。
- 保留 session ID、request ID 和非敏感错误信息，便于复现和审计。
- 出现参数错误时，先重新查看当前工具说明和上一条回执，再只提交一次符合说明的调用；不要试探未声明字段。
- 编辑回执先看 `strategy`（`hashline` / `apply_patch` / `replace`）与 `files[].operation`（`add` / `update` / `delete` / `move`）：整文件重写命中已存在文件时 `operation` 为 `update`，不要据此误判为"只改了几行"。回执携带的 `files[].hashline.header`（新 `[path#TAG]`）是同一文件后续 Hashline 编辑的定位依据，优先复用而不是重新读取。
- 回执 `warnings` 中的 `[解析回归] <path>` 表示**本次编辑把该文件从「可解析」变成了「解析失败」**（约行区间会一并给出）：写入已落盘、未被回滚，必须立即检查该区域的定界符、缩进与括号配对并修正；`[修复] ` 前缀则表示运行时改写了提交的原文，需要复核修复结果。
- 编辑载荷在流式生成期间会被投影为**只读 diff 预览**（PathDiff 与服务端 grammar 通道；Hashline 载荷会按当前文件内容渲染真实前后对比）。预览只读、不落盘，也不代表最终结果，以写入回执为准。

## 常用 CLI

- 全局：`axon --version`、`axon --help`、`axon <command> --help` 用于确认当前版本和命令。需要指定配置或工作区时，把 `--config <path>` 或 `--cwd <path>` 放在 `axon` 后、子命令前。
- Agent：`axon run` 执行一次有界任务，支持 `--model`、`--reasoning-effort`、`--session`、`--continue`、`--file`、`--tool`、`--timeout` 和 `--format json`。接续既有会话用 `axon run --continue` 或 `--session <id>`；`axon stop [--session <id>]` 请求停止非交互任务。每次 `axon run` 都是一条**独立主 turn 车道**（独立会话表面），不指定 `--continue`/`--session` 时**不会**继承上一次 CLI 调用的上下文；同一工作目录下并发多次调用互不干扰。
- 会话诊断：`axon debug session list|show|read|replay|doctor` 用于只读检查，`resume|fork|delete|export|output` 改变会话；该命令位于 `debug` 命名空间，用户不需要也不应管理 session_id。
- 本地执行与变更：`axon exec [--timeout-ms <ms>] [--kind command|test] <command...>` 执行本机命令；文件编辑由已获授权的工具调用直接完成并记录审计。
- 配置、认证和模型：用 `axon config path|env-path|validate|show|keys|get` 先检查，`config edit|set` 才会写入。用 `axon providers`、`axon models`、`axon auth status` 查看状态，只有用户要求时运行 `setup`、`providers add|remove` 或写入密钥。
- Skill 与 MCP：用 `axon skills --summary` 查看有效 Skill；用 `axon mcp list|get` 检查 MCP server。添加、移除、登录或退出前必须读取相应子命令帮助并取得用户授权。
- Memory 与使用统计：`axon memory stats|doctor|list|search|conflicts|compare|quality|review|inspect` 和 `axon usage` 是检查入口。`memory consolidate|consistency|recover-conflict|approve|archive-pending|quarantine|forget|import` 会改变状态，必须先确认范围。
- 诊断和导出：`axon doctor [--check <area>]`、`axon lsp servers|workspace-symbol|definition|references|diagnostics`、`axon debug builtin`、`axon debug channel-metrics` 用于定向检查。`axon debug channel-metrics` 按时间、模型和 Provider 查询已持久化的渠道性能、缓存与连接指标；`axon export` 输出运行数据；输出到文件前确认路径。
- Daemon、Cron 和 Remote：`axon daemon status`、`axon daemon health`、`axon cron list`、`axon remote status|feishu-status|subscriptions|outbox|audit` 为只读检查。`axon daemon install|uninstall|start|stop|restart|drain`、`cron create|update|pause|resume|remove|run|tick`、远控订阅撤销和投递/worker 命令都会影响运行状态或外部服务，需用户明确授权。
- 图片、服务、打包和升级：图片生成或编辑由已加载 Skill 完成，可能产生费用；`axon serve` 会监听本地端口；`axon package` 生成发布包；`axon upgrade --apply|--rollback` 改变后续 Axon 会话使用的版本。执行前都要确认目标与副作用。
- HTTP 接口：Gateway 面随 Daemon 常驻（`remote.gateway.http_bind`），`axon serve` 是独立的前台 headless server。路由全表、报文体与错误码见 [references/gateway-http.md](references/gateway-http.md)。
- WebSocket 接口：`ws://<remote.gateway.websocket_bind>/gateway/v1/ws`，一条连接可并存多路订阅。帧协议与排障口径见 [references/gateway-websocket.md](references/gateway-websocket.md)。
- 单个接口排障：两个面各有一个调用脚本（`scripts/gateway-http.ps1`、`scripts/gateway-ws.ps1`），可只调一个接口并打印原始回执。
- 桌面工作台：`axon desktop` 经稳定 launcher 启动窗口；`axon-desktop.exe --self-check` 输出单行 JSON 自检，`--serve <端口>` 起本地调试桥。入口、消费的网关面、状态文件与可调用命令族见 [references/desktop-app.md](references/desktop-app.md)。

## 升级与运行连续性

- 安装根目录使用 `versions/<version-id>/` 保存不可变版本，`current.json` 记录下一次启动版本；PATH 入口与 Daemon 服务通过稳定 `launcher/axon-launcher` 启动。升级重启 Daemon 后会自动回收无 lease 的旧版本目录。
- 升级前先运行 `axon upgrade` 检查候选版本；只有用户明确要求时才运行 `axon upgrade --apply` 或 `axon upgrade --rollback`。
- 已经开始的 CLI、TUI 或 Agent 任务应继续等待其正常完成。升级结果只影响之后启动的 Axon 会话；不要把正在运行的会话当作已自动切换版本。
- 升级完成后，在新的终端或新的 Axon 会话中运行 `axon --version` 核验版本；当前会话显示的版本不能作为升级后的核验依据。
- Daemon 运行态分两层看：`axon daemon status` 只回答 `installed` 与 `running`；内部健康快照用 `axon daemon health`，它给出 `host.status`（`running`、`upgrade_requested`、`admitting_shutdown`、`handoff_ready`、`stopping`、`stopped`）、`host.leaseRemainingMs`、`executions`、`executionBacklog.pending`、`deliveries.backlog.pending` 与 `streamSubscribers`。判断「是否还有在跑的工作」看这两个积压字段是否归零。
- `axon daemon drain [--reason <text>]` 经本地 IPC 请求排空：停止接收新的 Daemon 事件与新的任务领取，让已开始的任务自然完成。Daemon 异常退出或重启后会恢复未完成的已接收事件，重复入站不会创建新的处理任务。这是会改变运行状态的操作，必须先取得用户明确授权。
- `axon daemon stop|restart` 是服务级操作：先写停止请求让 Daemon 自行收口退出（等待窗口 15 秒），未响应才禁用并结束用户计划任务，Windows 上再以 `taskkill /T /F` 强制结束进程树（macOS 用 `launchctl bootout`，Linux 用 `systemctl stop`）。强制结束会中断仍在执行的任务。要优雅收口必须先 `drain`，用 `axon daemon health` 确认两个积压字段归零，再 `stop`。
- 停止、重启、重新安装或卸载完成后，重新读取 `axon daemon status` 与 `axon daemon health` 核验结果。
- 升级或回滚失败时保留命令回执与非敏感错误信息。不要为重试而终止其他 Axon 会话、删除 session、Memory、配置或运行记录。
- Shell 补全：`axon completion power-shell|bash|zsh|fish` 输出补全脚本；根据用户的 shell 选择一种。

## 执行与核验

- 写入配置、工作区、Memory、session、Daemon、Cron、Remote、MCP 或外部服务前，先确认目标和实际参数；仅执行用户要求的范围。
- provider、图片生成和外部投递可能产生费用或副作用，执行前说明目标；完成后读取对应回执、status、session 或 artifact，不以自然语言回复代替验证。
- 失败时保留非敏感的错误、request/session ID 和工具回执；不要删除配置、session、Memory 或缓存来掩盖问题。

## 按需参考

- 需要选择运行入口时读 [references/capabilities.md](references/capabilities.md)。
- 需要准确拼写 CLI 命令或了解当前命令的核验方式时读 [references/cli-reference.md](references/cli-reference.md)，随后仍以该命令的 `--help` 为准。
- 需要配置 provider、模型、MCP 或 Skill 时读 [references/configuration-extensions.md](references/configuration-extensions.md)。
- 需要执行 Agent 任务、查看 session、使用 Memory 或运维 Daemon、Cron、Remote 时读 [references/operations.md](references/operations.md)。
- 需要通过 HTTP 接口读写运行态、提交外部事件或查询处理结果时读 [references/gateway-http.md](references/gateway-http.md)，它同时给出本 Skill 目录下 `scripts/gateway-http.ps1` 的用法。
- 需要订阅实时事件、排查 WS 帧或驱动桌面调试桥时读 [references/gateway-websocket.md](references/gateway-websocket.md)，它同时给出 `scripts/gateway-ws.ps1` 的用法。
- 需要启动或排查桌面工作台、用调试桥或 CDP 驱动界面时读 [references/desktop-app.md](references/desktop-app.md)。
