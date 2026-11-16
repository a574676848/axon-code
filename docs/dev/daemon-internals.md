# Daemon 内部实现

本文记录守护进程的运行机制、并发模型、事件入口细节、投影时序与保留策略，属开发者文档。用户可见的服务管理命令与行为见 [`../daemon.md`](../daemon.md)。

---

## 1. 领取与并发模型

Daemon 是 Axon 的后台长生命周期宿主。当前实现已经把 `PrimaryTurnService` 接入 supervisor tick：Daemon 从 durable Interaction inbox 领取输入，按 Workspace lane 有界并发，并通过 lease token 提交 `Understood` 或 `Rejected` 终态。

主 turn 的并发单元是复合 lane（Workspace + trust scope + channel + destination），不是批次：`jarvis_interactions.lane_id` 是领取排他的持久化事实源，某条 lane 已有未过期 `processing` 时，该 lane 的后续记录不会被领取，同 lane 串行无需调用方维护在途状态。协调线程在同一批内只等待「任意一个 lane」结束，槽位一归还就补位领取并投递，因此某条 lane 的长 turn 不会推迟其他空闲 lane 的新消息；只有并发槽位（默认 4 条 lane）全满时才等待 lane 结束。槽位未满时以 250ms 有界窗口复查新到输入，窗口只是补位周期，不改变 durable 领取顺序。
## 当前职责

- 处理 `SubmitInteraction`，不接受旧 `SubmitAction`、`CancelAction`、`GetQueue` 或 `StopSession` 外部命令。
- 使用 Workspace registry 将 `WorkspaceId` 解析为真实工作目录。
- 调用 `InteractionTurnRunner` 完成普通问答或原子创建 Goal、首个 Execution 与 Event。
- 托管 Scheduler、事件桥、投递与维护 tick。
- 维护内部 host generation、lease 与 fencing 状态。Host lease 续租按 10 秒周期执行，租约有效期 30 秒；续租遇到存储抖动（`database is locked`）时按 750ms 持续重试，只有实际失去代际（另一 Host 已接管）才停止全部后台 Worker。停机请求在下一个重试点生效，不等待退避结束。
- 由 Execution Worker 线程池接管已提交 Execution，前台不活跃时写入未读 Result 并入队持久化 Delivery。

Goal 创建后的执行已经收口：Execution 由 Daemon 的 Execution Worker 池领取，并通过 `ProviderRunSubagentExecutor` 作为隔离子代理执行；结果写入 Result 与持久 completion notification。一个 Execution 只通知主 turn 一次，通知包含来源（一次性 Goal 或 Axon cron）、Goal 目标、执行结果及结果文档路径；主 turn 产出的汇总回复随后登记到创建 Task 时声明的唯一投递目标，目标取自 Execution 的 `transportContext`，不再广播到当时在线的其它客户端。TUI 只有真实展示后确认，桌面通知只有写入 durable outbox 后确认，任何渠道都不允许静默 ack。跨过创建后的下一个本地自然日仍未读则自动失效并静默停止通知。

Agent cron 按投递需求分流：需要投递的 cron 到期后创建 `GoalToolCommand::Create`，并把 `channel`/`destination` 写入 Execution 的 `transportContext`，复用 Goal 的 Execution、Result、完成通知与定向投递；不需要投递的 cron 直接调用同一 `ProviderRunSubagentExecutor`，不进入用户渠道。子代理结果超过 4000 个 Unicode 字符时以 Markdown 原子写入统一内容归档 `.axon/data/archive/*.md`，主 turn 只接收有界预览和归档地址。
主 turn 尚未产出汇总或执行失败时，后台 Execution 的原始回复不得作为渠道正文发送；通知继续留在持久收件箱，直到主 turn 汇总成功后投递，或按跨日未读规则自动失效。投递 sweep 每轮按有界批次补偿「汇总已落库但渠道推进中断」这一窗口；正文为空的通知不得推进，渠道行不得永久滞留在 pending。目标在登记时即已确定，不存在「等渠道上线再补登记」的路径。

Daemon 的维护清理覆盖 client presence、completion notification、completion delivery 以及既有 Interaction/Goal/Execution/Result 等运行态数据。清理使用索引、有界批次、IMMEDIATE 短事务和整轮时间预算；只删除 delivered/expired 且超过保留期的 delivery，随后删除无未终结子 delivery 的 notification，pending/dispatched 一律保留。

