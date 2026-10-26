# Daemon 后台运行

Daemon 是 Axon 的后台长生命周期宿主。当前实现已经把 `PrimaryTurnService` 接入 supervisor tick：Daemon 从 durable Interaction inbox 领取输入，按 Workspace lane 有界并发，并通过 lease token 提交 `Understood` 或 `Rejected` 终态。

主 turn 的并发单元是复合 lane（Workspace + trust scope + channel + destination），不是批次：`jarvis_interactions.lane_id` 是领取排他的持久化事实源，某条 lane 已有未过期 `processing` 时，该 lane 的后续记录不会被领取，同 lane 串行无需调用方维护在途状态。协调线程在同一批内只等待「任意一个 lane」结束，槽位一归还就补位领取并投递，因此某条 lane 的长 turn 不会推迟其他空闲 lane 的新消息；只有并发槽位（默认 4 条 lane）全满时才等待 lane 结束。槽位未满时以 250ms 有界窗口复查新到输入，窗口只是补位周期，不改变 durable 领取顺序。

## 当前职责

- 处理 `SubmitInteraction`，不接受旧 `SubmitAction`、`CancelAction`、`GetQueue` 或 `StopSession` 外部命令。
- 使用 Workspace registry 将 `WorkspaceId` 解析为真实工作目录。
- 调用 `InteractionTurnRunner` 完成普通问答或原子创建 Goal、首个 Execution 与 Event。
- 托管 Scheduler、飞书 transport、事件桥、投递与维护 tick。
- 维护内部 host generation、lease 与 fencing 状态。
- 由 Execution Worker 线程池接管已提交 Execution，前台不活跃时写入未读 Result 并入队持久化 Delivery。

Goal 创建后的执行已经收口：Execution 由 Daemon 的 Execution Worker 池领取，并通过 `ProviderRunSubagentExecutor` 作为隔离子代理执行；结果写入 Result 与持久 completion notification。一个 Execution 只通知主 turn 一次，通知包含来源（一次性 Goal 或 Axon cron）、Goal 目标、执行结果及结果文档路径；主 turn 产出的汇总回复随后登记到所有可用渠道的独立 delivery。TUI 只有真实展示后确认，Feishu 只有写入 durable outbox 后确认，任何渠道都不允许静默 ack。没有可用渠道时收件箱持久保留；跨过创建后的下一个本地自然日仍未读则自动失效并静默停止通知。

Agent cron 按投递需求分流：需要投递的 cron 到期后创建 `GoalToolCommand::Create`，复用 Goal 的 Execution、Result、完成通知和多渠道投递；不需要投递的 cron 直接调用同一 `ProviderRunSubagentExecutor`，不进入用户渠道。子代理结果超过 4000 个 Unicode 字符时以 Markdown 原子写入统一内容归档 `.axon/data/archive/*.md`，主 turn 只接收有界预览和归档地址。
主 turn 尚未产出汇总或执行失败时，后台 Execution 的原始回复不得作为渠道正文发送；通知继续留在持久收件箱，直到主 turn 汇总成功后投递，或按跨日未读规则自动失效。投递 sweep 每轮按有界批次补偿两类窗口：汇总已落库但渠道推进中断，以及通知结算后新上线渠道的补登记；正文为空的通知不得推进，渠道行不得永久滞留在 pending。

Daemon 的维护清理覆盖 client presence、completion notification、completion delivery 以及既有 Interaction/Goal/Execution/Result 等运行态数据。清理使用索引、有界批次、IMMEDIATE 短事务和整轮时间预算；只删除 delivered/expired 且超过保留期的 delivery，随后删除无未终结子 delivery 的 notification，pending/dispatched 一律保留。

关闭 CLI/TUI 不影响已提交 Execution 继续运行，重新接入后按 cursor replay 取回结果。

## 服务管理

安装包可注册 Daemon 服务；也可以使用 CLI 管理当前安装：

```text
axon daemon install
axon daemon status
axon daemon start
axon daemon stop
axon daemon restart
axon daemon uninstall
```

Windows 使用当前用户 Windows Task Scheduler 任务 `AxonDaemon`，登录时以 `InteractiveToken + HighestAvailable` 启动隐藏 PowerShell 7 包装进程并等待 `axon daemon run` 退出，将退出码传回任务计划程序。注册时解析并校验 `pwsh.exe` 绝对路径，运行时不再搜索 PATH。`RestartOnFailure` 在失败后每隔 5 分钟重试，最多 3 次；手动停止不会触发失败恢复。任务允许电池供电下继续运行。

