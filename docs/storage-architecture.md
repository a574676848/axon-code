# AXON 存储架构重构方案

> 目标：参照 DeepSeek Harness（DSH）的会话持久化模型，把 AXON 的运行态存储从"单一大 SQLite 事实源"重构为"事件面 + 控制面 + 投影面"三层结构。
>
> 前提：不考虑旧数据兼容，不做迁移。本机运行态数据直接废弃重建，仅保留人格、配置、Skill、记忆、用量等运营数据。
>
> 本文所有结论均来自当前仓库源码与运行态实测，关键位置标注 `文件:行号`。

## 1. 现状测绘

### 1.1 存储单元清单

| 存储 | 路径 | 归属 | 体量（本机实测） | 性质 |
| --- | --- | --- | --- | --- |
| 运行态事实源 | `<root>/data/runtime.sqlite3` | `axon-session` | 1903.5 MB | 已删除 |
| 会话 rollout | `<root>/data/sessions/rollout.sqlite3` | `axon-cli` | 495 MB | 已删除 |
| Thread 事件分片 | `<root>/data/sessions/threads/*.jsonl` | `axon-session` | 104 MB | 已删除 |
| 事件归档 | `<root>/data/archive/**` | `axon-cli` | 624.6 MB | 已删除 |
| 运行日志 | `<root>/data/logs/**` | `axon-logger` | 194.1 MB | 已删除 |
| Cron 定义 | `<root>/cron/cron.sqlite3` | `axon-cli` | 3.9 MB | 已备份后删除 |
| 用量统计 | `<root>/usage/usage.sqlite3` | `axon-cli` | 1.5 MB | 已备份后删除 |
| 人格 | `<root>/data/persona/persona.sqlite3` | `axon-persona` | 0.5 MB | 已备份后删除 |
| 记忆权威源 | `<root>/data/memory/memory.db` | `axon-memory` | 2.6 MB | 已备份后删除 |
| 记忆索引 | `<root>/data/memory/{tantivy,hnsw.idx}` | `axon-memory` | 1.1 MB | 已备份后删除 |
| MCP Schema 缓存 | `<root>/cache/mcp_tool_schemas.sqlite3` | `axon-mcp-runtime` | 0.1 MB | 已删除，可重建 |
| 全库检索索引 | `<root>/search-index/**` | 未定位 | 169 MB | 已删除，可重建 |

路径常量：`crates/axon-config/src/lib.rs:677`（`AXON_RUNTIME_DATA_DIR = "data"`）、`crates/axon-session/src/infrastructure/mod.rs:55-62`（`SESSION_STORE_DIR = "sessions"`、`session_store_path`）、`crates/axon-cli/src/cron_commands/runtime_cleanup.rs:26`（`RUNTIME_DATABASE_FILE = "runtime.sqlite3"`）。

### 1.2 runtime.sqlite3 的表清单

Schema 定义在 `crates/axon-session/src/infrastructure/sqlite_runtime_store/schema.rs:107-499`，共 22 张表。按写入频率与性质分类：

| 分类 | 表 | 写入频率 | 现有写入位置 |
| --- | --- | --- | --- |
| 高频事件流 | `session_events` | 数十行/秒，逐批一次 IMMEDIATE 事务 | `event_repository.rs:100-229` |
| 会话水位 | `session_runtimes` | 每批事件更新一次 | `event_repository.rs:202-213` |
| 租约 | `daemon_hosts` | 每 10 秒一次 + **每个业务事务一次 fence** | `runtime_repository.rs:162-187`、`helpers.rs:126-136` |
| 调度 | `runtime_schedules` | 到期 claim | `schedule_repository.rs` |
| 交互 | `jarvis_interactions` | 每轮数次 | `interaction_repository.rs` |
| 执行队列 | `jarvis_executions` | claim / commit / 租约续期 | `execution_repository.rs`、`action_repository.rs:94-132` |
| 目标 | `jarvis_goals` / `jarvis_results` / `jarvis_result_reads` | 低频 | `goal_repository.rs`、`result_repository.rs` |
| Agent 消息 | `jarvis_agent_messages` / `jarvis_continuations` | 低频 | `agent_mailbox_repository.rs`、`agent_continuation_repository.rs` |
| 交付 | `jarvis_completion_notifications` / `jarvis_completion_deliveries` / `jarvis_client_presence` | 低频 | `client_delivery_repository.rs` |
| 决策与账本 | `jarvis_decisions` / `jarvis_events` | 低频 | `decision_repository.rs`、`ledger_event_repository.rs` |
| 用户提问 | `jarvis_user_questions` | 低频 | `user_question_repository.rs` |
| 升级 | `daemon_upgrade_admissions` | 升级时 | `runtime_repository.rs:189-277` |
| 游标 | `transport_ingress_cursors` | 低频 | `ingress_cursor_store` |
| 元数据 | `jarvis_schema_identity` / `jarvis_workspaces` | 低频 | `schema.rs`、`workspace_repository.rs` |
| 检查点 | `session_checkpoints` | 每次 commit 一次 | `commit_repository.rs:194` 写入；**生产代码无 SELECT**，配套索引 `session_checkpoints_latest`（`schema.rs:153`）无查询使用，可直接降级为事件 |

**关键事实**：1903.5 MB 中 `session_events` 主表 1384.7 MB、三个索引 513.4 MB，其余 20 张表合计不足 2 MB。也就是说，**99.9% 的体量来自一张事件表**。

### 1.3 事件表的结构代价

`session_events`（`schema.rs:129-138`）有 4 个物理 B-tree：

1. 主表 `session_events`
2. 主键隐式索引 `sqlite_autoindex_session_events_1 (session_id, sequence)`
3. `session_events_retention (emitted_at_ms, session_id, sequence)`
4. `session_events_session_interaction (session_id, interaction_id, sequence)`（`schema.rs:577-579`）

每次 `INSERT` 要更新 4 个 B-tree，1.9 GB 里 27% 是索引副本。

### 1.4 写入路径的真实形状

**重要前提：并非每一帧都会走到落库路径。** Daemon 侧已经有一层帧分类（`crates/axon-daemon/src/application/interaction_turn_event_frame_class.rs:44-79`），在 `interaction_turn_event_sink.rs:179-226` 的 `dispatch` 里分派：

| 分类 | 判据 | 落库行为 |
| --- | --- | --- |
| `LivePart` | `reasoningDelta`、`assistantMessage` 且 `delta=true` | **不逐帧落库**：只走实时投递并累积，到语义边界由 `flush_live_parts` 落一条快照 |
| `LiveOnly` | `providerStatus` | 只走实时，另留一份可补发副本（内存） |
| `Snapshot` | `usageUpdate` | 实时投递 + 一份持久副本 |
| `Durable` | 其余全部 kind | 先 `retire_part_for` + `flush_live_parts`，再落这一条语义事件 |

也就是说"正文增量逐帧入库"这个问题**在代码里已经被修掉了**。§1.5 的 186 万行是修复上线之前的历史存量，不能当作当前写入速率的证据。

真正落到 `Durable` 的批次仍走同一个事务（`event_repository.rs:107-229`）：

1. `ensure_fence` → `UPDATE daemon_hosts SET updated_at_ms = updated_at_ms`（`helpers.rs:131`，一次真实写入 + WAL 记录）
2. `SELECT` 校验 `jarvis_interactions` 的 processing lease（`:113-125`）
3. `SELECT last_event_sequence FROM session_runtimes`（`:126-134`）
4. N 次 `INSERT INTO session_events`（`:151-201`）
5. `UPDATE session_runtimes SET last_event_sequence = ?`（`:202-213`）
6. 可能的 `UPDATE jarvis_interactions SET tool_summary = ?`（`:214-227`）

连接 PRAGMA（`connection_pool.rs:34-35`）：`foreign_keys=ON`、`synchronous=FULL`、`journal_size_limit=67108864`、`wal_autocheckpoint=1000`。WAL 模式由建库路径一次性设置（`schema.rs:32-41`）。

**这改变了重构的重心**：不是"停止写 delta"（已完成），而是"把剩下的写入放到不会互相阻塞的介质上"，并把历史存量与保留策略一次性处理掉。

### 1.5 事件内容的构成（历史存量）

删除前本机 186 万行事件的分布：

| event_type | 行数 | 占比 |
| --- | --- | --- |
| `wire_event` | 1,862,045 | 99.975% |
| `turn_completed` | 466 | 0.025% |
| `interaction_interrupted` | 20 | 0.001% |

按天分布：11-10 到 11-15 每天 20 万到 44 万帧，而 11-16 到 15:22 只有 30,468 帧。这个断崖与帧分类上线的时间点吻合，支持"历史存量"的判断。

**结论：1.9 GB 是过去那套「逐帧入库」留下的存量，不是当前架构的稳态产出。** 存量清理（按会话归档或删除）与介质切换同等重要，而"降低写入频率"已经不是主要矛盾。

### 1.6 双轨与重复

AXON 目前同时维护两份会话正文：

- `session_events`（`runtime.sqlite3`）：帧级事件流
- `rollout_turns`（`rollout.sqlite3`）：turn 级正文，本机 433.3 MB

外加 Thread 层已经走了 JSONL 分片（`crates/axon-session/src/infrastructure/jsonl_store.rs`，按 `threads/<thread_prefix>.jsonl` 分片）。

**结论：存储架构已经是三轨并存，且最重的一条恰恰是最不该用关系表承载的那条。**

### 1.7 已有的可复用资产

重构不需要从零造轮子，仓库里已经有：

| 能力 | 位置 | 状态 |
| --- | --- | --- |
| 分片 JSONL 存储 | `crates/axon-jsonl/src/store.rs` | 已具备：shard 滚动（默认 8 MiB）、单条 64 MiB 上限、`fs4` 跨进程文件锁 + 进程内 Mutex、崩溃尾行修复 `repair_tail`、`sync_data` 持久化模式、保留期淘汰 `append_retained_line`、容量预算 `append_bounded_line`、原子重写 `rewrite_with` |
| Thread 事件层 | `crates/axon-session/src/infrastructure/jsonl_store.rs` | 已具备：按 thread 分片、hex/摘要文件名规避 Windows 路径上限（`:70-80`）、反向扫描激活状态（`:52-54`）、容错扫描 |
| zstd 依赖 | workspace `Cargo.toml:171` | 已有 `zstd = "0.14"`，`axon-cli`、`axon-provider-transport` 已使用 |
| 归档压缩 | `crates/axon-cli/src/cron_commands/runtime_event_archive.rs` | 已实现 `.jsonl.zst` 分批归档（`ARCHIVE_BATCH_ROWS = 2000`，`ARCHIVE_ZSTD_LEVEL = 3`） |
| 文件锁 | workspace `Cargo.toml:157` | 已有 `fs4` |

## 2. DSH 的设计要素

DSH 在 `packages/session/` 下把持久化拆成 seam 与后端，其可移植的要素如下。

### 2.1 分层：seam / backend / policy / projection

- `session-persistence`：定义 `SessionPersistence` 抽象服务与 `SessionHandle` 契约（`create`/`open`/`stat`/`list`/`flush`/`close`/`read`/`append`），不含实现。
- `session-persistence-jsonl`：唯一的一等后端，每会话一个 append-only 日志。
- `session-checkpoint-policy`：独立插件，只决定"什么时候必须 flush"，不含存储逻辑。
- `session-projection` / `session-projection-cache`：读侧投影，缓存的是**内存表**，可从日志重建。

**可移植点**：AXON 的 `RuntimeStore` 是把"事件存储"和"状态机"合并成一个大 trait（`crates/axon-session/src/ports/runtime_store.rs:55-73`，聚合了 15 个 sub-store trait）。DSH 的做法是**按存储性质拆 seam**，而不是按业务实体拆。

### 2.2 物理格式：一段日志 = 若干独立 zstd frame

- 一个会话一个文件：`session.vN.jsonl.zstd`。
- 第一个 frame 只放 header 行；此后**每个 durable append batch 一个独立 checksummed frame**。
- 默认 `compression: 'zstd'`，可配 `'none'` 得到纯文本行。
- 代数（`v0`…`v4`）体现在文件名里，运行时选最高的规范代。

**可移植点**：frame 粒度等于批次，而不是文件级压缩。这样追加写不需要重压整个文件，且 torn frame 只影响最后一批。

### 2.3 写路径：有界缓冲 + 固定批窗口 + single-flight drain

- `session/event` 先进入该会话写句柄的**有界内部缓冲**，不立即落盘。
- 第一条待写事件启动一个**固定长度**的批窗口，后续事件加入但**不重置**窗口期限。
- 窗口到期把待写前缀 drain 出去，每个 batch 追加一个 frame 并 `fsync`，然后才 resolve。
- drain 期间新到的事件合并进下一个链式批次，顺序不变。
- 后台写失败时**保留事件、暂停自动路径、记日志**；下一次显式 flush 重试并报错。

**可移植点**：这是"把 N 次/秒的 fsync 压成 1 次/窗口"的机制。AXON 现在每批一个 IMMEDIATE 事务 + `synchronous=FULL`，本质上是同一种痛苦，但没有窗口合并。

### 2.4 耐久性边界：语义 checkpoint，而非时间 checkpoint

`session-checkpoint-policy` 只在三处强制 flush：

1. model request 前（构造 adapter stream 之前）
2. 顶级 tool body 执行前（外部副作用之前）
3. `agent/pre-step` 边界（下一步请求推导之前）

失败即 fail-closed：adapter 或 tool body 不运行。

**可移植点**：AXON 当前的 `synchronous=FULL` 是"每一笔都当关键路径"，代价极高。改成"按语义边界决定 flush"，把大多数增量排除在持久性承诺之外。

### 2.5 崩溃语义

