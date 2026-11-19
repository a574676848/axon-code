# 单渲染路径 后端改造提示词

> 用途：把下面整段作为实施会话的首条指令贴入。它自包含任务、事实底座、工作项与验收口径。
>
> 目标读者：后端实施者（`crates/axon-cli`、`crates/axon-session`、`crates/axon-session-runtime`、`crates/axon-daemon`、`crates/axon-domain`）。
> 前端切换由另一路负责，本提示词**不要求**改 `crates/axon-desktop/ui/**`。

---

## 一、任务

让三条会话入口（侧栏、目标看板、子代理）在桌面端只保留**一种** `interaction` 目标与**一条**渲染路径：前端不再按 `channel` 判断只读、可中断、可答问，也不再区分「会话订阅」与「交互订阅」。

后端要做的是三件事：

1. **能力契约**：会话的可写性与能力位由服务端下发，前端只读字段、不做推断。
2. **事实规范化**：派发轮的状态、逐轮指标归属、模型事实补齐，让独立会话的索引与主会话同形。
3. **可发现性**：新轮次出现可被订阅发现；中断能力要么真的可用，要么如实为假。

### 已裁决的前提（不要再讨论）

- **不考虑兼容与迁移**：项目尚未产生真实数据。可以直接改契约、升 schema、删旧字段、拒绝旧库；不写迁移脚本、不写兼容分支。
- **侧栏清单保持 `WHERE sessions.kind = 'primary'`**：不把 `goal` / `subagent` 会话放进 `/api/v1/sessions`。
- **不新增「会话轮次索引」路由**：`GET /api/v1/sessions/{id}/interactions` 就是统一索引，所有会话类型都必须能在它下面出条目。
- **一轮只归属一条会话**：来源会话（`kind=goal` / `kind=cron`）只做账本，不承接逐字稿。

## 二、事实底座（2026-11-19，HEAD `b8adacb8a` 已核实）

| 事实 | 位置 |
| --- | --- |
| 执行侧派发已写 inbox：`channel=agent`、来源键 `agent-dispatch:{session}:{invocation}`、目标执行 `client=goal` | `crates/axon-cli/src/run_commands/run_turn_start.rs:46-118` |
| 派发轮不会被调度器领走：领取候选按 `COALESCE(owner.kind,'primary') IN ('primary','cron')` 过滤 | `crates/axon-session/src/infrastructure/sqlite_runtime_store/interaction_repository.rs` |
| 逐字稿落点：目标落 `kind=goal`、cron 落 `kind=cron`，不再派生子会话 | `crates/axon-daemon/src/application/execution_session_sink.rs:1-12` |
| 开场帧：后台执行起笔时写会话流第一条 `userMessage` | `crates/axon-session-runtime/src/application/agent_turn_executor.rs::publish_turn_input` |
| 轮次标识：schema v10，`sessions.execution_id` 对任意 kind 可用，语义为「当前这一轮是谁」 | `crates/axon-session/src/infrastructure/sqlite_runtime_store/schema.rs` |
| 会话行已记录最近模型：`sessions.provider / model / model_name` | `crates/axon-cli/src/sessions_http_adapter.rs` |
| 索引读侧与读数：`sessionUsage` / `sessionElapsedMs` 直接读 `session_metrics` | `crates/axon-cli/src/workbench_http_session_interactions.rs`、`workbench_http_session_readings.rs:352-410` |
| 状态派生：无 work_item 时兜底 `received` | `interaction_query_repository.rs` 的 `INBOX_STATE_JOIN` |
| 事件归属列只在带 lease 写入时赋值：`interaction_column = interaction_lease.map(...)`，无 lease 写入即 NULL | `event_log_repository.rs:89`（`append_events` 与 `append_interaction_events` 共用） |
| 逐轮读数只按事件的 `interaction_id` 匹配，取不到就直接跳过 | `crates/axon-eventlog/src/tail.rs::collect_latest` |
| `TurnMetrics.turn_id` 取的是 `assistant_part_id`，值为 `{前缀}-{invocation_id}`，不是 interaction 标识 | `run_turn_metrics.rs:188-189`、`run_session_store.rs:358` |
| 结算 API 全部要求 `lease_token` | `crates/axon-session/src/ports/interaction_store.rs:97-126` |
| 中断路由只打 `active_primary_turns` | `crates/axon-cli/src/workbench_http_adapter.rs:226-240` |
| 子代理名册字段：`sessionId/slug/title/model/provider/status/turnCount/lastTurnStatus/hasRuntime` | `crates/axon-desktop/src/conversation/session_read.rs` |
| 看板入口会话：经 `latest_subagent_session_ids` 映射 + 回退 | `crates/axon-cli/src/goal_http_adapter.rs:197-218` |