关闭 CLI/TUI 不影响已提交 Execution 继续运行，重新接入后按 cursor replay 取回结果。

---

## 2. Interaction 流程



```text
SubmitInteraction
  -> durable append / idempotency
  -> PrimaryTurnService claim
  -> workspace resolution
  -> InteractionTurnRunner
  -> Answer 或 Goal transaction
  -> Interaction terminal state
```


---

## 3. 外部事件入口与接入细节


`/api/v1/*` 的正文上限按**每路由**分别判定，未登记的路径统一按 4 MiB 收体（不回落到 router 级的既有上限）。判定顺序固定：先按 `(方法, 路径)` 查表，声明了 `Content-Length` 且超过该路由上限的，在读体之前直接 413；声明了分块传输又没有长度的返回 411；两者都没有的视为无正文，因此 GET、HEAD、DELETE 不受影响。附件上传的文件本体上限是 64 MiB，其请求体上限为 64 MiB 加 8 KiB 的 multipart 固定开销，文件字节数只在读体过程中判定。

配置、密钥、模型发现、MCP、技能、人设与准则这些原先由桌面端直接读写本机文件的族，现在都有对应的 `/api/v1/*` 路由，桌面端因此不再直连本机配置文件、`.env`、`.mcp.json`、`SOUL.md`、`AGENTS.md` 与技能目录。后端自报支持哪些族由 `GET /api/v1/meta` 的 12 个能力位给出，运行态摘要由 `GET /api/v1/system/status` 给出（租约、排队、远端适配器、构建版本、服务器时间）。

两条入口的接入路径都不承载单个慢客户端的等待，这条并发契约必须保持：HTTP 面由 axum（hyper + tokio）承载，框架负责连接循环与请求解析，网关自身不再持有固定数量的工作线程；请求头解析、正文读取与 durable 入站中的同步部分（运行态存储读写、provider 解析脚本、事件落库）一律在阻塞池执行，因此慢请求只占阻塞池的一个名额，不会停掉整条接入面。阻塞池并发上限 64，排队预算 5 秒，超时返回 429 `gateway_busy`；内核连接许可（默认 256）仍然是更外层的准入。WebSocket 入口的全部连接共享单线程 runtime，durable 桥调用（事件提交、订阅重放）同样只在阻塞池执行，连接收口先关闭事件接收端再回收投影线程。

WebSocket 的 `event` 帧包含 `subscribeId`、`event_type`、`payload_json`、`cursor` 与布尔字段 `replay`。初始订阅和断流补偿中读取的 durable 历史事件使用 `replay: true`，实时流转发使用 `replay: false`。客户端应照常显示回放内容，但仅对实时终态发送新结果提示，避免切换会话或重连后反复推送历史通知。订阅一个**已经终态**的 Interaction 时，这次回放还会在全部 durable 事件之后补一条 `interaction.status` 终态帧（`payload` 为状态字符串，`replay: true`，无 `cursor`）：终态平时只由实时通道在状态跃迁时发出，早已结束的回合不会再跃迁，缺了这条帧客户端就没有收口依据；`received` 与 `processing` 不补，由实时跃迁给出。

provider 内容资产位于 `~/.axon/events/{provider}/`，目录名即 provider 名，其中 `parser.py` 必填、`sop.md` 可选。仓库内只保留输出契约 `crates/axon-daemon/src/infrastructure/event_script_schema.json` 与测试夹具，不存放任何 provider 业务脚本；新增外部系统只需新增目录，Rust 代码零改动。

解析脚本从 stdin 读取 UTF-8 JSON `{provider, received_at_ms, payload}`，向 stdout 写出 `{event_id, correlation_key?, title, content}`。`event_id` 必须带 `{provider}:` 命名空间，且不得掺入接收时间或随机数，否则 durable 幂等键失效。脚本以非交互方式启动：环境变量清空后只注入白名单项与 `PYTHONUTF8=1`，拿不到任何凭据；超过 2 秒按超时终止并整树回收进程。脚本缺失、崩溃、超时或输出不合契约时事件一律 fail-closed 丢弃并留下审计行，分别返回 404 / 504 / 502。