- 未完成的 Assistant 流保持 transient，**不落盘**。
- 追加中崩溃：已提交前缀全部保留；最后一行不完整则丢弃。
- torn zstd frame 只贡献其中完整的记录；写句柄在首次新写入前把这段尾字节截断并持久化。
- 完整 frame 的 checksum / 解压 / 结构失败按损坏处理，**不猜测**。
- 恢复时追加合成收尾事件（`interruptedTurnClosers`），不重写历史。

**可移植点**：AXON 的 `repair_tail`（`axon-jsonl/src/store.rs:354-384`）已经实现了"截到最后一个换行"，但缺少 frame 级校验与"只采纳完整记录"的语义。

### 2.6 明确不做的事

DSH 的 Known Limitations 直接写明了：没有删除 API、没有保留期、`list()` 不分页不筛选。也就是说**日志无限累积是显式接受的取舍**，因为靠 zstd 与"不落 delta"已经把体量压到可接受。

**对 AXON 的启示**：保留策略应该从"按时间清理大表"退化为"日志文件天然小，只按会话归档或删除"。

## 3. 关键差异：不能全盘照搬

| 维度 | DSH | AXON |
| --- | --- | --- |
| 进程模型 | 单进程 + kernel lock 排斥第二写者 | **多进程共享**：daemon 常驻、CLI run、子代理执行面同时打开同一个库（见 `schema.rs:66-70` 注释与 `crates/axon-cli/src/run_commands/run_agent_store.rs:12` `shared_runtime_store`） |
| 原子 claim | 不需要（单写者） | **需要**：`claim_next_action`、`claim_due_schedules`、`recover_expired_sessions`、`jarvis_interactions` 的 lane 排他领取，都依赖 SQLite 事务做跨进程 CAS |
| 事件类型 | 只落语义事件，delta 为 transient | 已用帧分类把 delta 挡在 durable 层之外（`frame_class.rs:44-79`），但历史上曾逐帧落库，库内还压着 186 万帧存量 |
| 查询需求 | `list()` 全量，无分页 | 桌面端需要 `interaction_tail_window`、`replay_events(after_sequence)`、按 lane 的最近交互列表 |

**结论：SQLite 不能整体消灭，必须保留一个"控制面"库**，承载租约、队列、状态机这类需要跨进程原子性的小表；把 99.9% 的体量（事件流、rollout 正文）迁到 append-only 压缩日志。

这个结论还有历史依据：仓库此前的架构决策已经确立"Goal / Workflow 权威状态保存在独立 SQLite"（`docs/` 无对应文档，但代码里 `jarvis_goals`/`jarvis_executions` 的设计与 DSH 的 `goals_1.sqlite` 独立库思路一致）。

## 4. 目标架构

### 4.1 库划分原则

SQLite 没有跨库事务，所以拆库不能按业务随意切，只能按**事务边界**切。四条原则：

1. **同一事务内必须原子提交的表，必须在同一个库。** 例如 claim execution 与推进 `sessions.last_event_sequence`，或 goal 结算与 result 写入。
2. **生命周期不同则分库。** 能整体归档或删除的表（outbox、audit）不与核心状态机同库，避免清理时锁住热路径。
3. **写热路径与写冷路径分库。** 一个低频表不应该和高频表共享同一个 WAL 与 checkpoint。
4. **事件流不建库。** 追加型叙述用压缩文件，SQLite 只承载需要原子条件更新的状态。

### 4.2 命名规范

`jarvis_` 是早期产品代号遗留（当前产品叫 Axon），必须去掉。规范如下：

| 规则 | 说明 |
| --- | --- |
| 不用产品代号做前缀 | 产品改名时前缀会再次过时；归属由所在库表达，不由表名表达 |
| 表名用复数名词 | 表里装什么就叫什么：`sessions`、`interactions`、`goals` |
| 状态机与历史分开命名 | 在途用 `*_queue`，终态用 `*_history` |
| 列名固定后缀 | 时间 `*_at_ms`（Unix 毫秒）、载荷 `*_json`、标识 `*_id` |
| 索引名表达用途 | `<table>_<用途>`，不再出现 `_retention`、`_reclaim` 这类实现期词汇 |
| schema 版本用 `PRAGMA user_version` | 删掉 `jarvis_schema_identity` 表 |

### 4.3 库清单

| 库 | 文件 | 内容 | 预估体量 |
| --- | --- | --- | --- |
| control | `<root>/data/control.sqlite3` | 实例租约、会话状态、交互、执行队列与历史、目标与结果、决策、工作区、调度、游标、升级交接 | 小于 20 MB |
| outbox | `<root>/data/outbox.sqlite3` | Agent 消息、continuation、完成通知与投递、用户提问、客户端在线 | 小于 5 MB |
| audit | `<root>/data/audit.sqlite3` | 跨会话账本事件，append-only | 可整体归档 |
| 事件面 | `<root>/data/events/**` | 会话事件日志，zstd 分帧 | 按会话，见 §7 |
| 运营库 | `usage/`、`cron/`、`persona/`、`memory/` | 保持不变，不属于本次重构 | 不变 |

### 4.4 表设计

#### 4.4.1 control.sqlite3（14 张）

| 新表 | 原表 | 主键 | 关键索引 | 说明 |
| --- | --- | --- | --- | --- |
| `daemon_instances` | `daemon_hosts` | `instance_id` | `(lease_expires_at_ms)` | 进程实例租约，fencing 的唯一锚点 |
| `sessions` | `session_runtimes` | `session_id` | `(status, owner_instance_id, owner_generation)`、`(last_activity_at_ms, status)`、`(workspace_id, archived, sort_order)` | 稳定对话身份 + 当前运行实例。**不含 channel / destination**，见 §4.13 |
| `session_metrics` | 新增（投影） | `session_id` | 主键即索引 | 会话累计：轮次、三类耗时、首字分布、token、作业计数。可由事件重放重建，见 §4.11 |
| `inbox` | `jarvis_interactions` | `inbox_id` | `(channel, source_message_id)` 唯一、`(session_id, received_at_ms DESC, inbox_id DESC)` | 只读入站事实：来源、幂等键、内容、受众、回复路由。**不含状态机**，见 §4.14 |
| `work_items` | `jarvis_executions`（在途） | `work_id` | `(status, kind, available_at_ms)`、`(session_id, priority DESC, created_at_ms)`、`(dedupe_key)` 部分唯一 | 统一队列：turn / goal / cron / subagent。**一套 claim 与 lease** |
| `work_item_history` | `jarvis_executions`（终态） | `work_id` | `(goal_id, finished_at_ms DESC)`、`(session_id, finished_at_ms DESC)` | 终态与重试历史，append-only |
| `goals` | `jarvis_goals` | `goal_id` | `(status, updated_at_ms)`、`(scope_json, status, updated_at_ms)` | 目标权威状态 |
| `goal_results` | `jarvis_results` + `jarvis_result_reads` | `result_id` | `(goal_id, created_at_ms DESC)`、`(execution_id)` 唯一、`(created_at_ms)` | 已读标记合并为 `acknowledged_at_ms` + `acknowledged_source` 两列 |
| `schedules` | `runtime_schedules` | `schedule_id` | `(status, next_run_at_ms, lease_expires_at_ms)` | 调度 |
| `decisions` | `jarvis_decisions` | `decision_id` | `(scope_json, revision)` 唯一、`(effect, expires_at_ms)` | 决策 |
| `workspaces` | `jarvis_workspaces` | `workspace_id` | `(last_seen_at_ms DESC)` | 工作区注册表 |
| `ingress_cursors` | `transport_ingress_cursors` | `(adapter, partition_key)` | 主键即索引 | 渠道游标 |
| `channel_bindings` | 新增 | `(channel, destination)` | 主键即索引、`(session_id)` | 无 UI 入站渠道到会话的固定映射，见 §4.13。只服务入站路由，不参与会话身份 |
| `upgrade_handoffs` | `daemon_upgrade_admissions` | `handoff_id` | `(state, deadline_at_ms)`、`handoff_token` 唯一 | 升级交接 |

`inbox` 的索引从原来的 5 条收敛为 2 条。原 `scope_json` 三连（`scope_status_recent`、`scope_channel_recent`、`scope_channel_status_recent`）只服务渠道回落，合并后由 `(session_id, received_at_ms DESC)` 覆盖；`lane_recent` 与 `lane_processing` 两族随状态机一起搬到 `work_items`。

#### 4.4.2 outbox.sqlite3（6 张）

| 新表 | 原表 | 说明 |
| --- | --- | --- |
| `agent_messages` | `jarvis_agent_messages` | Agent 间消息，幂等键唯一 |
| `continuations` | `jarvis_continuations` | 子会话回传，幂等键唯一 |
| `notifications` | `jarvis_completion_notifications` | 完成通知 |
| `notification_deliveries` | `jarvis_completion_deliveries` | 通知投递尝试 |
| `user_questions` | `jarvis_user_questions` | 问答内容与截止时间；等待状态由 `work_items.waiting` 表达 |
| `client_presence` | `jarvis_client_presence` | 客户端在线，TTL 清理 |

放同一个库的理由：这六张表都是"有界待消费"队列，可按过期时间整批删除，与需要长期保留的 control 表生命周期完全不同。分库后清理 outbox 不会与核心事务争同一把锁。

#### 4.4.3 audit.sqlite3（1 张）

| 新表 | 原表 | 说明 |
| --- | --- | --- |
| `ledger_events` | `jarvis_events` | 跨会话账本，`event_id` 主键、`idempotency_key` 唯一、`sequence` 单调 |

单独成库的理由：它是 append-only 的审计流水，可以整体归档或按序列区间删除，永远不参与业务条件更新。

### 4.5 旧表到新表的映射

22 张旧表收敛为 21 张新表（control 14 + outbox 6 + audit 1），其中一张迁出为文件，两张为新增。

| 旧表 | 新归属 | 动作 |
| --- | --- | --- |
| （新增） | `control.session_metrics` | 会话累计投影，见 §4.11 |
| （新增） | `control.channel_bindings` | 入站渠道到会话的绑定，见 §4.13 |
| `jarvis_schema_identity` | 无 | 删除，改用每库 `PRAGMA user_version` |
| `session_events` | `data/events/<session>/session.v1.jsonl.zstd` | 迁出为文件 |
| `session_checkpoints` | 事件 `kind = checkpoint` | 合并删除（生产代码零 SELECT） |
| `jarvis_result_reads` | `control.goal_results` 两列 | 合并删除 |
| `session_runtimes` | `control.sessions` | 重命名，去掉 channel / destination |
| `daemon_hosts` | `control.daemon_instances` | 重命名 |
| `jarvis_interactions` | `control.inbox` | 去前缀，状态机搬走 |
| `jarvis_executions` | `control.work_items` + `control.work_item_history` | 按在途 / 终态拆分，`kind` 扩展 |
| `jarvis_goals` | `control.goals` | 去前缀 |
| `jarvis_results` | `control.goal_results` | 去前缀 |
| `runtime_schedules` | `control.schedules` | 重命名 |
| `jarvis_decisions` | `control.decisions` | 去前缀 |
| `jarvis_workspaces` | `control.workspaces` | 去前缀，加 `active_session_id` |
| `transport_ingress_cursors` | `control.ingress_cursors` | 重命名 |
| `daemon_upgrade_admissions` | `control.upgrade_handoffs` | 重命名 |
| `jarvis_agent_messages` | `outbox.agent_messages` | 迁库 |
| `jarvis_continuations` | `outbox.continuations` | 迁库 |
| `jarvis_completion_notifications` | `outbox.notifications` | 迁库 |
| `jarvis_completion_deliveries` | `outbox.notification_deliveries` | 迁库 |
| `jarvis_user_questions` | `outbox.user_questions` | 迁库 |
| `jarvis_client_presence` | `outbox.client_presence` | 迁库 |
| `jarvis_events` | `audit.ledger_events` | 迁库 |

### 4.6 重叠存储消除

当前同一份信息最多有四种载体，逐组收敛：

| 重叠组 | 现状 | 目标 |
| --- | --- | --- |
| 会话正文 | `session_events`（帧级）+ `rollout_turns`（turn 级）+ `threads/*.jsonl`（模型可见历史） | 两份：会话事件文件 + thread 文件。`rollout.sqlite3` 删除，turn 正文由事件日志的 `turn` 记录派生；用量投影器改为只读事件日志（原依赖见 `docs/usage-statistics.md:98`） |
| 事件流 | `session_events`（单会话叙述）+ `jarvis_events`（跨会话审计） | 职责分离：前者进文件，后者进 `audit.sqlite3` |
| 检查点 | `session_checkpoints` 表 + `sessions.last_checkpoint_seq` 列 | 事件 `kind = checkpoint` + 一列水位 |
| 已读标记 | `jarvis_result_reads` 独立表 | 并入 `goal_results` 两列 |
| 正文载体 | 内联 `payload_json` + `payload_ref` + `event_payload_store` 分片 + `archive/*.jsonl.zst` | 一套：zstd frame 内联，取消外置与归档两套机制 |

最后一条收益最大：现在有 `SESSION_EVENT_INLINE_PAYLOAD_LIMIT_BYTES = 4 KiB`（`sqlite_runtime_store.rs:102`）、`WIRE_EVENT_INLINE_PAYLOAD_LIMIT_BYTES = 0`（`event_payload_store.rs:33`）两套阈值、一套 `{session}/{shard:05}.jsonl@{offset}+{len}` 引用格式、一套归档回收逻辑、一套悬空引用修复。全部可以删除。

### 4.7 事件面格式


**文件布局**

```text
data/
  events/
    <session-id-escaped>/
      session.v1.jsonl.zstd     # 当前规范代
      session.v1.jsonl.zstd.idx # 可选：每 frame 的 (seq_start, byte_offset, raw_len)
  rollout/
    <session-id-escaped>/
      turn.v1.jsonl.zstd
  threads/                      # 维持现有布局不变
    <thread-prefix>.jsonl
```