与源码冲突时以源码为准，并当场记录差异。

## 三、工作项

按 P0 → P2 顺序做。每项都写「现状 → 改动 → 验收」，验收不通过不得进入下一项。

### W1 会话能力契约（P0）

**现状**：只读、可中断、可答问只能由前端按 `channel` 推断（`targets.ts` 的 `STANDALONE_CHANNELS`、`sessionOpening.ts` 的 `isReadOnlySession`）。

**改动**：在 `GET /api/v1/sessions/{id}/interactions` 的 payload 增加 `session` 块。视图必然先取这一页，因此不增加请求：

```json
"session": {
  "kind": "primary|goal|cron|subagent",
  "origin": "human|goal|cron|subagent",
  "writable": true,
  "canInterrupt": true,
  "canAnswerQuestions": true,
  "title": "…",
  "provider": "…",
  "model": "…",
  "modelName": "…",
  "interactionCount": 12,
  "latestInteractionId": "…"
}
```

规则：

- `writable` 仅 `kind=primary` 为 `true`；`goal` / `cron` / `subagent` 一律 `false`。
- `canAnswerQuestions` 仅 `primary` 为 `true`（后台链不注入用户提问端口，见 `run_provider_orchestration.rs:164-166`）。
- `canInterrupt` 仅 `primary` 为 `true`：只有主 turn 有进程内取消令牌。目标执行的取消**不经过**中断接口，只能由来源会话调用 Goal 工具发起（见 W6）；`cron` 与 `subagent` 同样不提供中断入口。子代理给 `false` 是产品口径而不是「打不到」：父轮取消会连带它的收口走同一条停止链路。
- `origin` 由 `sessions.kind` 映射得到，禁止用 `source_channel` 猜。

**验收**：`GET /api/v1/sessions/{goalSessionId}/interactions?limit=1` 返回 `session` 块；`goal` 与 `subagent` 会话 `writable=false`；`primary` 会话 `writable=true`。

### W2 派发轮终态（P0）

**现状**：`agent-dispatch` 行没有 `work_items`，状态表达式兜底成 `received`，永远不结算；`mark_understood` / `mark_rejected` / `mark_interrupted` 都要求 `lease_token`，无 lease 无法落终态。

**改动**：

1. schema 升一版，`inbox` 增加 `terminal_kind TEXT NULL`（取值 `understood` / `rejected` / `interrupted`）；读侧状态表达式优先取该列。
2. 新增一条**不依赖 lease** 的结算入口 `settle_dispatch(interaction_id, terminal_kind, now_ms)`，只对派发轮（`source_channel='agent'`）开放。
3. 运行侧在轮次收口时调用它：成功写 `understood`，失败写 `rejected`，中断写 `interrupted`。

**不要做**：不要复用 `mark_*`（它们与 lease 生命周期绑定）；不要给派发轮建 `work_items`（会被调度器语义污染）。

**验收**：跑一轮目标执行后，`GET .../interactions` 里该轮 `status=understood`；制造一次失败后为 `rejected`；中断后为 `interrupted`。

### W3 逐轮指标归属（P0，三处必须同批改）

**现状链路（逐跳都可验证）**：

1. `publish_turn_metrics` 走的是 `store.append_events(...)`，这条路径不带 interaction 参数（`run_turn_metrics.rs:138-143`）。
2. `append_events` 里事件归属列只在带 lease 时赋值：`interaction_column = interaction_lease.map(|(id, _)| id.as_str())`，无 lease 即 `NULL`（`event_log_repository.rs:89`）。
3. 索引侧 `latest_turn_metrics` → `read_latest_per_interaction` → `collect_latest` 只认 `record.interaction_id`，取不到就跳过（`crates/axon-eventlog/src/tail.rs::collect_latest`）。
4. 派发 interaction 的标识由 `record_turn_input` 内部新建，没有回填 `request.interaction_id`，因此 `persist_turn_started` 也不会把它绑成 `sessions.execution_id`。

**影响面比预想大**：不只是 `goal` / `subagent`，**CLI 直跑的 primary 轮同样拿不到逐轮读数**。只有 daemon 托管、带 lease 写入的那些轮次（`append_interaction_events`）才有归属列。这解释了为什么部分会话的骨架分割线有耗时、部分没有。