包装进程的控制台输入、输出和 `$OutputEncoding` 均设为 UTF-8 无 BOM，`$PSStyle.OutputRendering` 为 `PlainText`，不经过表格格式化或添加 ANSI 颜色。`axon run --format json` 仍使用 JSONL 契约。任务 XML 暂存文件按 Windows Task Scheduler 的要求使用 UTF-16LE，该编码不影响日志和 CLI 输出。

`shell_command` 启动的 PowerShell 子进程也会显式初始化输入、输出和管道编码为 UTF-8 无 BOM；PowerShell 7 使用 `PlainText` 输出，避免新建子进程回到系统代码页或默认颜色模式。此设置通过启动脚本前缀完成，不增加额外进程或探测调用。

Windows 后台非交互子进程统一通过 `axon-process` 设置 `CREATE_NO_WINDOW`，包括 Shell、MCP、LSP、Workspace Git 探测和 Host 存活探测。隐藏 Daemon 包装窗口不等于隐藏所有后代进程；独立后台 worker 的每个非交互启动边界都必须遵守同一策略，避免短生命周期控制台闪现。用户明确打开的浏览器和外部编辑器不受该策略约束。

Daemon 保留当前用户身份、用户级 PATH 与 `USERPROFILE`，Shell 直接继承 Daemon 环境和权限，无需独立 Shell Worker；PowerShell 包装器使用 `-NoProfile`，不会加载 PowerShell 配置脚本。启动快照仍在 Daemon 启动时预热。管理员用户的 Daemon 与工具具有管理员权限，必须限制外部渠道访问并审核高风险操作。

最高权限任务的注册与更新需要管理员权限，安装器通过 UAC 请求授权，不关闭 UAC。授权必须使用同一用户，安装器拒绝切换到其他账号。停止不需要提权：`axon daemon stop` 写停止请求让 Daemon 自身 drain 退出（等待窗口 15 秒），只有它未响应而必须强制结束进程树时才需要管理员权限。用户未登录时任务不会运行，Feishu、HTTP、WebSocket 等监听也不可用，外部系统需要重试或持久化事件。Linux 用户单元使用 `Restart=always`，macOS LaunchAgent 使用 `KeepAlive`；进程重启后宿主按 lease 收敛中断中的 Execution。

Windows IPC 命名管道在启动时显式授权当前用户 SID、SYSTEM 与管理员，并采用中完整性标签，使同一用户的普通权限 CLI/TUI 可以连接最高权限 Daemon；不授予其他普通账户访问权。连接后的工具执行继承 Daemon 的最高权限，因此本地同一用户客户端也属于可信边界。

订阅客户端用「阻塞写 + 非阻塞轮询读」完成握手与持续推送，从而在 Daemon 卡死时按截止时间失败返回而不是永久阻塞。Windows 命名管道的 `PIPE_NOWAIT` 切换在管道尚未完全建立或处于繁忙状态时会瞬时失败（`ERROR_PIPE_BUSY`/`ERROR_NO_DATA`/`ERROR_PIPE_NOT_CONNECTED`）：这类管道状态错误在截止时间内重试，超期仍失败才返回最后一次真实错误——既不把可恢复状态当成硬故障，也不突破超时契约。

运行数据统一位于用户级 `~/.axon/data`（Windows 为 `%USERPROFILE%\\.axon\\data`）；安装目录只保存 `versions/<CLI版本>`、稳定 launcher 与 `current.json`，不承载数据库或日志。升级前会停止受管 Daemon 和桌面端，再替换对应 CLI 版本目录，避免同一版本重复安装产生随机目录。具体文件路径仍由配置根解析，不应由外部调用方拼接数据库文件名或内部 Session ID。源码覆盖安装会直接清理旧安装目录下遗留的 `data` 目录。

## Interaction 流程

```text
SubmitInteraction
  -> durable append / idempotency
  -> PrimaryTurnService claim
  -> workspace resolution
  -> InteractionTurnRunner
  -> Answer 或 Goal transaction
  -> Interaction terminal state
```

CLI 与 TUI 不再自行运行前台 turn：它们只提交 Interaction 并订阅 Daemon 事件流，Provider 执行与 lease 领取只发生在 Daemon 内。前端源码中不允许出现 Daemon 专有的领取与执行入口，该约束由源码守卫测试固定。

## 外部事件入口

Gateway 把外部系统的事件收敛为同一条 durable Interaction 输入，不新增存储、队列或执行路径。事件入口有三条路由：