会话目录名复用 `jsonl_store.rs:70-80` 的编码策略：十六进制转义，超过 96 字节改 `sha256_hex` 摘要，规避 Windows 路径上限。

**Frame 结构**

- Frame 0：header 行，含 `format_version`、`session_id`、`created_at_ms`、`cwd`、`seed`。
- Frame N（N ≥ 1）：一个 append batch，行内是连续的 `RuntimeEventRecord`。
- 每个 frame 独立 zstd 压缩，开启 checksum。
- 每条记录自带 `sequence`，写入时校验连续性；`sequence` 不连续即拒绝整批。

**记录模型（关键削减）**

```rust
struct RuntimeEventRecord {
    sequence: u64,
    emitted_at_ms: u64,
    interaction_id: Option<InteractionId>,   // 有归属才写
    kind: RuntimeEventKind,                  // 强类型枚举，不再用 String
    payload: serde_json::Value,
}
```

`RuntimeEventKind` 只包含**可重放的语义事实**：`turn_started`、`turn_completed`、`tool_call`、`tool_result`、`assistant_message`、`interaction_state`、`interaction_interrupted`、`checkpoint`、`session_recovered`、`turn_metrics`、`job_started`、`job_completed`。后三类的定义见 §4.11。

**传输增量（原 `wire_event`）不进这个日志**。桌面端渲染所需的 delta 走内存广播通道（`DaemonEventPublisher` 已有的 pub/sub），断线重连由「最近一次 `assistant_message` 终态 + 进行中的内存缓冲」重建。

### 4.8 读侧投影

需要的读能力与目标实现：

| 读需求 | 现状 | 目标 |
| --- | --- | --- |
| `replay_events(session, after_seq, limit)` | `session_events` 索引扫描 | 顺序扫描 frame + 二分 `.idx`；冷读时从最后一次 `checkpoint` 事件起扫 |
| `interaction_tail_window(session, interaction, limit)` | `session_events_session_interaction` 索引 | 反向扫描尾部 frame（复用 `jsonl_store.rs:52-54` 的反向扫描思路），按 `interaction_id` 过滤 |
| 会话内最近消息列表 | `jarvis_interactions_lane_recent` | 留在 `control.inbox`，按 `(session_id, received_at_ms DESC)` 取 |
| 会话列表与元数据 | `session_runtimes` | 留在 `control.sessions` |
| 启动激活状态 | `thread_service` 反向扫描 | 复用现有 `JsonlThreadEventStore::read_thread_activation` |
| 用量投影 | 只读 `rollout.sqlite3` 的 turn 快照 | 改为只读事件日志的 `turn` 记录，见 §9 |

投影缓存不是必须的：会话级日志本身小（预估见 §7），顺序扫几 MB 是可接受的。若真机测出首屏延迟，再加 `.idx` 旁挂索引（记录 `(seq, offset, raw_len)`），不做全量解析。

### 4.9 一致性协议

这是本方案最重要的设计约束，取代原来的"每事务 fence"。

**写序规则**

1. 事件先追加到日志并 `fsync`（或按批量策略 flush）。
2. 日志落盘成功后，才更新控制面的 `session_runtimes.last_event_sequence` 与 `last_activity_at_ms`。
3. 顺序颠倒会在崩溃时产生"水位大于日志"的悬空引用，因此必须单向。

**fencing 规则**

- 保留 `daemon_hosts` 与控制面事务，`claim_host` / `renew_host` 语义不变。
- 事件写入**不再**在每个批次里 `UPDATE daemon_hosts`（删掉 `helpers.rs:126-136` 在事件路径上的调用）。改为：
  - 写入前读一次进程内缓存的 `(host_id, generation, lease_deadline_ms)`（续租线程维护的原子值）。
  - `now > deadline - safety_margin` 直接返回 `Fenced`，fail-closed。
  - 日志记录的 `generation` 字段用于事后仲裁：读取侧忽略非当前 generation 的帧。
- 这样做的代价：进程被冻结超过租约时长后，旧 host 恢复时可能已写入几帧。但由于 `generation` 单调、且接管方会从 `last_event_sequence` 之后继续，这部分帧在读取侧可被识别为过期代际。**这是必须显式接受的语义弱化，需要在文档中写清。**

**checkpoint 规则（照搬 DSH）**

在这三个位置强制 flush：

1. Provider 请求构造完成、发起网络调用前。
2. 顶级 tool 调用进入执行体前（有外部副作用）。
3. 每个 `agent/pre-step` 边界。

其余时刻允许批窗口内的增量驻留内存。

### 4.10 崩溃恢复

| 场景 | 处理 |
| --- | --- |
| 进程在 append 中途崩溃 | frame 不完整 → 丢弃该 frame 的未完整记录；若 frame 被判定为 torn，截断到上一完整 frame 边界并持久化 |
| frame checksum 失败 | 拒绝该 frame 之后的所有内容，报损坏，不猜测 |
| 控制面水位大于日志尾部 | 视为崩溃窗口，把水位回退到日志实际尾序号，并追加一条 `session_recovered` 事件 |
| 控制面水位小于日志尾部 | 说明日志已写但控制面未提交，重放补齐 |
| 日志存在但控制面无 session 记录 | 从日志 header 重建 `session_runtimes` 行 |

### 4.11 会话级可观测事实

既然重建存储，就顺手补上三类"该有但没有"的事实。先看现状：

| 数据 | 现状 | 缺口 |
| --- | --- | --- |
| 首字延迟 TTFT | `TurnPerformanceRecorder` 采集 20 个阶段（`crates/axon-session-runtime/src/ports/turn_performance.rs:16-57`），TTFT 等于 `ProviderFirstDeltaParsed - ProviderRequestStarted` | **只在 `log_level = trace` 时启用**。`config_log_level_enables_first_token_telemetry` 只认 trace（`crates/axon-cli/src/lib.rs:500`），默认 `error` 级别完全不采集 |
| 生成速率 | `generation_speed_tps` 已写入 `usage_facts` | 是跨会话聚合投影，会话内看不到自己这轮多快 |
| token 消耗 | `input/output/reasoning/cache_read/cache_write` 五个字段齐全（`usage_statistics/repository/schema.rs`） | 同上，且依赖 rollout 投影，本次要删 |
| 后台任务 | `JobRuntime` 是进程内全局态，`JobStatus` 有 running/completed/refusal/failed/killed/timeout/unresponsive 七态 | **零持久化**。只能通过 `pending_job_ids(run_session_id)` 查在途，进程重启即丢；`agent_continuation_store.rs:74` 的 job 引用明确写着"只作可读线索，不承担解析职责" |

目标：这些事实归会话所有，写进会话事件流，由投影汇总，不再依赖日志级别与跨会话聚合。

**新增事件类型**

```rust
enum RuntimeEventKind {
    // 已有
    TurnStarted, TurnCompleted, ToolCall, ToolResult, AssistantMessage,
    InteractionState, InteractionInterrupted, Checkpoint, SessionRecovered,
    // 新增
    TurnMetrics(TurnMetrics),
    JobStarted(JobRef),
    JobCompleted(JobOutcome),
}

struct TurnMetrics {
    turn_id: String,
    interaction_id: Option<InteractionId>,
    origin_channel: Option<String>,    // 本轮的来源渠道，见下
    origin_kind: TurnOrigin,           // external | cron | goal | subagent
    ttft_ms: Option<u64>,              // None 表示未测得，禁止用 0 冒充
    provider_duration_ms: u64,
    input_tokens: u64,
    output_tokens: u64,
    reasoning_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
    generation_speed_tps: Option<f64>,
    stage_timings: Vec<StageTiming>,   // 只落关键 6 段，见下
    background_jobs_started: u32,      // 本 turn 启动的后台任务数
    background_jobs_running: u32,      // 收口时仍在跑的
}

struct JobRef { job_id: String, kind: String, lane_id: String, started_at_ms: u64 }
struct JobOutcome {
    job_id: String, status: JobStatus, elapsed_ms: u64,
    exit_code: Option<i32>, terminal_reason: Option<String>,
}
```

**来源渠道的记录方式**

渠道不参与会话身份（§4.13），但每一轮必须能回答"这条消息从哪来"。落法分两层，不做冗余存储：

| 场景 | 记录位置 |
| --- | --- |
| 有 `interaction_id` 的轮次 | `inbox.channel` 是唯一来源；事件只存 `interaction_id`，读取时 JOIN 取渠道 |
| 无 `interaction_id` 的轮次（cron / goal / subagent） | `turn_started` 与 `turn_metrics` 自己带 `origin_channel` 与 `origin_kind` |

`origin_kind` 用枚举而不是自由字符串：

```rust
enum TurnOrigin { External, Cron, Goal, Subagent }
```

`origin_channel` 只在 `External` 时有值（`tui` / `desktop` / `cli` / `feishu` / …）。不为每条事件重复写渠道字符串，因为那会把同一个值复制到几十万行里；`turn_metrics` 是一个轮次一条，冗余成本可接受，换来按渠道直接聚合而不用 JOIN。

**一个会话可以有多个渠道的轮次**

渠道属于轮次，不属于会话。第一轮在桌面发起、后续在 TUI 继续，是同一个 `session_id` 下两条 `turn_metrics`：`origin_channel` 分别为 `desktop` 与 `tui`。`sessions` 表没有渠道列，也不应该有——一旦把渠道写进会话，就等于把 §4.13 的裁决又推翻回去。

会话级的渠道分布由 `session_metrics.channel_mix_json` 维护，纯展示与聚合用途，可由 `turn_metrics` 重放重建：

```json
{
  "desktop": { "turns": 3, "inputTokens": 12000, "outputTokens": 3400 },
  "tui":     { "turns": 2, "inputTokens":  8000, "outputTokens": 2100 }
}
```

入站路由也允许一个会话被多个渠道绑定：`channel_bindings` 主键是 `(channel, destination)`，多行可以指向同一个 `session_id`。桌面与 TUI 不写这张表（它们直接传 `session_id`），只有飞书群、外部 webhook 这类无 UI 入口才需要。

### 4.11.1 turn_metrics 的生产链

采集发生在 CLI 进程（`run_session_store` 已能算出 TTFT 与速率），消费发生在 daemon 进程（结算时落事件），两者当前没有通道。落法分三段：

1. **采集（CLI）**：provider turn 结束时构造 `TurnMetrics`：
   - `ttft_ms` ← `compute_request_ttft_ms`
   - `generation_speed_tps` ← `compute_generation_speed_tps`
   - 五类 token ← `TokenUsageTelemetry`
   - `active_ms` ← turn 起止时间差
   - `outcome` ← turn 终态（succeeded / failed / interrupted）
   - 结果写进 `SessionActionRecord.payload_json` 的 `metrics` 键，与 `axon_eventlog::TurnMetrics` 同构。
2. **搬运（daemon 结算）**：`execution_worker_settlement::commit_conclusion` 从 action payload 取出 `metrics`，转成一条 `turn_metrics` 事件，与终态事件一起传给 `commit_action` 的 `events`。
3. **投影（自动）**：`event_log_repository::apply_projections` 识别该事件并累加进 `session_metrics`，不需要额外接线。

两条硬规则：

- `payload_json.metrics` 缺失时按"本轮未测得"处理，**不填 0**，否则平均值被稀释。
- `metrics` 只是回传通道，不是权威事实：权威事实是落盘后的 `turn_metrics` 事件。

**采集策略**

1. **始终采集，不受日志级别控制。** `first_token_telemetry_enabled` 的 trace 开关只保留给"逐阶段明细日志"，不再决定是否落库。
2. `stage_timings` 从 20 段压到关键 6 段：`WorkerClaimed→HistoryReady`、`→PayloadReady`、`→ProviderRequestStarted`、`→ProviderFirstDeltaParsed`（即 TTFT）、`→FirstEventCommitted`、`→FirstEventPublished`。全量 20 段只在 trace 日志里出现。
3. `TurnMetrics` 每 turn 一条，与 `turn_completed` **同帧写入**，不额外占一次 fsync。
4. 后台任务在 `JobStarted` 与 `JobCompleted` 各一条。`background_jobs_started` 让"这个会话跑了几个后台任务"一次读出，不必扫事件。
5. 会话累计由 `control.session_metrics` 一对一表维护（详见下节）。

**会话累计：`control.session_metrics`**

每轮的事实进事件流，累计值走投影表。不塞进 `sessions`，因为列会膨胀到二十几列，而 `sessions` 是每次 claim 都更新的热行。

```sql
CREATE TABLE session_metrics (
    session_id            TEXT PRIMARY KEY,
    -- 轮次
    turn_count            INTEGER NOT NULL DEFAULT 0,
    turn_succeeded        INTEGER NOT NULL DEFAULT 0,
    turn_failed           INTEGER NOT NULL DEFAULT 0,
    turn_interrupted      INTEGER NOT NULL DEFAULT 0,
    -- 耗时
    total_active_ms       INTEGER NOT NULL DEFAULT 0,   -- 累计 turn 执行时长，不含空闲
    total_provider_ms     INTEGER NOT NULL DEFAULT 0,   -- 累计 provider 往返
    total_tool_ms         INTEGER NOT NULL DEFAULT 0,   -- 累计工具执行
    max_turn_ms           INTEGER NOT NULL DEFAULT 0,
    -- 首字
    ttft_sum_ms           INTEGER NOT NULL DEFAULT 0,
    ttft_samples          INTEGER NOT NULL DEFAULT 0,   -- 只统计测得值，0 不计入
    ttft_max_ms           INTEGER NOT NULL DEFAULT 0,
    ttft_histogram_json   TEXT NOT NULL DEFAULT '[]',   -- 16 桶，毫秒对数分桶
    turn_histogram_json   TEXT NOT NULL DEFAULT '[]',
    -- token
    total_input_tokens      INTEGER NOT NULL DEFAULT 0,
    total_output_tokens     INTEGER NOT NULL DEFAULT 0,
    total_reasoning_tokens  INTEGER NOT NULL DEFAULT 0,
    total_cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    total_cache_write_tokens INTEGER NOT NULL DEFAULT 0,
    -- 工具与后台任务
    total_tool_calls      INTEGER NOT NULL DEFAULT 0,
    jobs_started          INTEGER NOT NULL DEFAULT 0,
    jobs_completed        INTEGER NOT NULL DEFAULT 0,
    jobs_failed           INTEGER NOT NULL DEFAULT 0,
    jobs_running          INTEGER NOT NULL DEFAULT 0,   -- 当前在跑，随启停增减
    -- 渠道分布（投影，可由 turn_metrics 重放重建）
    channel_mix_json      TEXT NOT NULL DEFAULT '{}',
    -- 时间线
    first_turn_at_ms      INTEGER,
    last_turn_at_ms       INTEGER,
    updated_at_ms         INTEGER NOT NULL
);
```