归一化后的事件在进入 durable ingress 前有两条增强：`content` 超过 4000 个 Unicode 字符时原子归档为 `~/.axon/data/archive/*.md`，Interaction 正文只保留包含归档路径与 `sop.md` 路径的简报，由主 turn 用 `builtin.read_file` 按 offset/limit 分页读取；带关联键的事件（provider 路由未显式给出时回落为 `event_id`）按该键做 3000ms 滑动窗口合并，窗口内的同类事件只唤醒主 turn 一次。事件有效期 24 小时，过期事件按既有策略拒绝。事件 durable 入账成功后立即唤醒主 turn，与 CLI/TUI 的 IPC 提交走同一条唤醒路径，不再依赖 1 秒维护定时兜底；被防抖吸收或幂等命中的提交不产生新记录，因此也不重复唤醒。

外部事件没有调用方 cwd，因此固定绑定 Daemon `--root` 工作目录派生的 Workspace：该 Workspace 由守护进程启动时注册，与 CLI、cron 提交侧的契约一致（提交方负责注册，主 turn 只按 id 查找），未注册时主 turn 会在打开运行时阶段拒绝事件。事件驱动的工作都在这个工作区内进行，仓库操作由 `sop.md` 用绝对路径表达，引擎不为外部事件引入第二套执行身份。

---

## 4. Turn 投影时序


Turn 执行过程中，Daemon 按 Provider 生产顺序消费 wire 事件。正文与推理增量、用量、供应商状态只走内存实时通道：订阅者收到即渲染，不写库、不带 cursor、不推进重放水位，慢消费者丢帧也不阻塞本轮。落库只发生在语义边界（工具调用前后、流结束、轮次收口）：边界上先把累积的正文压成一条快照事件（同一 part 的整段文本），再提交该语义事件，因此每个边界一次事务。回放读的是边界快照：整段正文仍可重建，但不再逐帧回放打字过程。Interaction 的实时帧与终态收口都在同一事务内校验当前 processing lease；supervisor 收口为 interrupted 后，旧 worker 的 delta、usage 或 completed 事件不会再进入 durable sequence。

Provider 的每次成功流固定产生 `StreamingStarted -> delta/reasoning/usage/tool -> StreamingCompleted`。整轮收口只追加记忆、技能、治理、运行错误、canonical assistant 等不可由实时增量替代的稳定快照，最后追加唯一 terminal `RunLifecycle`，再写入 `turn_completed`。实时 reasoning、tool、file、plan 与 usage 不在收口阶段重建；投影异常也不会把旧过程事件迟到插回日志。CLI/TUI 与 Gateway 订阅在同一 Workspace lane 回放时均按 `interactionId` 过滤，并直接消费 durable sequence，不建立第二套重排或完成门禁。

整轮的顺序契约是 `canonical assistant -> foreground/run completed -> turn_completed`。canonical 终止事件（最终 assistant 与 terminal `RunLifecycle`）在同一批 `publish_batch` 内原子提交；该批次失败必须终止本轮并形成结构化失败结论，禁止在 durable 缺口上继续写入 `turn_completed`。成功但无正文的回合仍提交 canonical assistant（内容为空），因此该契约对所有成功回合都成立。

文件差异在工具执行过程中按稳定 part ID 更新：流式参数只能形成预览，工具回执产生的实际结果会在同一实时序列中原地替换预览。终态收口不重新创建旧 `ToolCall`、`ToolResult` 或 `FileChange`。

`axon run` 通过同一 Daemon durable/realtime 订阅链路接收当前 Interaction 的过程 wire 事件并等待 `turn_completed`；Text 模式会把 assistant 增量实时写到 stdout，canonical 终态用于完整性校验与缺失正文兜底，JSON 模式保持终态 JSONL 契约。

---

## 5. 启动快照复用


Daemon 是长驻进程，`CliProviderTurnRunner` 持有进程级 `StartupSnapshotCache`：启动后立即在后台线程按运行态根目录预热首份启动快照，主 turn 直接复用，不再每轮现场重建 Principal、Personal Memory、User Agents、Workspace 指令、Skill catalog 与 MCP registry。