- `POST /gateway/v1/events`：通用信封，语义与既有实现一致，`sourceChannel` 由报文指定，不调用 provider 解析脚本。
- `POST /gateway/v1/events/{provider}`：provider 已完成归一化的信封，报文体即脚本输出契约。
- `POST /gateway/v1/events/{provider}/raw`：provider 原生 Webhook 报文，交给该 provider 的 `parser.py` 归一化。

三条路由共用既有鉴权、连接限额与 64KB 入站上限。凭据有两个通道，任一命中即通过：`Authorization: Bearer <token>`，或 URL 查询串 `?api_token=<token>`。查询串通道是为无法配置 HTTP header 的发送方（典型是 Jira Server WebHooks，其配置界面只能填 URL）准备的；它不做百分号解码，token 必须由 URL 安全字符构成。由于 token 会出现在 URL 里，该通道应只在受信内网或反向代理之后使用，并避免在中间层记录完整 URL。provider 名只允许小写 ASCII 字母、数字、`-` 与 `_`；未注册的 provider 直接按 404 拒绝，不进入耐久存储。

以 Jira Server 为例，WebHook 地址填 `http://<axon 主机>:3143/gateway/v1/events/jira/raw?api_token=<token>` 即可，Jira 侧无需任何 header 配置。

监听地址由两条配置项决定：`remote.gateway.http_bind`（默认 `127.0.0.1:3143`）与 `remote.gateway.websocket_bind`（默认 `127.0.0.1:3144`）。需要被跨网段的外部系统投递时，把 HTTP 入口改为 `0.0.0.0:3143`，并在主机防火墙上放行该端口的入站规则（例如开放 TCP 3143 端口）；WebSocket 入口保持环回，不对外放行。端口一旦暴露在网络上，凭据就是唯一访问门槛，务必使用高强度随机 token，并优先在反向代理之后只暴露该路径前缀。

两条入口的接入路径都不承载单个慢客户端的等待，这条并发契约必须保持：HTTP 面由 axum（hyper + tokio）承载，框架负责连接循环与请求解析，网关自身不再持有固定数量的工作线程；请求头解析、正文读取与 durable 入站中的同步部分（运行态存储读写、provider 解析脚本、事件落库）一律在阻塞池执行，因此慢请求只占阻塞池的一个名额，不会停掉整条接入面。阻塞池并发上限 64，排队预算 5 秒，超时返回 429 `gateway_busy`；内核连接许可（默认 256）仍然是更外层的准入。WebSocket 入口的全部连接共享单线程 runtime，durable 桥调用（事件提交、订阅重放）同样只在阻塞池执行，连接收口先关闭事件接收端再回收投影线程。

WebSocket 的 `event` 帧包含 `subscribeId`、`event_type`、`payload_json`、`cursor` 与布尔字段 `replay`。初始订阅和断流补偿中读取的 durable 历史事件使用 `replay: true`，实时流转发使用 `replay: false`。客户端应照常显示回放内容，但仅对实时终态发送新结果提示，避免切换会话或重连后反复推送历史通知。订阅一个**已经终态**的 Interaction 时，这次回放还会在全部 durable 事件之后补一条 `interaction.status` 终态帧（`payload` 为状态字符串，`replay: true`，无 `cursor`）：终态平时只由实时通道在状态跃迁时发出，早已结束的回合不会再跃迁，缺了这条帧客户端就没有收口依据；`received` 与 `processing` 不补，由实时跃迁给出。

provider 内容资产位于 `~/.axon/events/{provider}/`，目录名即 provider 名，其中 `parser.py` 必填、`sop.md` 可选。仓库内只保留输出契约 `crates/axon-daemon/src/infrastructure/event_script_schema.json` 与测试夹具，不存放任何 provider 业务脚本；新增外部系统只需新增目录，Rust 代码零改动。

解析脚本从 stdin 读取 UTF-8 JSON `{provider, received_at_ms, payload}`，向 stdout 写出 `{event_id, correlation_key?, title, content}`。`event_id` 必须带 `{provider}:` 命名空间，且不得掺入接收时间或随机数，否则 durable 幂等键失效。脚本以非交互方式启动：环境变量清空后只注入白名单项与 `PYTHONUTF8=1`，拿不到任何凭据；超过 2 秒按超时终止并整树回收进程。脚本缺失、崩溃、超时或输出不合契约时事件一律 fail-closed 丢弃并留下审计行，分别返回 404 / 504 / 502。