设计要点：

- **它是投影，不是事实源。** 全部字段可由 `turn_metrics` 与 `job_*` 事件重放得到。损坏或缺失时从事件日志重建，重建成本与一次会话重放相同。
- **累计耗时三个维度分开**：`total_active_ms`（turn 墙钟）、`total_provider_ms`（模型往返）、`total_tool_ms`（工具执行）。三者不是父子关系，工具耗时可能与 provider 重叠，所以分开记，不做减法推导。
- **分位数用固定直方图**，不保留样本。`ttft_histogram_json` 与 `turn_histogram_json` 各 16 个对数桶（1/2/5/10/20/50/100/200/500ms/1s/2s/5s/10s/30s/60s/∞），每轮 O(1) 加一，读时按桶插值算 p50/p95。表宽固定，不随轮数增长。
- **`ttft_samples` 只统计测得值**。未测得的轮次既不加 sum 也不加 samples，平均值不会被 0 稀释。这与 `usage_facts` 里"未测得首字必须落 NULL 而不是 0"的口径一致。
- **`jobs_running` 是唯一会减少的计数**，其余都是单调累加。它在 `JobStarted` 加一、`JobCompleted` 减一，任何时刻等于该会话在途作业数。
- **`sessions` 表只加一个冗余列** `open_job_count`，供回收逻辑在不 JOIN 的情况下判断会话是否还有在跑作业；明细一律读 `session_metrics`。

列表页读路径：`sessions JOIN session_metrics USING(session_id)`，一次索引查找，不扫事件、不扫 usage。桌面端要"这个会话跑了 47 轮、累计 12 分 30 秒、p95 首字 1.8 秒、3 个后台任务"这类信息时走这条路径。

**为什么不全放 `usage.sqlite3`**

- usage 是跨会话计费口径，且它的源是 `rollout.sqlite3`，本次要删。
- 会话级事实应当随会话走，删会话一起删，不留孤儿。
- 桌面端要看的是"这个会话"，不是"这个月"。

### 4.12 会话身份（一）：Lane 一词的废除

现状是三个词指两个实体，其中一个词纯属历史包袱。

**证据链**

1. `crates/axon-session/src/application/session_lane.rs:24-38`：`session_lane_id` 的返回值是 `"session-lane-" + sha256([workspace_id, trust_scope, channel, destination])`。
2. 同文件 `:42-49`：`interaction_session_lane_id` 直接从 Interaction 派生这个值。
3. `crates/axon-cli/src/workbench_http_lane_readings.rs:248`：用 lane 去查 `session_runtimes.session_id`。
4. `crates/axon-cli/src/run_commands/run_question_projection.rs:240` 的注释直说："Daemon 主 turn 的车道就是它的运行会话身份（`primaryLaneId`）"。
5. 本机删除前的实测数据里，`session_events.session_id` 的取值就是 `session-lane-9107a38c6c05df9fd8c6b47d7ac...` 这种格式。

结论：**`lane_id` 恒等于 `session_id`，两个名字指同一列值。** Lane 不是实体，是 Session 的复合主键构造方式。

**为什么会有两个名字**

`session_lane.rs` 的模块注释交代了设计意图："Axon 对外只有持续存在的同一个 JARVIS，Session 只是内部运行窗口。"也就是说他们想区分"对外持续身份"与"内部运行窗口"，但实现上把两者编成了同一个 ID，于是名字留下了、区分没留下。

**收敛方案**

1. **`lane` 这个词从代码与文档中消失**，统一叫 `session`。
2. `interactions.lane_id` 改名 `session_id`，外键直指 `control.sessions.session_id`。
3. 真正的两级区分是 **稳定身份** 与 **运行实例**，用两列表达，不新增表：

```sql
CREATE TABLE sessions (
    session_id        TEXT PRIMARY KEY,   -- ULID，客户端可引用；不再由哈希派生
    workspace_id      TEXT NOT NULL,
    trust_scope       TEXT NOT NULL,      -- 安全隔离维度：private / group-workspace / restricted
    title             TEXT NOT NULL,
    pinned            INTEGER NOT NULL,
    archived          INTEGER NOT NULL,
    sort_order        INTEGER NOT NULL,
    current_run_id    TEXT NOT NULL,      -- 当前运行实例；/new 重置时换新值
    run_generation    INTEGER NOT NULL,   -- 递增，替代原来的 run_session_id 语义
    root_session_id   TEXT NOT NULL,      -- 子代理的根会话
    cwd               TEXT NOT NULL,
    status            TEXT NOT NULL,
    owner_instance_id TEXT,
    owner_generation  TEXT,
    worker_attempt    INTEGER NOT NULL,
    context_version   INTEGER NOT NULL,
    last_checkpoint_seq INTEGER NOT NULL,
    last_event_sequence INTEGER NOT NULL,
    last_activity_at_ms INTEGER NOT NULL,
    created_at_ms     INTEGER NOT NULL,
    updated_at_ms     INTEGER NOT NULL
);
```

**`channel` 与 `destination` 不在这个表里。** 它们是 Interaction 的来源与回复路由元数据，不参与会话身份。理由与做法见 §4.13。

`root_session_id` 只对子会话有意义，`title` / `pinned` / `archived` / `sort_order` 承接桌面端已有的车道管理能力（`lane_create` / `lane_rename` / `lane_set_pinned` / `lane_set_archived` / `lane_reorder`）。

把 `workspace_id` / `trust_scope` / `channel` / `destination` 落成实列，收益是可以直接按渠道或工作区查会话，不必反解哈希。原来的 `run_session_id` 与 `root_session_id` 两列语义重叠，改为 `current_run_id`（可变）+ `run_generation`（单调），`root_session_id` 只对子会话有意义。

4. `primaryLaneId` 这个线上字段名保留（前端与网关契约不动），但服务端内部一律叫 `session_id`，并在文档里写明两者同值。

**收敛后的模型**

| 概念 | 实体 | 说明 |
| --- | --- | --- |
| Session | `control.sessions` | 稳定对话身份：一个 Workspace 的一个渠道表面。等价于原来的 Lane |
| Run | `sessions.current_run_id` + `run_generation` | 一次运行实例，`/new` 重置时递增 |
| Interaction | `control.inbox` | 一条入站消息，幂等，属于某个 Session |
| Turn / Execution | `control.work_items` / `work_item_history` | 一次执行，属于某个 Interaction，可重试 |

三个词（Lane / Interaction / Session）变成两个实体（Session / Interaction）加一个运行实例标记。这符合 OpenAI Assistants 的 thread/run 与 Anthropic 的 conversation 两级模型。

**顺带消掉的东西**

- `session_lane.rs` 整个模块可以删除，哈希构造下沉到 `sessions` 写入路径的一次计算。
- `lane_id`、`session_lane_id`、`primaryLaneId` 三套命名在代码里并存，收敛为一套。
- `session_metrics.session_id` 直接指向稳定身份，不需要区分"lane 维度"与"session 维度"两套统计。

### 4.13 会话身份（二）：channel 不参与，多端同会话

目标：不同 channel 看到同一份数据；多端请求同一会话时渲染同步。

**现状为什么不成立**

会话身份是 `sha256(workspace, trust_scope, channel, destination)`（`session_lane.rs:24-38`），channel 和 destination 都是哈希输入：

- 桌面端用固定常量 `DESKTOP_LANE_CHANNEL` 派生自己的车道（`crates/axon-desktop/src/workspace/mutation.rs:167`）。
- TUI 走 `tui` channel（`session_lane.rs:69` 的测试就是这个组合）。
- 飞书每个 destination（群或私聊）再各一份。

结果：TUI 与桌面永远是两个 session、两条事件流，两边谁改都看不见对方。飞书加进来是第三份。

**新模型**

会话是**显式实体**，不由接入方式派生：

| 维度 | 旧 | 新 |
| --- | --- | --- |
| 身份 | `sha256(workspace, trust_scope, channel, destination)` | `session_id`（ULID）+ `workspace_id` + `trust_scope` |
| channel | 身份的一部分 | Interaction 的来源元数据，只用于回发 |
| destination | 身份的一部分 | Interaction 的回复路由，只用于回发 |
| 创建 | 首次收到该车道消息时隐式建 | 显式创建，或解析到该 Workspace 的活跃会话 |
| 多端 | 每个 channel 一个会话 | 一个会话被多个连接订阅 |

**多端同步机制**

1. 服务端只有一个事件流事实源。所有端订阅同一个 `session_id`，按各自 cursor 重放并接收实时广播。
2. 每个 `SubscribeEvents` 连接独立 cursor，不共享游标，不依赖端侧状态同步。
3. 同一会话的写入串行（现状的车道排他等价于会话排他）。两个端同时发消息时，先到者进入 processing，后到者进 `received` 队列，广播按序推进。
4. 端侧渲染完全由事件驱动：任何一端收到事件就更新，不做端到端对账。

**四个必须处理的点**

1. **`trust_scope` 不能丢。** 它是防"群聊看到私聊上下文"的唯一隔离维度（`session_lane.rs:52-58`）。保留为会话创建时的属性，不再作为哈希输入。
2. **客户端必须显式携带 `session_id`，服务端不做推断。** 桌面左侧菜单栏就是 `sessions` 列表（现为车道列表），选中即切换；TUI 用 `--session <id>` 指定或启动时从列表选择。服务端收到不带 `session_id` 的请求直接拒绝，不猜、不隐式建会话。
3. **无 UI 的入站渠道靠绑定表。** 飞书群、外部 webhook 这类没有"菜单"的入口，用 `control.channel_bindings` 把 `(channel, destination)` 映射到一个 `session_id`：首次绑定时创建或选择一个会话，之后该群的消息固定进这个会话。这张表只服务入站路由，不参与会话身份。
4. **协议字段改名。** `primaryLaneId`（`interaction_turn_payloads.rs:163`）、`laneId`（`crates/axon-protocol/src/daemon.rs:88`、`user_question.rs:158`）统一改 `sessionId`。这是破坏性协议变更，`crates/axon-desktop/ui` 的 `frames.ts` 与 store 必须同批改。
5. **会话管理的落点。** 桌面端已有 `lane_create` / `lane_rename` / `lane_set_pinned` / `lane_set_archived` / `lane_reorder`（`crates/axon-desktop/src/workspace/mod.rs:61-100`），语义直接迁到 `sessions` 表的 `title` / `pinned` / `archived` / `sort_order` 四列。左侧菜单读的就是 `(workspace_id, archived, sort_order)` 索引。

**影响面：约 250 处 `lane_id`**

| 层 | 代表位置 | 改动 |
| --- | --- | --- |
| 领域 | `crates/axon-session/src/application/session_lane.rs` | 整个模块删除 |
| 会话 | `interaction_turn_runner.rs:176、:366`、`primary_turn_lanes.rs:19` | 改为读 `interaction.session_id` |
| 网关 | `interaction_turn_payloads.rs:163`（`primaryLaneId`） | 改 `sessionId` |
| 桌面 | `workspace/mutation.rs:167`（`derive_lane_id`） | 改为从会话列表选择 |
| CLI HTTP | `workbench_http_*` 全套 `lane_*` 函数与路由 | 改名 `session_*` |
| TUI | `bottom_view.rs:39`、`state.rs:265` | 改名 |
| 协议 | `crates/axon-protocol/src/daemon.rs:88`、`user_question.rs:158` | 改名 |

这条改动与存储重构是同一批：两者都在改会话身份与归属，分开做会返工两次。

### 4.14 工作项模型：Interaction 与 Execution 的重划

两个都保留，但要重新分工：**Interaction 降为入站事实，Execution 升为统一工作项。** 理由与做法如下。

**现状：四个概念、四套领取**

| 概念 | 表 | 领取入口 |
| --- | --- | --- |
| Host 实例 | `daemon_hosts` | `claim_host`（`runtime_repository.rs:99`）、`claim_upgrade_replacement`（`:279`） |
| Interaction | `jarvis_interactions` | `claim_processing`（`interaction_repository.rs:285`）、`claim_processing_by_id`（`:343`） |
| Action | `jarvis_executions` | `claim_next_action`（`action_repository.rs:13`） |
| Execution | 同一张 `jarvis_executions` | `claim_executions`（`execution_repository.rs:162`） |
| Schedule | `runtime_schedules` | `claim_due_schedules`（`schedule_repository.rs:42`） |

其中 Action 与 Execution 是同一张表的两种查法，`SessionActionRecord` 就是 `ExecutionRecord` 的别名（`ports/session_action_store.rs` 只有 5 行 re-export）。也就是说"领取"这件事在当前代码里有**三套并行实现**：按 lane 领 Interaction、按 session 领 Action、按 owner 领 Execution。

**Interaction 该降级**

