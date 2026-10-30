# 飞书远程控制

飞书远控是 Axon 的常驻能力，让用户在飞书会话中直接触发 Agent turn、查看执行进度并投递最终结果。本章说明能力边界、配置项与运维方式。

## 能力边界

- **飞书 WebSocket 长连接**：使用事件订阅模式接入飞书 OpenAPI。
- **消息去重**：自动过滤重复消息。
- **@ 门禁**：群聊中需要 @ 机器人；私聊与白名单发送者可绕过。
- **统一智能体场景**：飞书只提供来源、受众、权限和回复路由元数据，不创建或切换用户可见会话。`chat_id`、`chat_type`、`alt_id` 与回复消息的 `thread_id` 均不成为人格或任务身份；所有消息先进入 Daemon 的 Interaction inbox，再由统一主 turn 按当前 Workspace 场景处理。
- **运行中追加输入**：Agent 执行期间收到的普通文本进入当前 turn 的 FIFO，只在 Provider 完成或工具结果返回后的安全边界交给 Agent；同一 Workspace 的后续 turn 串行等待，不会替换或取消当前 turn。ingress frontier 同时作用于 WebSocket 回调补投和已写入 Daemon 事件源但尚未执行的消息；gateway 读源与处理解耦——调度线程只按事件源顺序读取并投递，固定数量的 worker 按 **chat 分车道**执行「enrich → 治理 → 推进 frontier → Interaction dispatch」：同一 chat 严格串行、不同 chat 并行，因此单个 chat 的慢引用查询或大附件不会阻塞其他 chat。durable cursor 只在整批事件完成后按批次顺序推进，未确认区间在重启后按事件源顺序重放（治理记录与 Interaction ID 保证幂等）；缺少有效 `sourceCreatedAtMs` 时直接确认并丢弃。入站主链上的远端调用都有硬上界（reaction 归属与引用正文查询 3s、单条消息媒体下载合计 20s、Daemon IPC 往返 10s）：单个 chat 的慢网络或大附件只降级自身增强信息，不会无限拖住其他 chat 的入站治理。
- **引用消息上下文**：引用目标 ID 先随 WebSocket 事件持久化，gateway worker 再通过 inline 正文、共享双限缓存（512 条 / 8MiB，全进程一份）或官方 message get 解析父正文；真实查询受「每 1 秒最多 8 次」的时间窗预算约束，且 HTTP 在锁外执行，车道之间不会为一次远端查询排队。normal turn 与运行中追加输入都携带 typed `quotedMessage` sidecar，并在当前 Provider 边界按 token 预算选择完整正文、首尾摘录或仅 ID 指针；查询失败只降级引用上下文，不丢失当前消息。
- **媒体支持**：
  - 文本类附件（单个不超过 64KB）会以内联正文注入 Agent prompt，Agent 可直接阅读其内容。
  - 二进制媒体（图片 / 文件 / 音频 / 视频）由 gateway worker 在**入站 durable append 之后**下载到本机媒体缓存，并把本地路径注入 Agent 可见文本；同一批路径同时以结构化 `files` 进入本轮 Provider turn，因此图片作为 vision 输入、文件作为可读附件。下载必须晚于回调 ACK：在飞书回调线程内下载会拖长 ACK 并触发官方重投。
  - 附件是消息的增强而非本体：单条附件下载失败只降级该附件并在 prompt 中显式标注不可用，绝不拒绝整条用户消息；附件缓存过期回收时同样只丢附件。下载另有硬上界——单个附件 10s、单条消息合计 20s，超限按同一降级路径处理，避免把入站主链拖成长尾。
  - 图片单个媒体上限为 10MB；文件、音频、视频等媒体单个上限为 30MB，单条消息可包含多个媒体；本地媒体缓存总量 64MB / 64 个文件 / 24 小时保留。
  - 语音转文本能力：音频文件会被下载并注入路径；语音转写（ASR）需依赖具备语音识别能力的外部服务或多模态模型，运行时原生不提供内置离线 ASR 引擎。
  - 本地缓存治理只用于保护本机资源，不改变用户可用的 Feishu 能力。
  - Agent 可以在用户确认文件后，在回复中使用 `MEDIA:/absolute/path/to/file` 将文件作为附件发送：该协议行不会作为普通正文展示，正文（剥离协议行后的可见正文）与附件**都会投递**，附件路径不存在时退化为纯正文投递；文件未确认或发送失败时不会虚报成功。