归一化后的事件在进入 durable ingress 前有两条增强：`content` 超过 4000 个 Unicode 字符时原子归档为 `~/.axon/data/archive/*.md`，Interaction 正文只保留包含归档路径与 `sop.md` 路径的简报，由主 turn 用 `builtin.read_file` 按 offset/limit 分页读取；带关联键的事件（provider 路由未显式给出时回落为 `event_id`）按该键做 3000ms 滑动窗口合并，窗口内的同类事件只唤醒主 turn 一次。事件有效期 24 小时，过期事件按既有策略拒绝。事件 durable 入账成功后立即唤醒主 turn，与 CLI/TUI 的 IPC 提交走同一条唤醒路径，不再依赖 1 秒维护定时兜底；被防抖吸收或幂等命中的提交不产生新记录，因此也不重复唤醒。

外部事件没有调用方 cwd，因此固定绑定 Daemon `--root` 工作目录派生的 Workspace：该 Workspace 由守护进程启动时注册，与 CLI、cron 提交侧的契约一致（提交方负责注册，主 turn 只按 id 查找），未注册时主 turn 会在打开运行时阶段拒绝事件。事件驱动的工作都在这个工作区内进行，仓库操作由 `sop.md` 用绝对路径表达，引擎不为外部事件引入第二套执行身份。

## Turn 投影时序

Turn 执行过程中，Daemon 按 Provider 生产顺序消费 wire 事件。首个可见文本立即落库，后续同一 assistant/reasoning part 的连续 delta 在 12ms 或 16KiB 边界内批量提交；工具、审批、生命周期等语义边界会先强制刷新文本批次。每个批次使用一次 SQLite 事务取得连续 durable sequence，并严格按该 sequence 发布到 TUI、飞书、HTTP 与 WebSocket 订阅。Interaction 的实时批次与终态收口都在同一事务内校验当前 processing lease；supervisor 收口为 interrupted 后，旧 worker 的 delta、usage 或 completed 事件不会再进入 durable sequence。

Provider 的每次成功流固定产生 `StreamingStarted -> delta/reasoning/usage/tool -> StreamingCompleted`。整轮收口只追加记忆、技能、治理、运行错误、canonical assistant 等不可由实时增量替代的稳定快照，最后追加唯一 terminal `RunLifecycle`，再写入 `turn_completed`。实时 reasoning、tool、file、plan 与 usage 不在收口阶段重建；投影异常也不会把旧过程事件迟到插回日志。CLI/TUI 与 Gateway 订阅在同一 Workspace lane 回放时均按 `interactionId` 过滤，并直接消费 durable sequence，不建立第二套重排或完成门禁。

整轮的顺序契约是 `canonical assistant -> foreground/run completed -> turn_completed`。canonical 终止事件（最终 assistant 与 terminal `RunLifecycle`）在同一批 `publish_batch` 内原子提交；该批次失败必须终止本轮并形成结构化失败结论，禁止在 durable 缺口上继续写入 `turn_completed`。成功但无正文的回合仍提交 canonical assistant（内容为空），因此该契约对所有成功回合都成立。

文件差异在工具执行过程中按稳定 part ID 更新：流式参数只能形成预览，工具回执产生的实际结果会在同一实时序列中原地替换预览。终态收口不重新创建旧 `ToolCall`、`ToolResult` 或 `FileChange`。

`axon run` 通过同一 Daemon durable/realtime 订阅链路接收当前 Interaction 的过程 wire 事件并等待 `turn_completed`；Text 模式会把 assistant 增量实时写到 stdout，canonical 终态用于完整性校验与缺失正文兜底，JSON 模式保持终态 JSONL 契约。

## 启动快照复用

Daemon 是长驻进程，`CliProviderTurnRunner` 持有进程级 `StartupSnapshotCache`：启动后立即在后台线程按运行态根目录预热首份启动快照，主 turn 直接复用，不再每轮现场重建 Principal、Personal Memory、User Agents、Workspace 指令、Skill catalog 与 MCP registry。

缓存以「规范化 config 路径 + 规范化 cwd」为键，最多保留 4 条，每条记录带 TTL 与资源指纹。TTL 内直接命中且不做任何文件扫描；TTL 到期后先计算轻量指纹（用户级与工作区 AGENTS.md、`.mcp.json`、`.axon/skills`、用户级 memory 存储目录的 mtime / 大小 / 条目数）：指纹不一致，**或上一次快照存在探测失败的 MCP server**，都会重建；两者都不成立才只续期。缓存缺失或构建失败时返回 `None`，本轮自动回退到现场构建，语义与未接入缓存前完全一致。

指纹只做元数据扫描，不读取文件内容，且目录深度与条目数都有上限，避免异常目录拖慢 turn。