它现在背负三件不相干的事：

1. 入站事实：来源、幂等键、内容、附件、受众、回复路由（应当只读追加）。
2. 处理状态机：`status`、`lease_token`、`lease_expires_at_ms`、`tool_summary`（每轮被 UPDATE 多次）。
3. 展示元数据：`model`、`reasoning_effort`、`tool_summary`（供车道列表读）。

第 2 类是写放大的来源：一条消息从 `received` 到终态要经历多次 UPDATE，每次都进 WAL。而这些 UPDATE 与事件写入抢同一把锁。

**新的四层**

```text
inbox        入站事实：幂等键、内容、附件、受众、回复路由、session 归属。append-only
work_items   统一工作项：turn / goal / cron / subagent。在途行，一套 claim 与 lease
outbox       出站投递：agent 消息、continuation、完成通知、用户提问
ledger       跨会话账本
```

`work_items` 的 `kind` 扩展后，原来三条领取路径收敛为一条：

```sql
CREATE TABLE work_items (
    work_id        TEXT PRIMARY KEY,
    kind           TEXT NOT NULL,          -- turn | goal | cron | subagent
    session_id     TEXT NOT NULL,
    inbox_id       TEXT,                   -- turn 类型必填，其余为空
    goal_id        TEXT,
    priority       INTEGER NOT NULL,
    status         TEXT NOT NULL,          -- queued | running | waiting | terminal
    attempt        INTEGER NOT NULL,
    dedupe_key     TEXT,
    payload_json   TEXT NOT NULL,
    claimed_by     TEXT,
    claimed_generation TEXT,
    lease_expires_at_ms INTEGER,
    available_at_ms INTEGER NOT NULL,
    cancellation_requested_at_ms INTEGER,
    created_at_ms  INTEGER NOT NULL,
    started_at_ms  INTEGER,
    finished_at_ms INTEGER
);
```

`inbox` 只剩事实：

```sql
CREATE TABLE inbox (
    inbox_id          TEXT PRIMARY KEY,
    channel           TEXT NOT NULL,
    source_message_id TEXT NOT NULL,
    session_id        TEXT NOT NULL,
    content           TEXT NOT NULL,
    attachments_json  TEXT NOT NULL,
    scope_json        TEXT NOT NULL,
    audience_json     TEXT NOT NULL,
    reply_route_json  TEXT NOT NULL,
    model             TEXT,
    reasoning_effort  TEXT,
    received_at_ms    INTEGER NOT NULL,
    UNIQUE(channel, source_message_id)
);
```

**收益**

1. 一套 claim / lease / 重试 / 死信，替代三套并行实现。
2. `inbox` 变成纯追加，写完不再更新；写放大只落在 `work_items` 的在途行上（行数等于在途数）。
3. 状态机只有一处，`waiting`（等用户作答）、`queued`（等资源）、`running` 是同一张表的不同状态，不再分散到 `jarvis_user_questions` 与 `jarvis_agent_messages`。
4. 车道排他与目标幂等仍然是同一个 `dedupe_key` 唯一索引，语义不丢。

**代价**

1. 桌面端车道列表要 `inbox JOIN work_items` 才能拿到"这条消息在跑还是完了"。现在是一次单表查询，之后是两表 JOIN。索引都在主键上，代价可接受。
2. `waiting` 状态要能表达"等用户回答"，`jarvis_user_questions` 从独立表变成 `work_items` 的一种状态 + outbox 里的一条待投递记录。这让"问答"复用同一套超时与恢复逻辑。

**结论**

Interaction 不消失，但它不再是"会一直变的状态机"，而是"一条不可变的入站事实"。Execution 不退场，而是升级成承载全部工作类型的 `work_items`。四个概念收敛为两个实体，表数量不变（`interactions` → `inbox`，`execution_queue` + `execution_history` → `work_items` + `work_item_history`），但**领取逻辑从三套变一套**。

### 4.15 三端的会话选择：TUI、CLI、桌面

现状：TUI 的斜杠命令集中定义在 `crates/axon-tui-runtime/src/loop/agent_tui/commands.rs`，已有 `/model`、`/clear`、`/think` 等；弹窗骨架在 `widgets/command_palette.rs`，`/model` 的列表弹窗是现成参考。TUI **没有**会话列表能力，会话只能靠启动参数或"最近一条"决定。

目标：让 TUI 与桌面端一样以 `session_id` 为中心工作。

**命令面**

| 命令 | 语义 |
| --- | --- |
| `/sessions` | 弹窗列出当前 Workspace 的会话，上下键选择、Enter 进入 |
| `/sessions --all` | 包含已归档会话 |
| `/new` | **新建**一个 session（重构前是重置 run） |
| `/reset` | 重置当前 session 的运行实例（`current_run_id` 递增，会话与历史不变） |

`/new` 与 `/reset` 必须分开：前者换 `session_id`，后者只换 `run_generation`。现在两者混在 `/new` 一个命令里，重构后拆开。

**数据来源（新增接口）**

```http
GET /api/v1/sessions?workspace=<workspace_id>&include_archived=false
→ [{
     sessionId, title, status, pinned, archived,
     lastActivityAtMs, turnCount, totalActiveMs,
     lastTtftMs, p50TtftMs, openJobCount
   }]
```

字段直接来自 `sessions JOIN session_metrics`（§4.11 的投影），一次索引查找，不扫事件、不扫 usage。排序为 `pinned` 优先、再按 `last_activity_at_ms` 倒序。

**切换语义**

1. 选中后以新 `session_id` 重建订阅：`SubscribeEvents` 的 cursor 从 0 或最近 `checkpoint` 起。
2. 清空当前视图，按事件重放渲染，与冷启动加载同一路径，不新增渲染分支。
3. 切换不写任何状态：当前 `session_id` 只存在于 TUI 进程内，服务端无"活跃会话"概念（§11 裁决 1）。

**实现落点**

| 位置 | 改动 |
| --- | --- |
| `agent_tui/commands.rs` | 新增 `COMMAND_SESSIONS` / `COMMAND_RESET`，把 `/new` 的语义改为新建 |
| `agent_tui/state/command_handlers.rs` | 新增 `/sessions` 分支，打开会话列表弹窗 |
| `widgets/command_palette.rs` | 复用骨架，新增会话列表模型（标题、时间、状态） |
| `agent_tui/state/suggestions.rs` | 补 `/sessions`、`/reset` 的补全 |
| `workbench_http_adapter` | 新增 `GET /api/v1/sessions` |

**CLI**

现状：`axon debug session` 已经有一整套会话命令（`crates/axon-cli/src/session_commands/mod.rs:46-67`：list / show / read / resume / fork / delete / export / replay / output / doctor），其中 `resume` 的语义正是"恢复 Thread，让后续 run 继续追加到同一会话"。但它藏在 `debug` 层级下，而且要先 resume 再 run，两步。

目标：

1. `axon run "…" --session <session_id>` 一步指定会话继续对话，不必先 resume。优先级高于任何已保存的 resume 状态。
2. `axon debug session` 提升为顶层 `axon sessions`，去掉 `debug` 层级。
3. `--session` 是单次调用覆盖，`sessions resume` 是设置后续默认，两者都**不创建**会话。指定不存在的 `session_id` 直接报错，不隐式新建（§11 裁决 1）。
4. 非交互 `run` 同样遵守显式会话原则：不带 `--session` 且没有 resume 状态时，报错并提示先 `axon sessions list`。

```bash
axon run "把测试跑一遍" --session <session_id>     # 单次指定，继续该会话
axon sessions list --limit 20                      # 列出会话（含轮次与耗时）
axon sessions resume <session_id>                  # 设为后续 run 的默认会话
axon sessions show <session_id>                    # 查看单个会话摘要
```

落点：

| 位置 | 改动 |
| --- | --- |
| `crates/axon-cli/src/run_commands/*`（run 参数定义） | 新增 `--session`，解析后作为 `session_id` 传入上下文 |
| `crates/axon-cli/src/session_commands/mod.rs:46-67` | 子命令组从 `debug session` 提升为顶层 `sessions` |
| `crates/axon-cli/src/session_commands/thread_cli.rs` | `resume` 保持"写默认会话"语义，`--session` 不落盘 |

**桌面**

左侧菜单点击即切换，选中态即当前 `session_id`。菜单数据与 `/sessions` 同一接口（`GET /api/v1/sessions`），排序、置顶、归档语义一致。

## 5. 影响面与改造清单

这一节是重构的执行清单。每条都给出当前代码位置、需要改成什么、以及编译器能否帮你发现问题。

### 5.0 总览

| 层 | 涉及位置 | 编译器能否发现 | 难度 |
| --- | --- | --- | --- |
| 端口层 | `ports/runtime_store.rs` 32 个方法 | 能（trait 签名变化） | 中 |
| 写入侧生产调用点 | 8 处 | 能 | 高（`commit_action` 需拆事务） |
| 读取侧生产调用点 | 9 处 | 能 | 中（读契约必须逐条保持） |
| 直连 SQL | 约 35 处 | **不能**（字符串里的表名） | 最高 |
| 归档与清理 | `runtime_event_archive.rs` 全文件 + 3 个 cleanup 模块 | 不能 | 高（整段逻辑作废） |
| 诊断与 VACUUM | 6 处 | 部分 | 低 |
| 桌面端 | 0 处（架构门禁禁止直连） | 不适用 | 低（只需契约确认） |
| 测试 fixture | 约 40 个文件 | 能 | 低（统一收敛） |
| schema 元数据 | `schema.rs` | 能 | 低 |

### 5.1 端口层：`RuntimeStore` 拆分

现状（`crates/axon-session/src/ports/runtime_store.rs:55-73`）：一个 trait 聚合了 15 个 sub-store trait 的 32 个方法，事件存储与状态机混在一起。

目标：拆成两个端口。

- **`EventLogStore`**：只负责追加与读取事件流。
- **`ControlStore`**：保留租约、队列、状态机、Goal、调度、决策、工作区、健康。

方法归属逐条如下：

| 方法 | 现行 | 目标归属 | 改造要点 |
| --- | --- | --- | --- |
| `initialize` | 75 | 两者各一份 | 控制面建库 + 事件面建目录 |
| `save_goal_result_document` | 77 | ControlStore | 不变 |
| `claim_host` | 84 | ControlStore | 不变 |
| `renew_host` | 93 | ControlStore | 不变 |
| `request_upgrade` | 100 | ControlStore | 不变 |
| `mark_handoff_ready` | 109 | ControlStore | 不变 |
| `interrupt_host_actions` | 118 | ControlStore + EventLog | 中断事件写事件面 |
| `claim_upgrade_replacement` | 124 | ControlStore | 不变 |
| `create_session` | 134 | ControlStore | 不变 |
| `stop_session` | 141 | ControlStore + EventLog | 终态事件 |
| `reset_session` | 148 | ControlStore + EventLog | 新 rollout 身份 |
| `get_session` | 156 | ControlStore | **保留 `last_event_sequence`**，它是 durable 尾的定义 |
| `recover_expired_sessions` | 161 | ControlStore | 不变 |
| `set_session_status` | 168 | ControlStore | 不变 |
| `enqueue_action` | 176 | ControlStore | 不变 |
| `claim_next_action` | 182 | ControlStore | 不变 |
| `renew_action_lease` | 199 | ControlStore | 不变 |
| `list_pending_actions` | 208 | ControlStore | 不变 |
| `cancel_action` | 214 | ControlStore | 不变 |
| `list_runnable_execution_sessions` | 221 | ControlStore | 不变 |
| `update_action_status` | 227 | ControlStore | 不变 |
| `fail_action` | 236 | ControlStore + EventLog | 拆：先控制面置终态，再追加失败事件 |
| `quarantine_interrupted_executions` | 248 | ControlStore + EventLog | 拆：同上 |
| `quarantine_interrupted_interactions` | 255 | ControlStore + EventLog | 拆：同上 |
| `commit_action` | 262 | ControlStore + EventLog | **最关键**，见 §5.2 |
| `append_event` | 274 | EventLog | 迁，单条即单批 |
| `append_events` | 283 | EventLog | 迁，一次 frame |
| `append_interaction_events` | 291 | EventLog + ControlStore | 事件面写入前先向控制面校验 interaction lease |
| `replay_events` | 300 | EventLog | 迁，契约见 §5.3 |
| `interaction_tail_window` | 310 | EventLog | 迁，反向扫描 |
| `upsert_schedule` | 317 | ControlStore | 不变 |
| `claim_due_schedules` | 319 | ControlStore | 不变 |
| `settle_schedule` | 327 | ControlStore | 不变 |

统计：32 个方法中 5 个整体迁到 `EventLogStore`，4 个拆成两段，23 个留在 `ControlStore`。

### 5.2 `commit_action` 的拆分（唯一必须改事务语义的地方）

现状（`sqlite_runtime_store/commit_repository.rs:43-223`）在一个 `IMMEDIATE` 事务里完成：

1. `ensure_fence`（`:58`）
2. 结算账本 `commit_in_transaction`（`:72-87`），内含 execution / result / goal / `jarvis_events`
3. 读 `session_runtimes.last_event_sequence`（`:89-97`）
4. 逐条 `INSERT INTO session_events`（`:101-118`）
5. `INSERT INTO session_checkpoints`（`:194`）
6. 更新 `session_runtimes` 水位（提交尾部）

这是「结算」与「叙述」必须原子的唯一位置。按 §4.9 已确认的"先 fence 后写文件"，拆成两段：

```text
阶段 1（控制面事务，毫秒级）
  fence 校验
  结算账本：work_item 终态 + result + goal revision + ledger event
  推进 sessions.last_event_sequence

阶段 2（事件面追加，无库锁）
  按阶段 1 分配的起始 sequence 追加事件 frame 并 fsync
```