缓存以「规范化 config 路径 + 规范化 cwd」为键，最多保留 4 条，每条记录带 TTL 与资源指纹。TTL 内直接命中且不做任何文件扫描；TTL 到期后先计算轻量指纹（用户级与工作区 AGENTS.md、`.mcp.json`、`.axon/skills`、用户级 memory 存储目录的 mtime / 大小 / 条目数）：指纹不一致，**或上一次快照存在探测失败的 MCP server**，都会重建；两者都不成立才只续期。缓存缺失或构建失败时返回 `None`，本轮自动回退到现场构建，语义与未接入缓存前完全一致。

指纹只做元数据扫描，不读取文件内容，且目录深度与条目数都有上限，避免异常目录拖慢 turn。

显式刷新走 IPC 命令 `RefreshMcpCapabilities { cwd }`：Daemon 清除该运行态根目录的 MCP 健康负缓存，作废对应 Workspace 的快照条目，再在后台以**绕过健康负缓存与 schema 缓存**的方式重建快照。TUI `/mcp` 与 OAuth 授权成功后的自动刷新都会提交该命令，因此恢复的 MCP 工具在下一轮 turn 就进入 Provider 工具表，而不是只刷新前端目录。该命令由 IPC 层直接处理，不进入 `DaemonHost` 的命令分发；探测在后台线程执行，IPC 不等待，下一次 `acquire` 会等这次构建结束后复用其结果。

---

## 6. 容量基线与存储策略


容量、故障与恢复验收（均使用真实 SQLite 事实源与真实 DaemonHost，只把 Provider 执行替换为本地执行器）：

```powershell
cargo test -p axon-daemon --release daemon_host_capacity -- --ignored --nocapture
cargo test -p axon-daemon --release daemon_host_soak -- --ignored --nocapture
bash tools/verify-daemon-restart.sh           # 真机重启接管，需 REAL_DEVICE=1
```

Windows 实测基线（`--release`，1k/10k backlog 与 8 线程并发入口）：

| 场景 | P50 | P95 | 总耗时 | 常驻内存 | WAL |
|---|---|---|---|---|---|
| 1k backlog | 1.78ms | 2.65ms | 1.94s | 8.96MB → 9.84MB | 4.14MB |
| 10k backlog | 2.22ms | 3.51ms | 23.34s | 11.61MB → 11.87MB | 4.17MB |
| 并发入口 8 线程/1000 动作 | 1.19ms | 1.62ms | 2.29s（另有 2.04s 结算） | 8.87MB → 13.63MB | 4.15MB |

事实源存储策略固定为：进程内连接池 + WAL + `synchronous=FULL`。`synchronous=FULL` 是因为已提交的外部副作用事实必须承受断电级故障，崩溃恢复只依赖已提交记录，不做重放猜测。写事务统一使用 `TransactionBehavior::Immediate`，使写锁争用落到 `busy_timeout` 上而不是立即返回 `SQLITE_BUSY`。写锁等待上限生产默认为 5 秒；`SqliteRuntimeStore::with_busy_timeout` 允许注入更小的上限，使「超过等待上限后有界失败且不留下幽灵 claim」的确定性验证不必空转完整生产超时。WAL 体积有硬上限：连接统一设置 `journal_size_limit = 64 MiB`，并显式钉住 `wal_autocheckpoint = 1000` 页。没有上限时 WAL 会随每次写入增长到与主库同量级（实测 708 MiB / 703 MiB），一次 checkpoint 需要回写整库并独占写锁数十秒，所有写者按 `busy_timeout` 排队失败。

### 事件正文外置与归档

`session_events.payload_json` 只承载定长元数据：流式帧（`wire_event`）一律外置，其余事件正文超过 4 KiB 才外置。外置正文按会话追加写入 `data/archive/session-events/<session_id>/<分片>.jsonl`（每 2000 个序号一片），表里保留 `payload_ref`，形如 `<session_id>/<分片>.jsonl@<偏移>+<长度>`，回放时按偏移取回。单条 desktop 车道一天数十万帧，分片让文件数与「会话数 × 保留窗口内分片数」成正比，不会退化成百万小文件。