**改动（三处，缺一处就对不上）**：

1. **回填标识**：`record_turn_input` 返回新建的 interaction id；`run_provider_orchestration.rs:176-181` 的调用顺序已经就绪（先 `record_turn_input`、后 `persist_turn_started`），把返回值写进 `request.interaction_id`，让 `sessions.execution_id` 绑定到这一枚。
2. **写入带归属**：新增一条不校验 lease、只写归属列的写入入口（例如 `append_events_for_interaction(session, host, interaction_id, events, now)`），`publish_turn_metrics` 改走它；`append_events` 保持现状不动，避免打开「无归属写入」的口子。
3. **轮次键同源**：`TurnMetrics.turn_id` 现在取 `assistant_part_id`（`{前缀}-{invocation_id}`，见 `run_session_store.rs:358`），必须改成这一枚 interaction id。原因是客户端按 `turnId === turn.id` 过滤收尾指标帧（`frames.ts:608-610`），键不同源会被整帧丢掉，界面上表现为「有累计、无逐轮」。

**不要做**：不要让 `publish_turn_metrics` 去伪造 lease；不要给派发轮建 `work_items` 换取归属列。

**验收（三条同时成立）**：

- 索引里每条 interaction 都带 `metrics`（`activeMs` 与 token 分桶非零）；
- CLI 直跑一轮后，同一页索引里那一轮也有读数（覆盖「无 lease 也不算丢」）；
- 真机：骨架分割线出现耗时与 token，且逐轮之和与 `session_metrics` 累计自洽（允许在跑轮差一条）。

### W4 模型事实（P2，可与 W1 合并交付）

**现状**：派发 interaction 的 `model` / `reasoning_effort` 为空，`effective_model_payload` 读 inbox 最新一轮时拿不到。

**改动**（二选一，优先前者）：派发时把本轮模型与强度写进 interaction；或在 `effective_model_payload` 缺值时回退 `sessions.provider / model / model_name`。

**验收**：`goal` 会话的索引 payload 里 `effectiveModel` 非空，且与 `sessions` 行一致。

### W5 轮次头可发现（P1）

**现状**：前端靠 `workspace.sessions` 里的 `interaction_count` 发现「这条会话又多了一轮」；独立会话不在该清单，切成交互订阅后，跑动中的新轮次不会被接管，界面停在旧轮。

**改动**（已定稿）：

- WS 新增一路订阅 `subscribeId=session-turns:{sessionId}`，只推 `interaction.created` 与 `interaction.settled` 两类轻帧，不推正文；订阅、退订语义沿用现有 WS 协议。
- 首轮只建基线、不补历史。为消除「先读索引、后订阅」的竞态，`subscribed` 回执额外带 `baseline`（`latestInteractionId` / `latestInteractionStatus` / `latestInteractionReceivedAtMs`），表示这条流在建立时已经知道哪些轮次。
- 客户端顺序固定为**先订阅、收到 `subscribed`、再读索引**。只能先读索引的实现，必须拿 `baseline.latestInteractionId` 与索引最新一轮比对，落后就再读一次；否则两次动作之间新建的轮次会被折进基线，`interaction.created` 永不播报。

**验收**：目标执行在跑时，只订阅轮次头即可发现新轮（可用 `python websockets` 带 Authorization 复现）；`session-turns:` 与 `interaction:` 前缀不冲突；订阅建立后立刻新建的一轮在下一个节拍内播报 `interaction.created`；空会话的 `subscribed` 不带 `baseline`。

### W6 中断与取消链路（P1，已定稿）

**结论**：中断接口只服务主 turn。`/api/v1/sessions/{id}/interrupt` 置位 `active_primary_turns` 里那条轮的取消令牌，其余会话一律回 `interrupted=false`，`canInterrupt` 也只对 `primary` 为 `true`。

**目标执行不走中断接口**：它的取消只能由来源会话调用 Goal 工具（`builtin.goal_cancel`）发起。工具成功后做两件事，缺一不可：

1. 在目标会话上写一条取消标记，运行侧的看门狗按有界周期消费它并触发本轮取消令牌（与主 turn 的取消令牌同一条收口链路）；
2. 在目标会话的 `inbox` 里追加一条已收口的派发轮次，把这次取消记成该会话的一轮（`create` / `update` / `start` 同样各记一条）。

目标轮与用户轮共用主 turn 管道，因此执行要求本身也是一条入站事实：`goal_create`（带执行）与 `goal_start` 投递首轮要求，执行中的 `goal_update` 追加一条新要求，模型声明 `continuation` 时下一阶段同样以入站事实推进。取消标记命中后本轮以 `interrupted` 收口，账本落 `cancelled`。