崩溃恢复（启动时执行，不引入 pending 表）：

- **水位大于日志尾**：事件未写或写残。把水位回退到日志实际尾序号，缺失的终态从结算记录（`goal_results.summary`）重建为一条 `session_recovered` 事件，**追加到当前尾部**而不是填补空洞，避免破坏 sequence 单调性。
- **日志尾部有 torn frame**：截断到上一完整 frame 边界，再回退水位。
- **水位小于日志尾**：正常顺序下不应出现。若出现说明有外部写入，以日志为准重建水位并记 `session_recovered`。

选择"控制面先、事件面后"的理由：结算是权威事实，事件是叙述。反过来（事件先、结算后）会导致崩溃时出现"有事件但没结算"的幽灵回合，比"有结算但少几条叙述"更难修复。

### 5.3 写入侧生产调用点（8 处）

| 调用点 | 行 | 现在写什么 | 改造 |
| --- | --- | --- | --- |
| `axon-daemon/src/application/interaction_turn_runner.rs` | 301 | `append_interaction_events` | 改调 `EventLogStore`，lease 校验保留在控制面 |
| `axon-daemon/src/application/interaction_turn_event_sink.rs` | 364、395、471 | `append_interaction_events`（含失败重试路径） | 同上；395 行是重试分支，需要保证批次幂等 |
| `axon-daemon/src/application/execution_session_sink.rs` | 72 | `append_events` | 同上 |
| `axon-daemon/src/application/child_session_event_sink.rs` | 69 | `append_events` | 同上 |
| `axon-session-runtime/src/application/execution_worker_settlement.rs` | 185 | `append_event` | 同上 |
| `axon-session-runtime/src/application/execution_worker_settlement.rs` | 229 | `commit_action` | 按 §5.2 三段式 |
| `axon-session-runtime/src/application/execution_worker_outcome.rs` | 42 | `commit_action` | 按 §5.2 三段式 |
| `axon-cli/src/daemon_host_runner_question_durable.rs` | 39 | `append_events` | 改调 `EventLogStore` |
| `axon-cli/src/run_commands/run_rollout_store/api.rs` | 239 | rollout `append_event` | 删除 `rollout.sqlite3`，turn 记录改由 `EventLogStore` 写入会话事件流 |
| `TurnPerformanceRecorder` 采集点 | `crates/axon-cli/src/lib.rs:472`、`run_request_payload.rs:151-260`、`run_provider_loop/mod.rs:176-413` | 只在 `log_level = trace` 时采集阶段 | 改为无条件采集；收口时写一条 `turn_metrics`（见 §4.11） |
| `JobRuntime` 生命周期 | `crates/axon-agent-runtime/src/job_runtime`（`JobId` / `JobStatus`） | 纯进程内全局态，重启即丢 | 启停两处写 `job_started` / `job_completed` 事件，并维护 `control.sessions.open_job_count` |

### 5.4 读取侧生产调用点（9 处）

| 调用点 | 行 | 方法 | 改造 |
| --- | --- | --- | --- |
| `axon-daemon/src/infrastructure/gateway_replay.rs` | 176 | `replay_events`（会话级分页） | 改读事件面 |
| 同上 | 268 | `replay_events`（交互级分页） | 同上 |
| 同上 | 382 | `interaction_tail_window` | 改读事件面尾部 |
| 同上 | 499 | `replay_events`（默认页大小全量） | 同上 |
| `axon-daemon/src/infrastructure/gateway_bridge.rs` | 225 | `replay_events` | 同上 |
| `axon-daemon/src/application/daemon_host.rs` | 585 | `replay_events` | 同上 |
| `axon-cli/src/run_commands/daemon_history_loader.rs` | 158 | `replay_events` 分页循环 | 同上 |
| `axon-session/src/infrastructure/sqlite_runtime_store/event_repository.rs` | 29 | 端口默认实现 | 迁入 `EventLogStore` 实现 |
| `axon-cli/src/workbench_http_lane_readings.rs` | 100 | 直连反向扫 `turn_completed` | 见 §5.5 |

**读契约（新实现必须逐条满足，写进契约测试）**

1. 按 `sequence > after` 升序分页，页大小由调用方给（`replay_page_size`）。
2. `get_session().last_event_sequence` 是 durable 尾，也是重放的封闭上界（`gateway_replay.rs:223-240`）。
3. 单次重放有字节预算 `MAX_GATEWAY_REPLAY_BYTES` 与页数上限 `MAX_GATEWAY_REPLAY_PAGES`，触顶必须留审计日志而不是静默截断（`gateway_replay.rs:202-215`）。
4. 外置正文缺失时必须返回 `SESSION_EVENT_MISSING_ARCHIVE_TYPE` 占位，不能丢弃该事件（`gateway_replay.rs:185-189`）。
5. 反向查询能力：桌面端每页正文靠 `ORDER BY sequence DESC` 从新往旧扫，凑齐即停（`workbench_http_lane_readings.rs:100-109`）。
6. 按 `interaction_id` 定向取尾窗（`interaction_tail_window`）。

### 5.5 直连 SQL（编译器不报错，最高危）

这些位置直接对 `runtime.sqlite3` 拼 SQL，不走 trait。改动时必须逐个手工核对，否则会出现"编译通过、运行时表不存在"。

**`session_events`（全部要重写）**

| 位置 | 用途 |
| --- | --- |
| `axon-session/.../commit_repository.rs:108` | `commit_action` 事务内 INSERT |
| `axon-session/.../event_repository.rs:153` | 主写入 INSERT |
| `axon-session/.../event_repository.rs:340、378` | 尾窗查询 |
| `axon-session/.../failure_repository.rs:67` | 失败终态 INSERT |
| `axon-session/.../interrupted_interaction_repository.rs:203、288` | 隔离 INSERT 与查询 |
| `axon-session/.../interrupted_execution_repository.rs:269` | 隔离 INSERT |
| `axon-session/.../runtime_repository.rs:386` | quarantine INSERT |
| `axon-cli/src/workbench_http_lane_readings.rs:100` | 桌面端每页正文反向扫描 |
| `axon-cli/src/cron_commands/runtime_cleanup.rs:383、477` | 过期计数与筛选 |
| `axon-cli/src/cron_commands/runtime_cleanup_session_events.rs:152` | 悬空引用修复扫描 |
| `axon-cli/src/cron_commands/runtime_event_archive.rs:113、295、321、406、472、495` | 归档、最小序号、`DELETE ... RETURNING`、回写 |

**`jarvis_events`（账本，带幂等唯一索引，不能简单并入事件流）**

| 位置 | 用途 |
| --- | --- |
| `axon-session/.../ledger_event_repository.rs:22、45、59、87、115` | 幂等查、追加、按序号重放、按 goal 查 |
| `axon-session/.../ledger_commit_repository.rs:154、168` | 结算事务内追加 |
| `axon-session/.../goal_repository.rs:168、178、218、224、369、376` | `goal.cancelled` 事件 |
| `axon-session/.../goal_commit_repository.rs:289、342、349` | 幂等查与追加 |
| `axon-session/.../interrupted_execution_repository.rs:229`、`interrupted_interaction_repository.rs:157` | 隔离事件 |

结论：`jarvis_events` **留在控制面**，它是带唯一键与单调序号的账本，与 `session_events` 是两种东西。

**`session_checkpoints`**：只有写（`commit_repository.rs:194`），没有生产读。可以直接降级为事件流里的一种事件类型，表删除。

**控制面（保留，但需要跟新库名走）**

| 位置 | 用途 | 改造 |
| --- | --- | --- |
| `axon-cli/src/system_http_adapter.rs:206` | `daemon_hosts` lease 只读查询 | 换成控制面路径 |
| `axon-cli/src/workbench_http_lane_readings.rs:50、169、248` | `jarvis_interactions` / `session_runtimes` | 同上 |
| `axon-cli/src/cron_commands/runtime_cleanup_counts.rs:33、62、83` | 保留计数 | 同上 |
| `axon-cli/src/cron_commands/runtime_cleanup_families.rs:275、283、494、517、528` | 删除候选 | 同上 |
| `axon-cli/src/cron_commands/runtime_cleanup.rs:287、303、461、489、626、630` | 保留判定 | 同上 |

### 5.6 归档与清理（整段逻辑作废重写）

`axon-cli/src/cron_commands/runtime_event_archive.rs`（520 行）完全围绕 `session_events` 表构建：

| 函数 | 现行 | 重构后 |
| --- | --- | --- |
| `event_archive_root` | 59 | 删除，事件面本身就是文件 |
| `archive_expired_session_events` | 65 | 改为"按会话整目录归档或删除" |
| `enforce_disk_budget` | 100 | 改为"按会话目录总字节淘汰最旧会话" |
| `read_archive_batch` | 153 | 删除 |
| `write_archives` | 179 | 删除 |
| `resolve_payload` | 219 | 删除 |
| `reclaim_external_payload_files` / `reclaim_dead_payload_shards` | 233、286 | 删除 |
| `delete_rows_by_rowid` | 313 | 删除 |
| `database_footprint` | 340 | 改为目录体量统计 |

连带影响：

- `runtime_cleanup.rs:166-201`（归档与悬空正文修复流程）整体重写。
- `runtime_cleanup_session_events.rs` 整个模块删除。
- `session_family_cleanup.rs:343`（`vacuumed_rollout`）随 rollout 迁走。
- 保留期语义从"按事件时间删行"变为"按会话目录删除"，粒度变粗但代价模型简单得多。

### 5.7 诊断、导出、健康、VACUUM

| 位置 | 现状 | 改造 |
| --- | --- | --- |
| `axon-cli/src/system_http_adapter.rs:191-222` | 读 `daemon_hosts` 报 lease | 控制面路径，语义不变 |
| `axon-session/.../health_repository.rs:18` | 读交互计数 | 控制面，不变 |
| `axon-cli/src/commands/export.rs` | 导出 thread 详情 | 走 ThreadService（JSONL），不受影响 |
| `axon-cli/src/cron_commands/session_cleanup_sqlite.rs:16` | 遍历 `data/**/*.sqlite3` 做 VACUUM | 只剩控制面一个候选，可直接停用 |
| `axon-cli/src/cron_commands/runtime_cleanup.rs:664` | `incremental_vacuum(4096)` | 移到控制面维护 |
| `axon-cli/src/interactive_tui/doctor_commands.rs:575` | 检查 data 目录结构 | 更新预期目录清单 |

### 5.8 桌面端

`crates/axon-desktop` 不直连数据库（`tools/verify-architecture.ps1` 的依赖门禁强制）。数据全经 gateway 与 HTTP adapter，因此 **Rust 侧改动集中，前端代码不需要动**。

但有两处契约需要前端确认：

1. `turn_completed` 载荷里的 `response` 仍是每页正文的唯一来源（`workbench_http_lane_readings.rs:76-85` 的注释明确写了这条依赖），新事件面必须保留该字段。
2. 非语义帧（`LivePart` / `LiveOnly`）已经不进 durable 层，桌面端当前靠内存广播补帧。重构后事件面只含终态与边界快照，需要确认 `crates/axon-desktop/ui/src/frames.ts` 在断线重连时对"只有终态、没有历史增量"的回放能正确渲染，并确认补帧窗口足够覆盖常见网络抖动。

### 5.9 测试与 fixture

约 40 个测试文件直接 `SqliteRuntimeStore::new(dir.join("runtime.sqlite3"))`：

| crate | 文件数 | 代表 |
| --- | --- | --- |
| `axon-session` | 18 | `sqlite_runtime_store_session_tests.rs`、`sqlite_runtime_store_event_batch_tests.rs` |
| `axon-daemon` | 12 | `daemon_host_*_tests.rs` |
| `axon-session-runtime` | 7 | `execution_worker_*_tests.rs` |
| `axon-cli` | 4 | `runtime_cleanup_*_tests.rs`、`daemon_host_runner_tests.rs` |

改造：新增 `axon-test-support` 里的 `RuntimeFixture`，一次建好"控制面 + 事件面目录"，替换全部直接构造。**不要逐文件改**，否则 40 处会各写各的。

### 5.10 schema 元数据

| 位置 | 现状 | 改造 |
| --- | --- | --- |
| `schema.rs:9` | `SCHEMA_IDENTITY = "jarvis-runtime-goal-v1"` | 换新 identity（旧库直接 `InvalidQuery` 拒绝） |
| `schema.rs:13-22` | `INITIALIZED_DATABASE_PATHS`、`SCHEMA_INIT_LOCK` | 控制面沿用；事件面改为目录初始化 |
| `schema.rs:529-585` | `migrate_additive_columns` | 整段删除（不做迁移） |
| `schema.rs:107-499` | 22 张表 DDL | 删 6 张（`session_events`、`session_checkpoints`、`jarvis_completion_notifications`、`jarvis_completion_deliveries`、`jarvis_client_presence`、`daemon_upgrade_admissions` 视实现决定保留与否） |

### 5.11 改造依赖顺序（按编译器能帮你发现问题的先后）

1. 新建 `crates/axon-eventlog`（无业务依赖，纯基座）。
2. 拆 `RuntimeStore` trait 为 `EventLogStore` + `ControlStore`。**这一步会让 32 个方法归属一次性暴露**。
3. 实现事件面端口（含契约测试）。
4. 改写入侧 8 处。
5. 改读取侧 9 处。
6. 改直连 SQL 约 35 处（**编译器帮不上**，必须逐个过）。
7. 重写归档与清理。
8. 收敛测试 fixture。
9. 切 schema identity、删表、更新文档。

第 6 步建议用一份清单勾选推进，每改完一个模块跑一次 `cargo check`，并在 PR 描述里贴上勾选结果。

### 5.12 文档体系：对外产品说明 SSOT

**定位转变**