`session_events` 增量追加 `interaction_id` 列并建 `(session_id, interaction_id, sequence)` 联合索引：带 Interaction 租约的写入把归属落列，历史帧据此只读目标交互自己的帧，不再扫整条会话。老记录该列为空串，读取侧自动回退整会话扫描。外置正文读取失败（缺失、非法名、权限、编码）只把该帧降级为 `archive_missing` 占位并带 `archiveError`，单帧损坏不拖垮整页；清理任务按活跃会话渐进修复悬空 `payload_ref`，改写成内联占位并清空引用，且只在正文文件确实不存在时才改写：该更新不可逆，可恢复或可诊断的临时读取故障只由读取侧降级，不在这里判成永久丢失。

清理走「先归档再删行」：过期事件按 lane 追加写入 `data/archive/session-events/<session_id>.jsonl.zst`（zstd 多帧串联，可分批中断），归档成功后才删除对应行。分片被同一会话的多条事件共用，单条行过期不删整片，由会话族清理统一回收；只有确认删除的行才回收其独占外置正文，整批回收会在并发删除时误删仍被其他行引用的文件。删除与回执由单条 `DELETE ... RETURNING` 一次完成，先查后删会在两条语句之间留出并发窗口，让调用方分不清谁真的被删了。该目录整体排除在 3 天文件保留期之外。归档目录与事件正文外置目录同一处，备份与清理只需看一个路径。

磁盘水位淘汰在时间维度清理之后执行，常量先写死：硬上限 4 GiB、高水位 3.2 GiB、低水位 2 GiB；只淘汰一小时以前且不属于运行中 lane 的事件，同样先归档再删除，随后由 `incremental_vacuum` 归还空闲页。上限按实测高负载口径留出余量（desktop 车道单日约 400 MiB，7 天约 2.8 GiB），避免水位淘汰先于 7 天保留期触发，把保留窗口悄悄缩短。

事件保留窗口按会话状态区分：仍在推进的会话保留最近 200 条事件作为续跑锚点，其余事件按 7 天窗口回收；已终结会话不再有 sequence 回退风险，事件按时间窗全量回收。会话根记录的回收同时覆盖 rollout 索引与运行态两侧，避免不在 rollout 索引里的子代理与历史 desktop 车道永久滞留。

---

## 7. 进程资源遥测


### 进程资源遥测

守护进程每 30 秒采样一次自身常驻内存与 CPU，样本按日历日分片追加写入 `data/resource-metrics/samples-<YYYY-MM-DD>.jsonl`，单条记录含 `sampledAtMs`、`pid`、`rssBytes`、`cpuPercent`、`totalMemoryBytes`、`highWaterBytes`、`rssRatioPercent`。样本与日志同族，按 3 天文件保留期回收。

采样只落盘、不逐条打日志；连续 3 次越过水位才输出一条 WARN 遥测，避免抖动刷屏。内存高水位取物理内存的八分之一并夹在 768 MiB 与 4 GiB 之间，与桌面端 UI 失败看门狗同一口径；CPU 高水位为单核 80%。WARN 行以 `[resource-telemetry]` 开头并自带完整 JSON 样本，可直接按时间轴还原增长曲线：

```powershell
Get-ChildItem "$env:USERPROFILE\.axon\data\resource-metrics" -Filter 'samples-*.jsonl' |
  Get-Content |
  ForEach-Object { $_ | ConvertFrom-Json } |
  Sort-Object sampledAtMs |
  Select-Object -Last 20 sampledAtMs, rssBytes, cpuPercent
```

---

## 8. 租约续期与状态收敛


执行期的租约由工作线程自行续约：领取 Action 时写入租约到期时间，随后在整轮执行期间按租约时长的一半周期续租一次（每次一条按主键的 UPDATE，命中 `jarvis_executions` 主键），轮次收口即停表，空闲期不占线程也不产生写入。因此「一轮跑得比租约久」不会再被判成超期：只有工作线程真的消失（进程崩溃、Host 换代或取消请求）时，supervisor 才会把仍在 `running` 的执行隔离为 `uncertain`。续租条件不成立（租约已被他人接管或已过期）时工作线程立即停止续租，不重试。

后台 Execution 的逐字稿写在它绑定的运行时会话里（`goal-session-<goalId>` 这类标识）；被隔离为 `uncertain` 的执行只有一条终态记录，回看该会话时是空态而不是伪造的正文。

---

## 相关文档

- [../daemon.md](../daemon.md) - 用户可见的服务管理与行为
- [存储架构方案](./storage-architecture.md)
- [存储层现状测绘](./storage-map.md)
