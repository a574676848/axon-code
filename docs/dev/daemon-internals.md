# Daemon 内部实现

本文记录守护进程的运行机制、并发模型、事件入口细节、投影时序与保留策略，属开发者文档。用户可见的服务管理命令与行为见 [`../daemon.md`](../daemon.md)。

---

## 1. 领取与并发模型

Daemon 是 Axon 的后台长生命周期宿主。当前实现已经把 `PrimaryTurnService` 接入 supervisor tick：Daemon 从 durable Interaction inbox 领取输入，按 Workspace 会话有界并发，并通过 lease token 提交 `Understood` 或 `Rejected` 终态。

主 turn 的并发单元是复合会话（Workspace + trust scope + channel + destination），不是批次：`inbox.session_id` 是领取排他的持久化事实源，某个会话已有未过期 `processing` 时，该会话的后续记录不会被领取，同会话串行无需调用方维护在途状态。协调线程在同一批内只等待「任意一个会话」结束，槽位一归还就补位领取并投递，因此某个会话的长 turn 不会推迟其他空闲会话的新消息；只有并发槽位（默认 4 个会话）全满时才等待会话结束。槽位未满时以 250ms 有界窗口复查新到输入，窗口只是补位周期，不改变 durable 领取顺序。
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

解析脚本从 stdin 读取 UTF-8 JSON `{provider, received_at_ms, payload}`，向 stdout 写出 `{event_id, correlation_key?, title, content}`。`event_id` 必须带 `{provider}:` 命名空间，且不得掺入接收时间或随机数，否则 durable 幂等键失效。脚本以非交互方式启动：环境变量清空后只注入白名单项与 `PYTHONUTF8=1`，拿不到任何凭据；超过 2 秒按超时终止并整树回收进程。进程退出码即判定：`0` 放行；`2` 表示按业务纪律主动丢弃（正常运行态，按 `400 event_filtered` 回执）；其余非零视为脚本故障（`502 event_script_failed`）。脚本缺失、超时与输出不合契约同样 fail-closed，分别返回 404 / 504 / 502。每次丢弃都经 `LogSink` 记一行 `[gateway-events] provider=<名> 事件被丢弃：<原因>`：业务过滤记 INFO，脚本故障记 ERROR，provider 未注册与信封不合契约记 WARNING；INFO 行受 `[agent] log_level` 门控，默认 `warning` 下只保留状态码与故障行。

归一化后的事件在进入 durable ingress 前有两条增强：`content` 超过 4000 个 Unicode 字符时原子归档为 `~/.axon/data/archive/*.md`，Interaction 正文只保留包含归档路径与 `sop.md` 路径的简报，由主 turn 用 `builtin.read_file` 按 offset/limit 分页读取；带关联键的事件（provider 路由未显式给出时回落为 `event_id`）按该键做 3000ms 滑动窗口合并，窗口内的同类事件只唤醒主 turn 一次。事件有效期 24 小时，过期事件按既有策略拒绝。事件 durable 入账成功后立即唤醒主 turn，与 CLI/TUI 的 IPC 提交走同一条唤醒路径，不再依赖 1 秒维护定时兜底；被防抖吸收或幂等命中的提交不产生新记录，因此也不重复唤醒。

外部事件没有调用方 cwd，因此固定绑定 Daemon `--root` 工作目录派生的 Workspace：该 Workspace 由守护进程启动时注册，与 CLI、cron 提交侧的契约一致（提交方负责注册，主 turn 只按 id 查找），未注册时主 turn 会在打开运行时阶段拒绝事件。事件驱动的工作都在这个工作区内进行，仓库操作由 `sop.md` 用绝对路径表达，引擎不为外部事件引入第二套执行身份。

---

## 4. Turn 投影时序


Turn 执行过程中，Daemon 按 Provider 生产顺序消费 wire 事件。正文与推理增量、用量、供应商状态只走内存实时通道：订阅者收到即渲染，不写库、不带 cursor、不推进重放水位，慢消费者丢帧也不阻塞本轮。落库只发生在语义边界（工具调用前后、流结束、轮次收口）：边界上先把累积的正文压成一条快照事件（同一 part 的整段文本），再提交该语义事件，因此每个边界一次事务。回放读的是边界快照：整段正文仍可重建，但不再逐帧回放打字过程。Interaction 的实时帧与终态收口都在同一事务内校验当前 processing lease；supervisor 收口为 interrupted 后，旧 worker 的 delta、usage 或 completed 事件不会再进入 durable sequence。

Provider 的每次成功流固定产生 `StreamingStarted -> delta/reasoning/usage/tool -> StreamingCompleted`。整轮收口只追加记忆、技能、治理、运行错误、canonical assistant 等不可由实时增量替代的稳定快照，最后追加唯一 terminal `RunLifecycle`，再写入 `turn_completed`。实时 reasoning、tool、file、plan 与 usage 不在收口阶段重建；投影异常也不会把旧过程事件迟到插回日志。CLI/TUI 与 Gateway 订阅在同一会话回放时均按 `interactionId` 过滤，并直接消费 durable sequence，不建立第二套重排或完成门禁。