- **回合级进度投影**：入站消息在进入持久 Action 前立即添加 `Typing` reaction；gateway worker 只做本地 reaction 状态确认，把未启动的请求投入有界异步队列，从不在入站主链上同步等待飞书 reaction API。回合成功或失败都会清理 `Typing`，控制命令（如 `/status`）也由 Daemon 完成事件统一收口。回合执行期间只维护**一张** interactive 进度卡：卡片由 header 与一个 markdown 元素组成，过程行在上、结论在下、终态页脚收尾；首个可渲染事实（正文增量、工具行或思考行）创建卡片，后续事件用同一 `message_id` 原位 PATCH，正文与过程行共用 1.5 秒合并窗口，是否真的发 PATCH 由渲染指纹决定。过程行**不提供展开**，一行即全部信息：`{标记} **{类别}** · {目标} · {读数}`——目标取模型自述 `intent` 或该工具专用主参数（`memory_search` 的 `query`、`invoke_skill` 的 `skill_name`、子代理的 `task`、`send_message` 的 `message` 等，规则表与桌面端、TUI 共用 `axon-display`，上限 80 字符），读数取非零退出码、失败、已写入或耗时（上限 24 字符）；工具结果只补标记与读数，绝不覆写调用时已定的目标行，回执原文与 reasoning 正文都不投影，思考只留一行耗时。供应商的连接与生成中状态不进入卡片（运行中由 `Typing` 承担），只有重试、失败与故障转移各留一条提示行；供应商路由与上下文占用只出现在终态页脚。投影严格按共享 durable sequence 消费，不在飞书侧另建重排或完成门禁；`MEDIA:` 协议行整行隐藏，且尾部可能演化成协议行的半截内容会被扣留，不会泄漏给用户。
  - 回合结束时由 `finish` 同步收口（有硬上界），只有飞书 API 确认成功的正文才计入已投影账本；账本完整且回复不含附件指令时不再重复投递最终回复，其余情况一律由 durable outbox 补偿未投影内容。网络投影是纯增强能力：任意失败都只降级为「未投影」，绝不丢失正文。
  - 交互卡片容量与预算：正文受 8,000 字符上限约束，卡片序列化后受 28KB 预算约束。超出预算时先收过程行窗口（只保留最近 5 行，更早的压成一行「N 个动作已省略」），再收缩正文；正文一旦被截断即视为未完整投影，由 durable outbox 无损补投，长正文不会在卡片里静默丢尾。
- **Cron 投递**：用户级 Cron 用 `--wake feishu:<chat_id>` 声明结果投递目标；不带 `--wake` 的任务执行完只落运行记录，不产生任何投递。
- **后台完成通知**：Goal/Cron 汇总只投递给创建时声明的目标，不再广播。`notification_chat_id` 是用户级全局配置，通过 `GET /api/v1/crons/config` 的 `defaultNotificationChatId` 暴露；用户没有特意指明目标时，Agent 以它作为 `--wake feishu:<chat_id>` 的默认值。后端不做隐式兜底。

## 配置

飞书远控配置保存在 `[remote]` 与 `[remote.feishu_websocket]` 下：