**验收**：目标执行运行中调用 `goal_cancel` 后该轮以 `interrupted` 收口；`goal` 会话的中断按钮不出现（`canInterrupt=false`）；`goal` 会话的轮次列表里能看到创建、更新、启动、取消各自的记录。

### W7 子代理名册结构（P2）

**现状**：前端自己把 `subagent:/root/x` 归一成 `root_x` 才能匹配，且 `hasRuntime=false` 走另一条文案。

**改动**：payload 增加 `agentPath`（`/root/x`）与可直接匹配的 `agentId`（`subagent:/root/x`）；「无会话」统一为 `sessionId: null` + `reason`。`slug` 可以保留但不再作为匹配键。

**验收**：前端删掉 `slugOf()` 与 `hasRuntime` 分支后仍能定位会话。

### W8 看板入口事实（P2）

**现状**：入口能否进入靠 `latestExecution.sessionId` 是否为空二分，且该字段经一次子会话映射 + 回退。

**改动**：`latestExecution` 直接给出逐字稿所在会话的 `sessionId`，并增加 `hasTranscript` 与 `unavailableReason`；中间映射结果落成正式字段，不再让调用方猜。

**验收**：卡片入口文案与 `hasTranscript` 一致；进入后订阅的会话一定有逐字稿。

## 四、不做清单

- 不新增 `GET /sessions/{id}/turns` 或 `GET /sessions/{id}/events`：`interactions` 索引已够。
- 不把 `goal` / `subagent` 放进 `/api/v1/sessions` 清单。
- 不为「只读」新造权限系统：`kind` + 能力位足够。
- 不写兼容层、不做存量迁移、不保留旧 schema 版本判定。
- 不在事件里同时保留两枚轮次键：`assistant_part_id` 与 interaction 标识只能留一枚，否则两套键还会再漂移一次。
- 不改前端实现（`crates/axon-desktop/ui/**` 由另一路负责）。

## 五、验收（缺一不得收口）

```powershell
cargo fmt --all --check
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
pwsh -File tools/verify-architecture.ps1
pwsh -File tools/verify-docs.ps1
git diff --check
```

真机（安装态二进制，按阶段 V）：

1. **目标执行**：看板进入后索引有 N 轮、`session.writable=false`、每轮 `metrics` 非空、跑动中新轮自动出现。
2. **子代理**：名册点开后不再依赖 slug 归一化，行为同上。
3. **侧栏主会话**：行为零变化（回归项）。

契约快照：把 `primary` / `goal` / `subagent` 三段索引 payload 落成测试夹具，断言 `session` 块字段齐备、能力位取值正确。

## 六、顺序与风险

**顺序**：W1 → W2 → W3 → W5 → W6 → W4 → W7 → W8。

**风险**：

- W2 若给派发轮建 `work_items`，调度器会把它当主 turn 领走；现有 `kind` 过滤是唯一防线，不要动那层。
- W3 改的是共享载荷与轮次键：`TurnMetrics.turn_id` 的消费方包括 TUI 指标行、网关回放与客户端收尾帧过滤（`frames.ts:608-610`），三处必须一次改齐；动手前先 `trace_path` 追一遍 `publish_turn_metrics` 的调用者与 `turn_id` 的读取方。改动后 `assistant_part_id` 只保留「消息分段标识」这一种含义，不再当轮次键。
- W5 新增订阅前缀必须与 `session:` / `interaction:` 三分清晰，否则帧过滤会串台。
- W1 / W4 / W7 / W8 是纯读侧，可单独回退；W2 / W3 涉及 schema 与共享载荷，回退要读写两侧同步。

## 七、交付格式

- 提交信息用中文，不出现 en/em 破折号。
- 同步文档：`docs/dev/storage-architecture.md`（inbox 终态列与派发轮结算）、`docs/skills/operate-axon/references/gateway-http.md`（`session` 块）、`docs/skills/operate-axon/references/gateway-websocket.md`（轮次头订阅）、`docs/dev/workbench-design-system.md`（能力位口径）。
- 真机取证保留 CDP `OK:` 回执与 daemon accesslog 关键行；未取证项如实标注。

## 八、交接

本提示词只覆盖后端。W1 / W2 / W3 落地后通知前端负责人开始切换 `targetOfSession` 与只读保护改造；W5 / W6 落地后再开启「跑动中新轮」与「中断按钮」。