`docs/` 现在是混合体：既有用户看的 `quickstart.md`、`desktop.md`，也有开发者看的 `ui-constraints.md`、`storage-architecture.md`，还有实现演进史 `memory-reflection-redesign.md`、`turn-lifecycle-hardening.md`。读者进门分不清哪份是产品说明书。

重构后 `docs/` 只承载**对外产品说明**，是产品行为的唯一事实源。

**两条硬规则**

1. 产品文档里禁止出现技术性变更内容：表名、库名、代码路径、内部模块名、PRAGMA、缓存策略、性能基线数字、实现演进史。
2. 技术文档全部移入 `docs/dev/`，不进入产品文档索引。

**目录分层**

```text
docs/
  README.md                产品文档索引
  overview.md              产品是什么、解决什么问题
  quickstart.md            安装与首次使用
  concepts.md              核心概念（用户心智模型，不是表结构）
  cli.md                   命令参考（用户可用参数）
  configuration.md         配置项说明（用户可改的键）
  desktop.md               桌面端使用
  tui.md                   TUI 使用
  skills.md / mcp.md / providers.md
  memory-management.md / persona.md / voice.md
  usage-statistics.md      用量看板怎么读
  troubleshooting.md       用户视角排障
  security-baseline.md     安全说明
  images/

docs/dev/                  开发者与维护者文档，不属产品 SSOT
  README.md
  ui-constraints.md
  workbench-design-system.md
  context-message-contract.md
  turn-lifecycle-hardening.md
  storage-architecture.md
  storage-map.md
  reconstruction-wbs.md
```

**逐篇处置**

| 文档 | 处置 | 要点 |
| --- | --- | --- |
| `overview.md` | 产品化重写 | 只讲能力与边界，去掉模块名与存储描述 |
| `concepts.md` | 产品化重写 | Session / Interaction / Turn 用用户语言解释，不提表名 |
| `quickstart.md` | 修订 | 只讲"首次运行会生成什么"，不列库文件名 |
| `cli.md` | 修订 | 只保留用户可用命令；`debug` 类命令移入 dev |
| `configuration.md` | 修订 | 只保留用户可改的键，内部调优项移入 dev |
| `desktop.md` | 修订 | 功能说明，去掉数据来源的实现叙述 |
| `tui.md` | 修订 | 补 `/sessions`、`/new`、`/reset` 的用户说明 |
| `usage-statistics.md` | 产品化重写 | 删掉"rollout.sqlite3 是唯一事实源"这类实现细节 |
| `troubleshooting.md` | 产品化重写 | 保留用户可执行的动作，删掉 WAL 与写锁的成因分析 |
| `security-baseline.md` | 核对 | 用户可理解的边界保留，TLS 与凭据实现细节移入 dev |
| `skills.md`、`mcp.md`、`providers.md`、`memory-management.md`、`persona.md`、`voice.md` | 核对 | 只确认无表名与代码路径 |
| `ui-constraints.md` | 移入 `docs/dev/` | 开发者规范 |
| `workbench-design-system.md` | 移入 `docs/dev/` | 设计系统属于实现规范 |
| `context-message-contract.md` | 移入 `docs/dev/` | 内部装配契约 |
| `turn-lifecycle-hardening.md` | 移入 `docs/dev/` | 内部状态机加固 |
| `memory-reflection-redesign.md` | 移入 `docs/dev/` | 重构设计记录 |
| `storage-architecture.md`、`storage-map.md`、`reconstruction-wbs.md` | 留在 `docs/dev/` | 本次产出 |

**连带修改**

- `AGENTS.md` 对 `docs/ui-constraints.md` 与 `docs/workbench-design-system.md` 的强制前置引用改为 `docs/dev/` 路径（共 4 处）。
- `tools/verify-docs.ps1` 的坏链检查覆盖 `docs/dev/`。
- `docs/README.md` 只索引产品文档；`docs/dev/README.md` 单独索引技术文档。

**校验**

对产品文档做一次机械扫描，命中即失败：`jarvis_`、`.sqlite3`、`session_events`、`runtime.sqlite3`、`rollout.sqlite3`、`crates/`、`payload_ref`、`lane_id`、`WAL`、`PRAGMA`。
### 5.13 仓库脚本清理（已执行）

删除 11 个历史阶段验收脚本，均为 JARVIS 阶段产物或零引用空壳：

```text
tools/jarvis-acceptance.sh            空壳，仅 set -euo pipefail
tools/verify-jarvis-m3.ps1            25 KB，M3 生态扩展验收，零引用
tools/verify-jarvis-r9.ps1            零引用
tools/verify-jarvis-gateway.ps1       仅内部调用已删的 capacity/soak
tools/verify-jarvis-capacity.ps1      由 cargo test --ignored 取代
tools/verify-jarvis-soak.ps1          同上
tools/smoke-setup.ps1                 零引用
tools/check-tui-naming.ps1            零引用，能力已并入 verify-architecture.py
tools/check-architecture-layers.ps1   零引用，能力已并入 verify-architecture.py
tools/benchmark-prompt-cache.ps1      一次性基准
scripts/memory-baseline-check.ps1     零引用，记忆子系统已另有门禁
```

连带同步：`docs/daemon.md` 的容量/soak 入口改为 `cargo test --ignored`；`daemon_host_capacity_tests.rs` 与 `daemon_host_soak_tests.rs` 的文档注释与 `#[ignore]` 理由同步更新。

保留的全部是有真实调用方的：CI 使用的 8 个门禁（`verify-architecture` / `verify-docs` / `verify-ui-contract` / `verify_performance` / `verify_tui_smoke` / `verify_cli_release_smoke` / `check-no-lint-allows` / 两个 `.py` 实现）、真机验证的 4 个 CDP 脚本、`verify-daemon-restart.sh`、`verify-workspace.ps1` 与 `verify-output-filter.ps1`（被 `axon-output-filter/README.md` 引用）、`perf-baseline.json` 与 `tui_smoke_contract.json` 两个数据文件，以及 `scripts/` 下全部安装链脚本。

## 6. 数据处置（已执行）

按长官确认：除红框保留集外全部删除，按全新装机处理，不做迁移。

### 6.1 保留集

| 路径 | 内容 |
| --- | --- |
| `<root>/events/` | `daily-report` / `jira` / `mail-patrol` 任务产物目录 |
| `<root>/identity/owner.json` | 设备身份 |
| `<root>/persona/` | 人格美术资源（`art-fullbody.png`、`art-halfbody.png`、`image-01.png`）与 `character.md` |
| `<root>/skills/` | 全部技能 |
| `<root>/.env`、`.mcp.json`、`.setup.lock`、`AGENTS.md`、`coding.md`、`config.toml`、`config.toml.backup-1794732877696`、`config.toml.backup-1794732970119`、`SOUL.md` | 配置与身份提示 |

### 6.2 已删除

| 路径 | 体量 | 损失 |
| --- | --- | --- |
| `<root>/data/` | 3803.5 MB | 运行态事实源、会话、归档、日志、记忆库、人格状态库、附件 |
| `<root>/bin/` | 164.0 MB | 安装树，由重装重建 |
| `<root>/search-index/` | 169.0 MB | 全库检索索引，可重建 |
| `<root>/sessions/` | 104.1 MB | 会话笔记与窗口滚动锁 |
| `<root>/cron/` | 3.9 MB | 定时任务定义，已备份 |
| `<root>/usage/` | 1.5 MB | 用量统计与定价规则，已备份 |
| `<root>/cache/` | 0.1 MB | MCP schema 缓存 |
| `<root>/crates/` | 0 | 空残留 |

合计释放约 4.2 GB。删除后 `<root>` 剩 345.19 MB，其中 `skills/` 占 338.9 MB。

### 6.3 备份（删除前）

删除前把体量小、含定义或不可再生的运营数据备份到 `<home>/.axon-backup-20261116-154743`（21.8 MB）：

```text
persona.sqlite3    # 人格状态库（状态、转移日志、习得风格）
memory/            # 记忆权威源 memory.db 与 Tantivy / HNSW 投影
cron.sqlite3       # 定时任务定义
usage.sqlite3      # 用量统计与 pricing_rules
attachments/       # 附件归档
```

需要恢复时，在新版本首次启动前把对应文件拷回 `<root>/data/persona/`、`<root>/data/memory/`、`<root>/cron/`、`<root>/usage/`、`<root>/data/attachments/` 即可。不恢复则这些能力从零开始。

### 6.4 全新装机

```powershell
pwsh -File scripts/install-from-source.ps1 -Components all
```

安装脚本重建 `<root>/bin/` 版本树，写入 `launcher`、`current.json`、`axon.cmd` 并注册 Daemon。首次启动会按新 schema 重建控制面库与事件面目录。

### 6.5 后果（已裁决：全部不恢复）

1. 人格美术资源与 `character.md` 保留，人格状态库**不恢复**，人格从当前 `character.md` 重新累积。
2. 长期记忆**不恢复**，记忆库从空开始。
3. 定时任务定义**不恢复**，系统内置任务会在首次启动时重建，用户自建任务需重新创建。
4. 历史用量与 `pricing_rules` **不恢复**，成本看板从首次启动开始计。
5. 附件归档**不恢复**，历史消息中的附件引用失效。
6. 运行态全部消失：`runtime.sqlite3`、`rollout.sqlite3`、thread JSONL、archive、logs。

备份保留在 `<home>/.axon-backup-20261116-154743`，随时可手工取回其中单项。

## 7. 体量预估

按 DSH 实测基准（235 个会话压缩后 556.8 MB，2.37 MB/会话；单会话最大压缩比 6.31x）与本机 AXON 的事件结构换算：

| 项 | 现状 | 目标 |
| --- | --- | --- |
| 每会话事件日志 | 16.4 MB（未压缩，含索引，历史存量口径） | 约 0.3 到 0.8 MB（边界快照 + zstd） |
| 116 会话总量 | 1903.5 MB | **约 40 到 90 MB** |
| 控制面库 | 1903.5 MB（同库） | **小于 50 MB 且不再增长** |
| 单次写锁占用 | 数十毫秒到数十秒（checkpoint） | 毫秒级（控制面事务）+ 追加写不持库锁 |

两点说明：

1. 上表的"现状"是历史存量口径。帧分类（§1.4）已把正文增量挡在库外，因此**稳态日增会显著低于历史峰值**，上述 40 到 90 MB 是带历史存量换算的保守上限。
2. 关键收益不是"省磁盘"，而是**写锁持有时长下降两个数量级**：控制面事务是毫秒级的单行/单表操作，事件面追加不持有任何数据库锁。续租不再可能被业务写饿死。


## 8. 分阶段实施

每一步都可独立验证、可回滚，不允许一次性大爆炸。

### M0：基座（不动业务）

- 新建 `crates/axon-eventlog`：在 `axon-jsonl` 之上封装 `EventLogStore`，提供 `append_batch`、`read_from(seq)`、`read_tail(limit)`、`repair`、`frame` 级校验。
- 依赖 `zstd`、`fs4`、`sha2`，均为既有 workspace 依赖。
- 纯新增 crate，不接入业务。门禁：单元测试覆盖 torn frame、checksum 失败、并发追加、跨进程锁。

### M1：事件面切换（最大的一刀）

- 新增 `events/<session>/session.v1.jsonl.zstd` 写入路径。
- 把 `append_event` / `append_events` / `append_interaction_events` / `commit_action` 改为"事件面 + 控制面水位"。
- `session_events` 表与三个索引从 schema 删除。
- 帧分类（`frame_class.rs:44-79`）与 sink 的 `LivePart` 累积策略**保持不变**：它们已经决定"什么值得落盘"，M1 只换落地介质，不动这个判定。已确认：每个 part 结束时各落一条快照，assistant 与 reasoning 都落。
- 桌面端已经是"实时增量 + 终态"两条通道（`LivePart` 只走实时），M1 不需要改前端渲染模型，只需确认断线重连后的补帧来源。
- 同一批落地 §4.11 的三类事件：`turn_metrics`、`job_started`、`job_completed`，以及 `control.sessions` 的六个汇总列。这三类事件不影响既有读路径，可独立验收。
- 门禁：gateway replay 契约测试、桌面端真机首屏与断线重连验证、`cargo test --workspace`、容量验收（连续 24 小时写入后库体量恒定）、`turn_metrics` 与 `usage_facts` 同轮数值一致。

### M2：控制面收缩

- 删除 `session_checkpoints`、`jarvis_events`、`jarvis_completion_*`、`jarvis_client_presence`，改由事件面派生或投影。
- `synchronous` 降为 `NORMAL`，`busy_timeout` 降为 1s。
- 移除事件路径上的 `ensure_fence` 写操作，改进程内租约快照。
- 门禁：fence 语义测试（冻结进程模拟）、续租压在写风暴下的存活测试。

### M3：rollout 与 thread 统一

- `rollout_turns` 迁到 `data/rollout/<session>/turn.v1.jsonl.zstd`，删除 `rollout.sqlite3`。
- Thread 层补 zstd frame 与 checksum。
- 门禁：恢复链路（`thread_repair`、`turn_repair`）全量回归。

### M4：清理与门禁收口

- 删除 `runtime_cleanup` 里针对已删表的逻辑，改为按会话目录归档或删除。
- 更新 `docs/`（新增存储架构文档并同步索引）、`docs/skills/operate-axon/`（如涉及配置键或诊断命令）。
- 全量门禁：`cargo fmt/check/clippy/test`、`verify-architecture.ps1`、`verify-docs.ps1`、`verify-ui-contract.ps1`。

### M5：发布治理（详见 §13）

- 版本号 `0.1.10` → `0.0.1`，同步 MSI / scoop / homebrew / 文档里的版本引用。
- 删除 14 个旧 tag 与全部 GitHub Releases。
- orphan 分支重建提交历史，force push 覆盖 master。
- 打 `v0.0.1` 触发 release workflow，验证四平台产物。
- 门禁：`axon --version` 输出 0.0.1；`git log --oneline` 只有一个提交；`git tag` 只有一个 tag；release 页面只有 0.0.1 的产物。