显式刷新走 IPC 命令 `RefreshMcpCapabilities { cwd }`：Daemon 清除该运行态根目录的 MCP 健康负缓存，作废对应 Workspace 的快照条目，再在后台以**绕过健康负缓存与 schema 缓存**的方式重建快照。TUI `/mcp` 与 OAuth 授权成功后的自动刷新都会提交该命令，因此恢复的 MCP 工具在下一轮 turn 就进入 Provider 工具表，而不是只刷新前端目录。该命令由 IPC 层直接处理，不进入 `DaemonHost` 的命令分发；探测在后台线程执行，IPC 不等待，下一次 `acquire` 会等这次构建结束后复用其结果。

## Feishu 入口

飞书只使用私有 WebSocket 入站。治理层接受的普通消息被转换为 Workspace-scoped Interaction，并通过 `SubmitInteraction` 交给 Daemon；旧 SessionAction payload 和 active run steer 旁路已删除。工具调用不再提供远端审批控制，旧审批卡片与命令会被明确拒绝，也不会转换为普通 Interaction。

主 turn 被永久拒绝（Provider、Renderer 或 Runtime 层失败）时不会产生 `turn_completed`，因此 `PrimaryTurnService` 除了把 Interaction 置为 `rejected` 并保留分层日志，还把来源渠道、投递目标与入站消息幂等键交给组合根；组合根对飞书渠道复用同一条 artifact + durable outbox 通道补发 Warning 级失败回执（标题「Axon 处理失败」），并回帖到用户原始消息，非飞书渠道显式跳过。回执 dispatch 由 Interaction 稳定派生，重复调用不会产生第二条消息；入队失败只降级为告警，不影响已提交的拒绝终态。

## 健康与容量

`axon daemon health` 通过 `GetHealth` IPC 读取内部聚合指标，覆盖 Interaction、Goal、Execution、投递积压与实时事件流订阅数，不要求调用方提供 Session ID，也不暴露 Session、lease 或 generation 管理入口。

`streamSubscribers` 是当前实时事件流订阅数。网关一条连接可以并存多路订阅（主会话车道流与子代理会话流各占一路），因此这个数就是「Daemon 侧此刻在为几路订阅投递事件」；窗口在会话之间切换、关闭子代理视图之后它必须回落，是订阅是否真的释放的唯一可观测判据。

```text
axon daemon health
```

容量、故障与恢复验收脚本（均使用真实 SQLite 事实源与真实 DaemonHost，只把 Provider 执行替换为本地执行器）：

```powershell
pwsh -File tools/verify-jarvis-capacity.ps1   # R7-06：backlog 与并发入口容量
pwsh -File tools/verify-jarvis-soak.ps1       # R7-07：常驻 soak 与投递重试 soak
bash tools/verify-daemon-restart.sh           # R7-01/09：真机重启接管，需 JARVIS_REAL_DEVICE=1
```

Windows 实测基线（`--release`，1k/10k backlog 与 8 线程并发入口）：

| 场景 | P50 | P95 | 总耗时 | 常驻内存 | WAL |
|---|---|---|---|---|---|
| 1k backlog | 1.78ms | 2.65ms | 1.94s | 8.96MB → 9.84MB | 4.14MB |
| 10k backlog | 2.22ms | 3.51ms | 23.34s | 11.61MB → 11.87MB | 4.17MB |
| 并发入口 8 线程/1000 动作 | 1.19ms | 1.62ms | 2.29s（另有 2.04s 结算） | 8.87MB → 13.63MB | 4.15MB |

事实源存储策略固定为：进程内连接池 + WAL + `synchronous=FULL`。`synchronous=FULL` 是因为已提交的外部副作用事实必须承受断电级故障，崩溃恢复只依赖已提交记录，不做重放猜测。写事务统一使用 `TransactionBehavior::Immediate`，使写锁争用落到 `busy_timeout` 上而不是立即返回 `SQLITE_BUSY`。写锁等待上限生产默认为 5 秒；`SqliteRuntimeStore::with_busy_timeout` 允许注入更小的上限，使「超过等待上限后有界失败且不留下幽灵 claim」的确定性验证不必空转完整生产超时。

## 升级与恢复边界

launcher 和服务管理模块提供版本原子切换、心跳保活与进程生命周期管理。守护进程支持崩溃后由系统服务自动拉起，对超期的 Execution 租约执行自动接管；当外部副作用执行状态未知时自动收敛至 `uncertain` 保护状态，杜绝非幂等副作用的盲目重复执行。网络断开时具备指数退避重连、待投递事件排空与消息幂等去重能力。

## 相关文档
- [系统概览](./overview.md)
- [核心概念](./concepts.md)
- [开源贡献与分层架构](../CONTRIBUTING.md)