```toml
[remote]
mode = "standard"

[remote.gateway]
api_token_env = "AXON_GATEWAY_API_TOKEN"
http_bind = "127.0.0.1:3143"
websocket_bind = "127.0.0.1:3144"

# 默认仅监听本机；局域网或外部网络访问时，可按需改为 0.0.0.0:3143/3144
# 或 [::]:3143/3144，并由防火墙或反向代理限制暴露范围。

[remote.feishu_websocket]
enabled = true
app_id = "cli_xxx"
app_secret_env = "FEISHU_APP_SECRET"
enable_notification_sending = true
# 后台完成通知的固定接收会话；留空不发送
notification_chat_id = "oc_notify"
allowed_sender_open_ids = ["ou_allowed"]
blocked_sender_open_ids = ["ou_blocked"]
require_mention = true

[[remote.feishu_websocket.chat_policies]]
chat_id = "oc_ops"
allowed_sender_open_ids = ["ou_ops"]
blocked_sender_open_ids = []
require_mention = true
```

### `mode`

`remote.mode` 是远控吞吐治理的用户入口；cursor 与 payload 上限由运行时按模式推导：

| 模式 | 适用场景 | 运行态策略 |
|------|----------|------------|
| `standard` | 常规飞书远控、低频协作 | 默认 cursor 与 payload 内部预算 |
| `high-throughput` | 高频事件流、密集群聊协作 | 更高的 cursor 与 payload 预算 |

### 群聊策略

`chat_policies` 可针对单个 `chat_id` 覆盖默认 sender 白名单与 mention 行为；不同会话可设置独立门禁。

### Sender 治理

- `allowed_sender_open_ids`：全局白名单发送者。
- `blocked_sender_open_ids`：全局黑名单发送者，命中后直接拒绝。
- `require_mention`：群聊是否需要 @ 机器人；私聊通常可设为 `false`。

### 飞书应用能力与权限

- 启用机器人能力和 WebSocket 事件订阅，并订阅接收消息事件。
- 为应用身份授予发送消息、处理 reaction、"获取指定消息内容"以及"获取群信息/会话信息"所需的飞书权限；引用解析使用 `GET /open-apis/im/v1/messages/:message_id`，reaction 补齐会话类型使用 `GET /open-apis/im/v1/chats/:chat_id`（缺少会话信息权限时 reaction 输入会被跳过并留下告警，不会误判受众）。
- 机器人必须位于目标消息所属会话中。飞书拒绝访问或返回空正文时，Axon 会继续处理当前消息，但模型只能看到引用 message ID。
- 权限名称以飞书开发者后台当前展示为准；调整权限后需要发布应用版本并在租户内重新授权。

## 环境变量

| 变量 | 用途 |
|------|------|
| `FEISHU_APP_ID` | 飞书 App ID |
| `FEISHU_APP_SECRET` | 飞书 App Secret |

`.env` 文件与 `config.toml` 同目录保存，配置 / CLI JSON 输出 / 资源投影都不会输出 App Secret 原文。

## 部署与运维

### 启用 Axon 用户服务

飞书远控依赖 Axon Daemon 常驻宿主。安装 Axon 时会一并注册 Daemon，用户通过统一的 `axon daemon` 命令管理服务：

- macOS：由 launchd 安装用户服务，启用 `RunAtLoad + KeepAlive`。
- Linux：由 systemd user service 安装，启用 `Restart=always`。
- Windows：由当前用户最高权限 Task Scheduler 在登录时隐藏启动 `AxonDaemon`，任务通过 `RestartOnFailure` 自动恢复，Daemon 内部负责连接保持和失败重试。注册或更新需要同一用户的 UAC 授权。

```bash
# Linux / macOS 一次性脚本安装并启动 Axon 用户服务
curl -fsSL https://raw.githubusercontent.com/a574676848/axon-code/master/scripts/install.sh | bash
axon daemon start
```

```powershell
# Windows
iex (irm https://raw.githubusercontent.com/a574676848/axon-code/master/scripts/install.ps1)
axon daemon start
```

### 手动控制

```bash
axon daemon start
axon daemon stop
axon daemon restart
axon daemon status
```

`axon daemon status` 返回 Daemon PID、运行状态、重连次数与有限错误摘要；Feishu 连接状态单独显示，未配置 Feishu 时不影响 Daemon 会话与 Cron。