## 9. 验证标准

| 指标 | 门槛 |
| --- | --- |
| 控制面库体量 | 24 小时持续运行后不超过 50 MB，且增量小于 5 MB |
| 单次事件写入延迟 p99 | 小于 5 ms（不含 fsync）；含 fsync 小于 20 ms |
| 续租存活 | 人为制造 60 秒写风暴，续租不返回 `Fenced`，Worker 不停 |
| 崩溃恢复 | 在 append 任意位置 `kill -9`，重启后日志可读、水位自动对齐、零数据丢失（除未 flush 的 transient delta） |
| 断线重连 | 桌面端断网重连后视图与在线态一致 |
| 磁盘下降 | 同等工作量下运行态目录小于现状的 10% |

## 10. 风险与决策点

| 风险 | 影响 | 缓解 |
| --- | --- | --- |
| 事件面与水面非原子 | 崩溃时水位与日志不一致 | 单向写序 + 恢复时以日志为准 + `session_recovered` 事件 |
| 去掉事务内 fence 的语义弱化 | 极端脑裂窗口内旧 host 可能多写几帧 | generation 单调 + 读取侧按代际过滤 + 文档显式声明 |
| 桌面端依赖帧级重放 | 断线重连行为改变 | 内存广播 + 终态重建；先用 M1 的契约测试锁定行为 |
| 多进程同时追加同一会话日志 | 序号冲突 | `fs4` 文件锁 + 序号连续性校验，冲突即拒绝 |
| zstd 追加写 | 每次追加要新建 frame，小批次压缩率低 | 批窗口合并（照搬 DSH 固定窗口），窗口内的 delta 合并成一个 frame |
| `list()` 全量扫描 | 会话数增长后变慢 | 控制面保留 `session_runtimes`，列表走控制面，不走日志 |

## 11. 已裁决事项

| # | 议题 | 结论 |
| --- | --- | --- |
| 1 | 客户端会话归属 | **必须显式携带 `session_id`**，服务端不推断。桌面左侧菜单栏即 `sessions` 列表（现为车道列表），选中即切换；TUI 用 `--session <id>` 指定。无 UI 渠道靠 `control.channel_bindings` 映射 |
| 2 | 事件写入与 fence 的原子性 | **先 fence 后写文件**，代际交错由 `generation` 列过滤。不引入 intent / pending 表，崩溃时按水位回退并从结算记录重建 `session_recovered` |
| 3 | `LivePart` 快照粒度 | **保持现状**：每个 part 结束时各落一条，assistant 与 reasoning 都落。帧分类逻辑不动 |
| 4 | 备份恢复 | **全部不恢复**。备份 `<home>/.axon-backup-20261116-154743`（21.8 MB）保留不动，作冷存档 |
| 5 | 发布治理 | 版本归零 **0.0.1**；删 14 个旧 tag 与全部 GitHub Releases；orphan 重建历史并 force push；**不保留任何旧版安装包**（详见 §13） |

## 12. 重构后的安装与首次启动

本次已按要求先删数据、暂不安装。重构完成后按下列顺序恢复。

### 12.1 前置

- `<root>` 当前只剩 `events/`、`identity/`、`persona/`、`skills/` 与根下 9 个配置文件（详见 §6.1）。
- `<root>/bin` 不存在，`axon` 命令不可用。这是预期状态。

### 12.2 安装

```powershell
pwsh -File scripts/install-from-source.ps1 -Components all
```

安装脚本会重建 `<root>/bin` 版本树（`launcher/`、`versions/`、`current.json`、`axon.cmd`），并注册与启动 Daemon。脚本需要 UAC 提权，必须有人在机器前确认。

### 12.3 首次启动会发生什么

| 项 | 行为 |
| --- | --- |
| 控制面库 | 按新 schema 从零创建，`SCHEMA_IDENTITY` 已换新值 |
| 事件面目录 | `data/events/`、`data/rollout/`、`data/sessions/threads/` 按需创建 |
| 人格 | 美术资源在 `persona/`，状态库需从 `<home>/.axon-backup-20261116-154743/persona.sqlite3` 恢复 |
| 记忆 | 需从备份的 `memory/` 恢复，否则从零学习 |
| 定时任务 | 需从备份的 `cron.sqlite3` 恢复，否则为空 |
| 用量统计 | 需从备份的 `usage.sqlite3` 恢复，否则从零计 |
| 附件 | 需从备份的 `attachments/` 恢复，否则历史附件引用失效 |

### 12.4 验收矩阵（已安装态真机）

前置条件：

- 验证对象是 `$HOME/.axon/bin/versions/<版本>/` 下的安装态二进制，并与 `target/release/` 核对 SHA-256。
- 桌面端以 `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS='--remote-debugging-port=9333'` 启动，验证结束关闭调试端口。
- 所有 CDP 判定断言 `OK:` 前缀，不看退出码。

#### 12.4.1 基础

| # | 检查 | 判定 |
| --- | --- | --- |
| B1 | `axon --version` | 输出 0.0.1 |
| B2 | `Get-Command axon` | 解析到安装根，不是 cargo bin |
| B3 | `axon doctor` | 通过 |
| B4 | `data/` 布局 | 只有新布局，无 `runtime.sqlite3`、`rollout.sqlite3` |
| B5 | 单回合写入 | `data/events/<session>/session.v1.jsonl.zstd` 生成、可解压、记录数与水位一致 |
| B6 | daemon 重启 3 次 | 续租不报 `database is locked` |
| B7 | 24 小时运行 | `control.sqlite3` 稳定在 50 MB 以内 |

#### 12.4.2 桌面端

| # | 场景 | 判定 |
| --- | --- | --- |
| D1 | 启动 | 主窗口出现，左侧会话菜单渲染，header 指标区可见 |
| D2 | 新建会话 | 左侧新增条目并处于选中态，事件目录创建 |
| D3 | 正常对话 | 流式输出；首字延迟、速率、token 在 header 出现数值 |
| D4 | 工具调用 | 工具卡片渲染，参数与结果可见 |
| D5 | 子代理 | 子会话条目出现，状态从 running 到终态 |
| D6 | 后台任务 | 任务列表出现，状态流转，终止按钮可用 |
| D7 | 切换会话 | 点另一会话，历史正确加载，无残留上一会话内容 |
| D8 | 加载历史会话 | 冷启动后打开旧会话，向上翻页连续 |
| D9 | 导航条 | 置顶、排序、归档状态与 `sessions` 表一致 |
| D10 | header 指标 | 轮次、累计耗时、首字、token 与 `session_metrics` 一致 |
| D11 | 归档会话 | 归档后从默认列表消失，`/sessions --all` 可找回 |
| D12 | 来源渠道可见 | 历史里**每一轮**各自标注来源；同一会话内的桌面轮次与 TUI 轮次显示不同来源，不取会话级统一值 |

#### 12.4.3 TUI

| # | 场景 | 判定 |
| --- | --- | --- |
| T1 | 启动 | 进入交互界面，状态栏正常 |
| T2 | 正常对话 | 流式渲染 |
| T3 | 工具调用 | 工具行渲染 |
| T4 | `/sessions` | 弹窗列出会话，上下键可选 |
| T5 | 进入会话 | 选中后历史加载正确 |
| T6 | `/new` 与 `/reset` | 前者换 `session_id`，后者只换 `run_generation` |
| T7 | `/sessions --all` | 归档会话可见 |

#### 12.4.4 CLI

| # | 场景 | 判定 |
| --- | --- | --- |
| C1 | `axon sessions list` | 列出会话，含 title、lastActivity、turnCount |
| C2 | `axon run "…" --session <id>` | 追加到指定会话，历史被带入上下文（回复能引用上一轮内容） |
| C3 | `--session` 指向不存在的 id | 报错并提示，**不隐式新建** |
| C4 | 不带 `--session` 且无 resume 状态 | 报错并提示先 `axon sessions list` |
| C5 | `axon sessions resume <id>` 后再 run | 追加到该会话 |
| C6 | `--session` 与已保存的 resume 状态冲突 | `--session` 胜出，resume 状态不被改写 |

#### 12.4.5 跨端同步（本次重构的核心验收）

| # | 场景 | 判定 |
| --- | --- | --- |
| X1 | 桌面在聊，TUI 载入同一会话 | TUI 渲染出桌面已发的消息与回复 |
| X2 | TUI 在聊，桌面载入同一会话 | 桌面渲染出 TUI 已发的消息与回复 |
| X3 | 两端同时在线，一端发消息 | 另一端实时收到增量，无需刷新 |
| X4 | 一端中断会话 | 另一端看到中断状态与终态 |
| X5 | 一端新建会话 | 另一端在 `/sessions` 或左侧菜单能看到新条目 |
| X6 | 不同渠道发起的轮次 | TUI 发起的一轮在桌面端显示来源为 tui，反之 desktop；cron 触发的一轮不显示为任何外部渠道 |
| X7 | 同一会话跨渠道续聊 | 桌面先聊两轮后，TUI 载入同一会话再聊一轮；历史里三轮分别标 desktop / desktop / tui，`channel_mix_json` 为 `{"desktop":{"turns":2},"tui":{"turns":1}}` |

记录方式：每条场景留 CDP `OK:` 行或 TUI 快照；未取得真机证据的条目如实登记"未真机验证"，不得用测试覆盖替代。
## 13. 发布治理：历史重置与版本归零

时间点：M4 门禁全绿之后执行。这是重构的最后一步，不是可选收尾。

### 13.1 为什么要重置

重构把存储契约、表名、协议字段全部换掉。旧历史与旧产物里出现的 `runtime.sqlite3`、`jarvis_*`、`lane_id`、`payload_ref` 在新代码里已不存在。保留它们会让 `git log`、`git blame`、issue 引用、release 下载页全部指向已删除的结构，读者无法判断哪些是当前事实。

### 13.2 版本号归零

版本号唯一来源是 workspace 清单，只需改一处：

```toml
# Cargo.toml:48
version = "0.1.10"   →   version = "0.0.1"
```

各 crate 用 `version.workspace = true` 继承，不必逐个修改。改完必须同步核对这些下游：

| 位置 | 检查点 |
| --- | --- |
| `installer/windows/axon.wxs` | MSI `Version` 属性与 ProductCode |
| `installer/scoop/axon.json` | `version` 与下载 URL、哈希 |
| `homebrew/axon.rb` | `version` 与 sha256 |
| `docs/` | 任何写死的版本号 |
| `.github/workflows/release.yml` | 版本从 tag 推导，无需改；确认推导规则 |

### 13.3 提交历史重置

用 orphan 分支重建，保留 `.git` 与 remote 配置，不重建仓库：

```powershell
# 1. 冷备：确认无误前不要删
git branch pre-reset master

# 2. 用当前工作树建一条无父提交
git checkout --orphan reconstruct
git add -A
git commit -m "chore: 存储架构重构后的首个提交"

# 3. 覆盖 master 并强推
git branch -M master
git push --force origin master
```

要点：

- `pre-reset` 分支保留完整旧历史，等新版本发布验证通过后再删。
- force push 后旧历史在远端短期内仍可从 reflog 与对象缓存访问，超过 GC 才真正消失。
- 必须在旧 tag 与旧 release 已清理之后执行，否则 orphan 提交会与残余 tag 指向不同历史。

### 13.4 tag 与 release 清理

当前 tag 共 14 个：

```text
archive/axon-edit-omp-audit, archive/ci-three-platform, archive/pre-open-source,
v0.1.0, v0.1.1, v0.1.2, v0.1.3, v0.1.4, v0.1.5, v0.1.6, v0.1.7, v0.1.8, v0.1.9, v0.1.10
```

删除本地与远端：

```powershell
git tag -l | ForEach-Object { git tag -d $_ ; git push origin ":refs/tags/$_" }
```

删除 GitHub Releases（每条 release 会把附件一起删掉）：

```powershell
gh release list --limit 200
gh release delete <tag> --yes --cleanup-tag
```

打新 tag 并触发构建：

```powershell
git tag -a v0.0.1 -m "Axon 0.0.1：存储架构重构首个版本"
git push origin v0.0.1
```

`release.yml` 会据 tag 产出 tar.gz / zip / msi / pkg / deb 五类产物，历史产物不再保留。

### 13.5 本地与远端构建产物

| 位置 | 处理 |
| --- | --- |
| `dist/`、`target/release/bundle/` | 删除 |
| `installer/windows/` 下的 MSI 输出 | 删除 |
| `<root>/bin/versions/*` | 不手工删，由 launcher 在安装新版本时按 release GC 回收 |
| GitHub Actions artifacts | 由保留期自动过期，无需动作 |
| GitHub Releases 附件 | 随 §13.4 的 release 删除一并消失 |

### 13.6 执行顺序与风险

```text
1. M4 门禁全绿
2. git branch pre-reset master            # 冷备
3. 改 Cargo.toml 版本为 0.0.1，重新构建并跑通门禁
4. 删远端 tag 与 release
5. orphan 重建历史并 force push
6. 打 v0.0.1，跑 release workflow，验证产物
7. 验证通过后删除 pre-reset 分支
```

风险：

- **force push 不可逆**：远端旧历史短期可从 reflog 恢复，超过 GC 后永久消失。
- **外部引用失效**：issue、PR、外部文档里指向旧 commit 的链接全部 404。
- **已安装用户无法回滚（已裁决接受）**：旧 release 删除后，已装 0.1.x 的用户下载不到旧版本。已确认**不保留任何旧版安装包**，彻底干净，符合"全新开始"的语义。
- **版本号归零是语义声明**：0.0.1 表示"从头开始"，不是"0.1.10 的补丁"。