整轮的顺序契约是 `canonical assistant -> foreground/run completed -> turn_completed`。canonical 终止事件（最终 assistant 与 terminal `RunLifecycle`）在同一批 `publish_batch` 内原子提交；该批次失败必须终止本轮并形成结构化失败结论，禁止在 durable 缺口上继续写入 `turn_completed`。成功但无正文的回合仍提交 canonical assistant（内容为空），因此该契约对所有成功回合都成立。

后台作业的生命周期同样落事件：作业注册表在启动与首次终态两处广播 `job_started` / `job_completed`，宿主订阅后追加到作业归属会话的事件流，并把「在跑作业数」投影进 `session_metrics.jobs_running`（会话列表的 `openJobCount` 读这个值）。临时作业随进程消亡，事件是历史事实而不是可续跑句柄。

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

事实源存储策略固定为：进程内连接池 + WAL + `synchronous=NORMAL`。`synchronous=NORMAL` 是 WAL 下的推荐档位：`COMMIT` 不再逐事务 `fsync`，只在 checkpoint 与显式同步点落盘，代价是断电可能丢掉最近一小段已提交事务，而库结构不会损坏；运行态事实（会话状态、队列、事件水位）接受这一语义弱化，崩溃恢复按「以日志为准回退水位、从结算记录重建叙述」设计。写事务统一使用 `TransactionBehavior::Immediate`，使写锁争用落到 `busy_timeout` 上而不是立即返回 `SQLITE_BUSY`。写锁等待上限生产默认为 1 秒，到期返回 `Busy` 交由上层可观测地失败并等下一次 tick 重试，而不是无限阻塞；`SqliteRuntimeStore::with_busy_timeout` 允许注入更小的上限，使「超过等待上限后有界失败且不留下幽灵 claim」的确定性验证不必空转完整生产超时。WAL 体积有硬上限：连接统一设置 `journal_size_limit = 64 MiB`，并显式钉住 `wal_autocheckpoint = 1000` 页。没有上限时 WAL 会随每次写入增长到与主库同量级（实测 708 MiB / 703 MiB），一次 checkpoint 需要回写整库并独占写锁数十秒，所有写者按 `busy_timeout` 排队失败。

### 事件存储布局与回收

事件不再是关系表里的行：每段会话一个目录 `<data>/events/<session>/`，里面是 `session.v<N>.jsonl.zstd` 帧日志、与它同名的 `.idx` 帧索引以及一把 `.lock`。帧本身已压缩，正文一律内联在帧里，没有外置正文、没有 `payload_ref`、没有「按行归档再删行」这一套；读取按序号定位，损坏帧只影响自己那一帧，由读取侧如实报错而不是拼出半条记录。

`.idx` 是纯派生索引：每帧一条定宽记录 `(首条序号, 帧偏移, 帧长度, 交互指纹)`，写句柄追加时同步维护，文件头记录「已覆盖日志长度」。读取据此二分定位起点帧后只解压需要的帧；按交互取页时再用指纹跳过别条交互的帧（指纹只是缩范围的粗筛，命中后仍逐条确认归属，跨交互帧记 0 由读侧保守扫过）。覆盖长度与日志不一致、结构不自洽或文件缺失时一律忽略并按日志重建，写句柄打不开它也不挡事件落盘。会话恢复只要末条事件时只解压末帧。

回收是目录级操作：保留期按会话目录的最后修改时间整目录删除（会话族与 SQLite 事实表按 7 天，日志与临时文件按 3 天），磁盘预算淘汰在时间维度之后执行，上限 4 GiB、高水位 80%、低水位 20%，从最旧的会话开始删且跳过运行中与受保护的会话。旧版本留下的 `data/archive/session-events/<session>` 目录（外置正文时代的产物）只随会话被删除时整目录回收，不再有新写入。

控制面库因此不随轮次增长：`control.sqlite3` 只放会话、入站、工作项与指标投影，事件体积全部落在事件目录里，备份与容量核算各看一个路径。

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

会话恢复分三段，目的是把文件 IO 移出写事务：先只读取候选会话与水位，再不持锁逐会话读事件面末条并算出落账计划，最后用一个短写事务带水位 CAS 落账。水位在两段之间被别人推进过的会话在这一步落空并被跳过，宁可下一轮再恢复，也不用旧水位覆盖别人的进度。把日志读取放在事务里会让写锁按住「会话数 × 单会话文件读」的时间，WAL 因此无法回卷，其他写者全部卡在 `busy_timeout` 上，最终表现为 `database is locked` 风暴。

后台 Execution 的逐字稿写在它绑定的运行时会话里（`goal-session-<goalId>` 这类标识）；被隔离为 `uncertain` 的执行只有一条终态记录，回看该会话时是空态而不是伪造的正文。

---

## 相关文档

- [../daemon.md](../daemon.md) - 用户可见的服务管理与行为
- [存储架构方案](./storage-architecture.md)
- [存储层现状测绘](./storage-map.md)