### Memory consolidation

Daemon 常驻 runtime 同时运行用户级 Cron tick；Memory consolidation 作为系统 Cron `axon.system.memory-consolidation` 自动创建或更新，默认每小时执行一次，使用 Cron 主链直接执行显式 TTL 清理、重复/冲突治理与 30 天半衰期的低活跃衰减归档，不经过 Agent，也不依赖 Feishu。

### 执行进度与最终回答

- OpenAI Responses、Chat Completions 与 Anthropic Messages 共用 Agent Runtime 的强类型工具历史和回合完成性判断。飞书不发起独立 Provider 续轮，也不修改 call/result、重试、compaction、prompt cache 或 WebSocket response chain。
- 飞书入站消息经治理链归一化为 Workspace-scoped Interaction，其正文、附件引用和可选 `quotedMessage` 只作用于当前 Provider turn；回复目标由渠道无关的 `ReplyRoute` 表达。`quotedMessage` 只传入当前 Provider turn，不随 completed event 回写；稳定 Runtime `session_id`、rollout ID、工作目录和 context epoch 均由 Session Runtime 在入队与执行边界注入，飞书配置或消息字段不能覆盖。飞书反馈清理与投递使用稳定 `session_id`，Provider 恢复只使用 rollout ID。模型选择继续由共享 Provider 执行链在每轮加载用户配置后完成。
- 同一 Workspace 的 Execution 由 Daemon Worker registry 串行执行。fencing、执行错误或 panic 边界时，registry 会在同一 ownership 临界区接管 pending 唤醒或释放 Worker lease；飞书、TUI 与 Cron 不维护独立 Worker 状态，也不依赖前台进程存活。
- 中间 Provider 回合的可见 assistant `delta` 会立即进入流式投影：首个可渲染事实创建 interactive 卡片，后续事件用同一个 `message_id` 原位 PATCH；worker 按 1.5 秒合并刷新，正文与过程行共用同一个合并门，不再为工具边界单独绕开窗口。思考开始与结束只投影一行耗时，原始 reasoning 文本永不投影。供应商状态、assistant 回合与工具调用严格按共享 durable sequence 消费，不在飞书侧另建重排或完成门禁；canonical assistant 与唯一 terminal 到达后只做卡片收口，不迟到重放旧工具事件。
- 一个回合只有一张进度卡：`tool_call_id` 独占**一行**而不是一条消息，同一调用的开始与完成原位更新该行，因此同一次回复不会再刷出一串各带引用头的工具消息。工具行只承载折叠态事实（类别、目标、读数），回执原文与输出片段不进入卡片；供应商的连接与生成中状态只由 `Typing` 承担，重试、失败与故障转移各留一条可覆盖的提示行。
- 模型原始回答中的 `<oai-mem-citation>` 是内部 Memory 使用信号：canonical response 保留它用于 citation 解析和 usage 统计，飞书流式卡片与最终回复统一隐藏完整块、未闭合块和分片中的半截标签。
- 单个 assistant 流式卡片最多承载 8,000 个字符正文，并按 interactive card 序列化后不超过 28KB 的安全预算；该预算低于飞书 30KB 卡片上限，并将 JSON 转义和卡片元数据预留计入计算，避免引号、反斜杠、中文或控制字符导致更新失败。预算不足时先收过程行窗口再收缩正文，正文被截断时该回合判定为未完整投影。
- 工具开始与完成事件在同一合并窗口内刷新卡片；工具结果只补行的状态与读数，不追加回执原文。
- 入站 `Typing` 是低延迟反馈，不代表正文已发送；普通 Provider runner 和控制 Action 在完成边界只持久化清理 task，由 Daemon feedback worker 异步收口，不占用 Session Worker 的 Provider 执行线程。网络或进程异常时，持久 processing 状态与 durable outbox 负责后续补偿，不依赖前台终端保持连接。
- 回合消息与工具进度由独立 worker 异步发送，Provider 与 coding 主流程只进行内存投影和有界队列写入，不执行 Feishu 网络请求。assistant 回合先进入候选状态，worker 在 Feishu API 成功后提交稳定投影 key；队列满、连接断开、网络失败或 worker 异常不会提交，并使最终 durable outbox 回退累计正文。每个入站 `request_id` 拥有独立的进度 dispatch 作用域，后续任务不会复用 UUID 或更新历史任务消息；API 失败会写入 Axon 日志。
- 回合结束的 `Finish` 是有序收口屏障，且在投影邮箱中优先出队：收口不必等待其他 chat 排队中的增量与工具卡，回合自身的 assistant 增量仍按到达顺序应用。worker 结算 Feishu API 结果后才回报投影结论，调用方据此计算 `FinalAssistantProjection`；收口等待的上界由单次受限网络超时派生（在途调用 + 最终 PATCH），因此超时只可能发生在网络受限阶段，不会把「卡片即将写成功」误判为「未投影」。调用方超时即让出结算权，worker 随即放弃该回合的卡片收口，正文完全由 durable outbox 投递；最终正文与已投影账本一致时不再发起冗余 PATCH。全部 PATCH 成功时终态为 `FullyProjected`，不会额外发送重复的普通文本尾段；若最终回复包含 `MEDIA:` 附件指令，仍会在终态继续投递附件；PATCH 失败时仍由 durable outbox 仅补偿真实未投影内容。
- 工具执行边界的恢复状态先提交 rollout SQLite；一次飞书用户请求的启动、Provider 内部工具循环与最终完成使用同一 invocation identity 幂等更新一个 rollout turn，不会因内部 checkpoint 重复用户消息。这条热路径不再同步重放 Thread 事件。Thread mirror 只使用 `<hex>.000000.jsonl` 起始的 8 MiB 分片，不读写旧单文件 JSONL；Provider 启动 mirror 只反向读取有界的激活快照以判断 sequence 和 active turn。rollout SQLite 只保留一份权威 turn 记录，事件账本保存计数与 output_ref 摘要；Thread JSONL 用于独立 thread 的可读投影，其 metadata 同样只保存回执摘要与归档引用。
- Provider 请求准备链使用 turn-scoped trace telemetry 记录 Config/Startup、RunRequest、Memory embedding、Thread 起始 mirror 与持久化等阶段；embedding 同步 fallback 会在底层写入非敏感 warning，不记录 prompt、凭据或 provider raw body。
- `response.incomplete`, `response.failed`, 缺失终止事件或传输中断不会被当成已完成回合；共享 Runtime 只在有界恢复成功后产生新的完整投影，因此飞书展示不会反向驱动工具重做。
- 失败投影只消费结构化 `RuntimeFailure`。Provider 工具调用不完整时稳定错误码为 `provider_tool_call_incomplete`，正文同时展示协议、缺失字段与调用索引；不从 stdout/stderr 猜测类型，也不记录原始工具参数。失败正文写入 reply artifact，durable outbox v3 保存完整 `RuntimeFailure`，Daemon JSON 报告使用 `agentFailure`，从而与 CLI、TUI 和飞书远控保持同一事实。
- 若失败前已经产生可见 assistant 输出，跨渠道切换按副作用边界判定：本轮没有写过文件（工具回执里没有任何落盘写入）且失败属于可转移的瞬时传输错误时，Runtime 可切换到渠道链中的下一个渠道；已写过文件，或失败是参数与鉴权一类永久错误时保持 `visibleOutputEmitted=true` 并禁止静默 replay，Provider 返回 HTTP 400 且明确包含 `REQUEST_BODY_INVALID` 与 `Invalid tool use format.` 的确定性格式错误仍是这条禁令的例外。切换不会在原渠道无限重试，也不会重复投影已经发送的内容；飞书只补偿真实未投影尾部。
- 终态显式区分 `Missing`、`FullyProjected` 与 `Unprojected`：缺少可靠投影时使用累计正文兜底；正文已完整投影时成功结束且不创建重复 outbox；仍有未投影内容时，仅对连续前导句执行有界保守去重，再把尾部写入 artifact 与 durable outbox。语义比较只忽略 Markdown、空白和绝对路径等展示噪声，并使用顺序覆盖率识别轻微语序变化；否定或失败语义不一致时禁止裁剪。
- 最终回答超过单条消息容量时按 8,000 字符无损分段，首段回复用户原消息，后续段发送到同一会话；任一分段失败都会按真实失败状态进入既有重试闭环。

## 安全与审计边界

- 远控侧只具备受控观察与投递能力；stdin 写入、原始交互流读取、补全接受、backend 切换和提权操作均由内置安全边界阻断。
- 工具调用无需远端审批；飞书不提供 `/approve` 或 `/deny` 控制命令。明确标记为 `Forbidden` 的调用仍由本地运行时拒绝。
- 所有远控消息进入会话事件账本与审计日志，可通过 `axon debug session replay <ID>` 与 `axon debug session export <ID>` 复核。

## 故障排查

- **消息使用了错误的 Provider 或模型**：检查当前 Workspace 的运行配置和 Daemon 日志中的内部运行上下文。Interaction payload 不应包含 `runSessionId`、`cwd` 或模型字段；这些值由 Daemon 场景解析与当前用户配置决定。
- **普通消息长期排队且 `attempt=0`**：这表示 Execution 已持久化但尚未被 Daemon Worker 领取，应核对 Daemon 是否存活、Host lease 是否持续续租，以及该稳定 `session_id` 的 Worker registry 是否释放了旧 ownership。还应确认上一个已提交 Execution 的事件发布只写持久 Feishu outbox 并触发维护信号，没有等待进程 stdout 或其他同步观察通道。Provider、Typing 或飞书投递错误发生在 claim 之后或独立投影链路，不能解释 `attempt=0`。
- **同一 chat 的消息上下文不连续**：同一 Workspace 的相邻消息应解析到同一条 Workspace 车道 Session，并复用该车道既有的 rollout；只有首次进入该 Workspace 才创建。若上下文仍不连续，检查该 Workspace 的 registry 解析是否稳定（同一规范化根目录必须解析出同一 `workspace_id`）、是否有外部调用者在轮换该车道的运行窗口，以及 Daemon inbox 顺序。消息不应依赖用户选择或重置 Session 来恢复上下文。
- **消息停在 Thinking 或委派说明**：检查父 Agent 是否在最终回复前等待了关键子 Agent 终态。completion 只投递父 mailbox，不会自动启动后续父 turn；failed/interrupted 必须由仍在运行的父 turn 接管并形成最终回复。
- **`DaemonState` 长时间未到 `connected`**：检查 `FEISHU_APP_ID` / `FEISHU_APP_SECRET` 是否正确，飞书开发者后台是否启用了事件订阅能力并填入正确的回调地址。
- **群聊消息不响应**：确认 `require_mention` 与该会话的 `chat_policies.require_mention` 均为 `true`，并且消息中 @ 了机器人。
- **Sender 被静默拒绝**：检查全局 / 群聊的 `blocked_sender_open_ids` 与 `allowed_sender_open_ids` 是否同时命中。
- **回复消息能执行但模型看不到父正文**：确认应用已发布“获取指定消息内容”权限，机器人仍在目标会话中，并检查日志里的 reply lookup failure stage。网络、权限或正文解析失败不会阻断当前消息。
- **Cron 未投递到飞书**：确认 `axon cron create` 传入了 `--wake feishu:<chat_id>`（`axon cron list` 的 `wake` 列可见），且 `chat_id` 在 `chat_policies` 中存在。

## 相关文档

- [CLI 参考](./cli.md#远程与后台任务)
- [配置指南](./configuration.md#飞书远程控制)
- [系统概览](./overview.md)
- [核心概念](./concepts.md)
