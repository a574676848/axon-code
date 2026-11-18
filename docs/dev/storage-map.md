# AXON 数据存储层全量测绘报告

> 目标仓库：本仓库根（Rust workspace，AXON）。所有条目均为 `文件路径:行号`，路径相对仓库根。
> 本报告只陈述代码事实，不含建议。

---

## 0. 路径解析链（所有存储单元的公共前缀）

| 事实 | 位置 |
| --- | --- |
| 运行数据目录名常量 `AXON_RUNTIME_DATA_DIR = "data"` | `crates/axon-config/src/lib.rs:677` |
| `.axon` 目录名常量 `USER_AXON_DIR` | `crates/axon-config/src/schema/defaults.rs:63` |
| 用户级 Axon 根目录拼接 `user_home...join(USER_AXON_DIR)` | `crates/axon-config/src/config_paths.rs:97` |
| CLI 侧根目录 `runtime_axon_root_path_static` | `crates/axon-cli/src/runtime/paths.rs:21` |
| CLI 侧 data 目录 `runtime_axon_data_path_static` | `crates/axon-cli/src/runtime/paths.rs:26` |
| CLI 侧 `RUNTIME_DATA_DIR = "data"` | `crates/axon-cli/src/daemon_commands.rs:22` |
| Daemon 侧 `RuntimePaths::new`（`root/data`、`root/data/logs`） | `crates/axon-daemon/src/infrastructure/runtime_paths.rs:19-27` |
| Daemon 侧 `control.sqlite3` / `outbox.sqlite3` 路径 | `crates/axon-daemon/src/infrastructure/runtime_paths.rs:28-36`（`control_database` `:30-31`、`outbox_database` `:35-36`） |
| Session 持久化目录 `session_store_path(root) = root/data/sessions` | `crates/axon-session/src/infrastructure/mod.rs:61-63` |
| 目录名常量 `SESSION_STORE_DIR = "sessions"` | `crates/axon-session/src/infrastructure/mod.rs:57` |
| `axon run` / daemon 共享 runtime 库的唯一解析点 | `crates/axon-cli/src/run_commands/run_agent_store.rs:12-15` |
| `SqliteRuntimeStore::from_root` | `crates/axon-session/src/infrastructure/sqlite_runtime_store.rs:287-289` |
| 运行态两库的文件名常量 `CONTROL_DATABASE_FILE_NAME` / `OUTBOX_DATABASE_FILE_NAME` | `crates/axon-session/src/infrastructure/sqlite_runtime_store.rs:100`、`:102`（原 `DATABASE_FILE_NAME` 已随分库删除） |
| 事件正文归档根 `event_archive_root()` | `crates/axon-session/src/infrastructure/sqlite_runtime_store.rs:222-227` |

---

## 1. 持久化存储单元清单

### 1.1 SQLite 数据库

| # | 文件名常量 | 文件名字面量定义位置 | 物理路径 | 所属 crate | 创建/打开代码位置 |
| --- | --- | --- | --- | --- | --- |
| 1 | `CONTROL_DATABASE_FILE_NAME` / `OUTBOX_DATABASE_FILE_NAME` | `crates/axon-session/src/infrastructure/sqlite_runtime_store.rs:100`、`:102` | `<axon_root>/data/control.sqlite3`、`<axon_root>/data/outbox.sqlite3`（`SqliteRuntimeStore::build` 在库目录下 `join`，`sqlite_runtime_store.rs:233-246`） | `axon-session` | `SqliteRuntimeStore::from_root` `crates/axon-session/src/infrastructure/sqlite_runtime_store.rs:287-289`；实际 `Connection::open` 在 `.../sqlite_runtime_store/connection_pool.rs:97` |
| 2 | `CRON_DATABASE_FILE` | `crates/axon-cli/src/cron_commands/repository.rs:17` | `<axon_root>/cron/cron.sqlite3`（`crates/axon-cli/src/cron_commands/repository.rs:41-45`） | `axon-cli` | `CronRepository::open` `crates/axon-cli/src/cron_commands/repository.rs:40-49` → `connection()` `crates/axon-cli/src/cron_commands/repository.rs:350` |
| 3 | `USAGE_DATABASE_FILE` | `crates/axon-cli/src/usage_statistics/repository.rs:27` | `<axon_root>/usage/usage.sqlite3`（`crates/axon-cli/src/usage_statistics/repository.rs:54-57`；清理侧同一路径 `crates/axon-cli/src/usage_statistics/mod.rs:103`） | `axon-cli` | `UsageRepository::open` `crates/axon-cli/src/usage_statistics/repository.rs:49-73` |
| 4 | `"persona.sqlite3"`（路径拼接，无独立常量） | `crates/axon-persona/src/infrastructure/sqlite_persona_store.rs:172-179` | `<axon_root>/data/persona/persona.sqlite3` | `axon-persona` | `PersonaStore::open` `crates/axon-persona/src/infrastructure/sqlite_persona_store.rs:200-213`；只读 `open_read_only` `:224-234` |
| 5 | `GOVERNANCE_DB_FILE` | `crates/axon-cli/src/run_commands/run_post_run_governance_store.rs:10` | `<axon_root>/data/sessions/post_run_governance.sqlite3`（`crates/axon-cli/src/run_commands/run_post_run_governance_store.rs:36-40`） | `axon-cli` | `GovernanceQueue::open` `crates/axon-cli/src/run_commands/run_post_run_governance_store.rs:36-56` |
| 6 | `MCP_SCHEMA_CACHE_FILE` | `crates/axon-mcp-runtime/src/runtime/mcp_schema_cache.rs:14`；清理侧同名常量 `crates/axon-cli/src/cron_commands/session_cleanup_files.rs:24` | `<axon_root>/cache/mcp_tool_schemas.sqlite3`（`crates/axon-mcp-runtime/src/runtime/mcp_schema_cache.rs:228-230`） | `axon-mcp-runtime`（写读）/`axon-cli`（清理删除行） | `open_cache` `crates/axon-mcp-runtime/src/runtime/mcp_schema_cache.rs:197-209` |
| 7 | `MEMORY_DB_FILE = "memory.db"` | `crates/axon-memory/src/lib.rs:156` | `<axon_root>/data/memory/memory.db`（`MEMORY_DATA_PARENT_DIR`/`MEMORY_DATA_DIR` `crates/axon-memory/src/lib.rs:152-154`；只读侧 `<root>/data/memory` `crates/axon-memory-read/src/lib.rs:144`） | `axon-memory` | `crates/axon-memory/src/store/store_schema.rs:187-195` |
| 8 | 测试 fixture 私有库（非产品存储） | `crates/axon-cli/src/cron_commands/session_cleanup_sqlite.rs:106` 的 `maintenance.sqlite3` 仅出现在 `#[cfg(test)]` | — | — | 仅测试 |

补充事实（SQLite 库级）：

- 会话清理会**递归扫描 `<axon_root>` 下所有 `*.sqlite3`** 做 VACUUM 维护：后缀常量 `crates/axon-cli/src/cron_commands/session_cleanup_sqlite.rs:11`，扫描 `:33-64`，仅当 `PRAGMA freelist_count > 0` 且 `PRAGMA quick_check(1) == ok` 才执行 `VACUUM;`（`:66-81`）。
- usage、governance 两个库在打开时设置 `auto_vacuum = INCREMENTAL`：`crates/axon-cli/src/usage_statistics/repository/retention.rs:400-422`、`crates/axon-cli/src/run_commands/run_post_run_governance_store.rs:48`。运行态库（`control.sqlite3` 与 `outbox.sqlite3`）也会设 `INCREMENTAL`，但仅对**空库**在建表前生效（`crates/axon-session/src/infrastructure/sqlite_runtime_store/schema.rs:75-90`，调用点 `:113`）；`cron.sqlite3`、`persona.sqlite3`、`mcp_tool_schemas.sqlite3` 的打开路径**没有** `PRAGMA auto_vacuum` 语句（依据：全仓 `auto_vacuum` 命中点为上述各处 + `crates/axon-cli/src/cron_commands/session_cleanup_sqlite.rs:111` 的测试 fixture）。


### 1.2 各 SQLite 库的表清单（除运行态库见第 2 节）

| 库 | 表 | 定义位置 |
| --- | --- | --- |
| `cron.sqlite3` | `cron_jobs` | `crates/axon-cli/src/cron_commands/repository_schema.rs:31` |
| | `cron_runs` | `crates/axon-cli/src/cron_commands/repository_schema.rs:46` |
| | `cron_agent_config` | `crates/axon-cli/src/cron_commands/repository.rs:367` |
| | 索引 `idx_cron_jobs_due`、`idx_cron_runs_job`、`idx_cron_runs_finished_at` | `crates/axon-cli/src/cron_commands/repository_schema.rs:44,55-56` |
| `usage.sqlite3` | `usage_facts` | `crates/axon-cli/src/usage_statistics/repository/schema.rs:141`（v2 迁移 `:229`，v3 `:333`） |
| | `usage_rollup_minute` | 同上 `:112`（v3 `:371`） |
| | `usage_rollup_hour` | 同上 `:177`（v2 `:267`，v3 `:392`） |
| | `usage_rollup_day` | 同上 `:196`（v2 `:286`，v3 `:411`） |
| | `usage_projection_state` | 同上 `:215`（v2 `:305`，v3 `:430`） |
| | `pricing_rules` | 同上 `:90`（v2 `:311`，v3 `:436`） |
| | `PRAGMA user_version = 6`（当前 schema 版本） | 断言见 `crates/axon-cli/src/usage_statistics/repository/schema_tests.rs:110,294` |
| `persona.sqlite3` | `persona_schema_identity` | `crates/axon-persona/src/infrastructure/sqlite_persona_store.rs:27` |
| | `persona_state` | 同上 `:28` |
| | `persona_transitions` | 同上 `:39` |
| | `persona_learned_style` | 同上 `:51` |
| | `persona_style_publication` | 同上 `:62` |
| `post_run_governance.sqlite3` | `governance_jobs` | `crates/axon-cli/src/run_commands/run_post_run_governance_store.rs:256` |
| `mcp_tool_schemas.sqlite3` | `mcp_schema_cache` | `crates/axon-mcp-runtime/src/runtime/mcp_schema_cache.rs:214-220` |
| `memory.db` | `memory_records` | `crates/axon-memory/src/store/store_schema.rs:38` |
| | `memory_supersessions` | `crates/axon-memory/src/store/store_supersession.rs:17` |
| | `memory_commitments` | `crates/axon-memory/src/store/store_thread.rs:83` |
| | `memory_threads` | `crates/axon-memory/src/store/store_thread.rs:97` |
| | `memory_stage1_outputs` | `crates/axon-memory/src/pipeline/pipeline_state.rs:114` |
| | `memory_pipeline_jobs` | `crates/axon-memory/src/pipeline/pipeline_state.rs:128` |
| | `memory_usage` | `crates/axon-memory/src/usage/usage.rs:65` |
| | `memory_runtime_quality` | `crates/axon-memory/src/usage/quality.rs:85` |
| | `memory_vector_projections` | `crates/axon-memory/src/projection/vector_projection.rs:57` |
| | `memory_schema_versions` | `crates/axon-memory/src/projection/vector_projection.rs:68` |

### 1.3 非 SQLite 持久化

| 单元 | 路径 / 命名规则 | 常量或实现位置 | 所属 crate |
| --- | --- | --- | --- |
| Thread 事件 JSONL 分片 | `<axon_root>/data/sessions/threads/` 下按 thread 分片，后缀 `.jsonl` | 目录名 `THREAD_EVENTS_FILE = "threads"` `crates/axon-session/src/infrastructure/jsonl_store.rs:22`；路径 `JsonlThreadEventStore::path` `:59-61` | `axon-session` |
| 通用 JSONL 分片存储（shared） | 逻辑路径 + 物理 shard，宽度 6 位序号 | `ShardedJsonlStore` `crates/axon-jsonl/src/store.rs:29`；`JSONL_SUFFIX = ".jsonl"` `:25`；`SHARD_SEQUENCE_WIDTH = 6` `:21`；默认分片上限 8 MiB `:12`；单条记录硬上限 64 MiB `:19` | `axon-jsonl` |
| 会话事件正文外置分片 | `<db_dir>/archive/session-events/<session>/<shard>.jsonl`，5 位零填充 shard 号 | `crates/axon-session/src/infrastructure/sqlite_runtime_store/event_payload_store.rs:53-57`；目录常量 `EVENT_ARCHIVE_DIR = "archive"` `.../sqlite_runtime_store.rs:231`，`SESSION_EVENT_ARCHIVE_SUBDIR = "session-events"` `.../sqlite_runtime_store.rs:233` | `axon-session` |
| 会话事件离线归档（zstd） | `<db_dir>/archive/session-events/<session_id>.jsonl.zst`，追加写、每帧独立 zstd 段 | `crates/axon-cli/src/cron_commands/runtime_event_archive.rs:190-211`；目录 `event_archive_root` `:59-62` | `axon-cli` |
| 内容归档（provider 工具输出） | `<data>/archive` 下（目录名复用 `CONTENT_ARCHIVE_DIR_NAME`） | 引用常量 `crates/axon-cli/src/cron_commands/session_cleanup_files.rs:28` | `axon-agent-runtime` |
| Shell 完整输出归档 | `<data>/logs/tee`，后缀 `.shell.log`；回退到 `%TEMP%/tee` | `SHELL_ARTIFACT_DIR_NAME = "tee"` `crates/axon-agent-runtime/src/runtime_limits.rs:62`；后缀 `:65`；回收侧 `crates/axon-cli/src/cron_commands/session_cleanup_files.rs:32-34,77-81` | `axon-agent-runtime` |
| 附件归档 | `<data>/attachments`（子目录 `blobs`、`previews`） | `ATTACHMENT_ARCHIVE_DIR_NAME = "attachments"` `crates/axon-agent-runtime/src/attachment_runtime.rs:39`；`BLOB_DIR_NAME` `:47`、`PREVIEW_DIR_NAME` `:48`；清理侧 `crates/axon-cli/src/cron_commands/session_cleanup_files.rs:30` | `axon-agent-runtime` |
| Memory 倒排索引（Tantivy） | `<memory_dir>/tantivy` | `MEMORY_TANTIVY_DIR` `crates/axon-memory/src/lib.rs:158` | `axon-memory` |
| Memory 向量索引（HNSW 镜像） | `<memory_dir>/hnsw.idx` | `MEMORY_HNSW_FILE` `crates/axon-memory/src/lib.rs:160` | `axon-memory` |
| Memory 工作区产物 | `raw_memories.md`、`turn_summaries/`、`extensions/` | `crates/axon-memory/src/workspace/artifacts.rs:13,15,17` | `axon-memory` |
| 语义检索索引（workspace） | `<workspace>/.axon` 侧 `search-index/index.json`，格式版本 1、候选 schema 版本 3 | `SEARCH_INDEX_DIRECTORY = "search-index"` / `SEARCH_INDEX_FILE = "index.json"` `crates/axon-agent-runtime/src/core/tools/search_index/model.rs:10-11`；`INDEX_FORMAT_VERSION` `:9`；`SEARCH_INDEX_SCHEMA_VERSION` `:24` | `axon-agent-runtime` |
| 会话索引 | `session_index.jsonl` | `crates/axon-agent-runtime/src/core/session/session_index.rs:17` | `axon-agent-runtime` |
| 自迭代回执 | `self_iteration_receipts.jsonl` | `crates/axon-agent-runtime/src/runtime/self_improvement/self_iteration_receipt_store.rs:16` | `axon-agent-runtime` |
| 反思池 / 账本 | `reflection_pool.jsonl`、`reflection_pool.lock`、`reflection_processed.jsonl`、`reflection_written.jsonl`、`reflection_pool_inflight.json` | `crates/axon-agent-runtime/src/runtime/self_improvement/reflection_pool_store.rs:18,21`；`.../reflection_ledger.rs:17,19,21` | `axon-agent-runtime` |
| 记忆同步审计 | `memory_sync_audit.jsonl` | `crates/axon-memory-ops/src/operations/sync_audit.rs:8` | `axon-memory-ops` |
| Memory 写入流水线锁 | `write_pipeline` | `crates/axon-memory-write/src/lib.rs:27` | `axon-memory-write` |
| 渠道指标样本 | `<data>/tmp/channel-metrics/samples-*.jsonl`（清理侧） | `crates/axon-cli/src/cron_commands/session_cleanup_metrics.rs:9-11`；运行侧 `crates/axon-cli/src/run_commands/run_channel_affinity.rs:15` | `axon-cli` |
| 进程资源遥测 | `*.jsonl`（`RESOURCE_METRICS_DIR_NAME` 下） | `crates/axon-cli/src/runtime/resource_metrics.rs:33`；清理 `crates/axon-cli/src/cron_commands/session_cleanup_files.rs:85-89` | `axon-cli` |
| TUI agent pane turn 事件 | `terminal_agent_pane_turn_events.jsonl` | `crates/axon-tui-runtime/src/agent_pane/agent_pane_turn_event_store.rs:12` | `axon-tui-runtime` |
| 桌面端草稿历史 | `draft_history.jsonl` | `crates/axon-desktop/src/input_history.rs:20` | `axon-desktop` |
| Gateway Key 存储 | `<axon_root>/data/gateway/auth-keys.json` | `DEFAULT_KEY_STORE_RELATIVE_PATH` `crates/axon-daemon/src/infrastructure/gateway_keys.rs:33` | `axon-daemon` |
| Gateway Key 审计日志 | `<axon_root>/data/logs/gateway-keys-audit.jsonl` | `AUDIT_LOG_RELATIVE_PATH` `crates/axon-daemon/src/infrastructure/gateway_keys.rs:35` | `axon-daemon` |
| Event provider 配置目录 | `<axon_root>/events` | `AXON_EVENT_PROVIDERS_DIR = "events"` `crates/axon-config/src/lib.rs:679`；装配点 `crates/axon-cli/src/daemon_host_runner.rs:228-230` | `axon-cli` |
| MCP OAuth 令牌 | `mcp-oauth/tokens` | `crates/axon-mcp-runtime/src/runtime/mcp_runtime_oauth.rs:54-55` | `axon-mcp-runtime` |
| MCP 健康负缓存 | `mcp_registry_health_negative.json` | `crates/axon-mcp-runtime/src/runtime/mcp_health_cache.rs:27` | `axon-mcp-runtime` |
| Daemon PID / 停止标记 | `daemon.pid`、`daemon.stop` | `crates/axon-cli/src/daemon_service_manager.rs:16`；`crates/axon-cli/src/daemon_stop_request.rs:16` | `axon-cli` |
| 启动更新状态 | `data/update.json` | `crates/axon-cli/src/upgrade_commands/startup_update.rs:18-19` | `axon-cli` |
| 桌面端连接配置 | `~/.axon-desktop` 目录 | `CLIENT_HOME_DIR_NAME` `crates/axon-desktop/src/connection/store.rs:50` | `axon-desktop` |
| 桌面端崩溃重启用标记 | `relaunch-failed.json`、`relaunch-failed.handled.json` | `crates/axon-desktop/src/connection/relaunch.rs:26,28` | `axon-desktop` |
| TUI 状态转储 | `axon-tui-state-dumps/tui-state*` | `crates/axon-tui-runtime/src/surface/state_dump.rs:20-21` | `axon-tui-runtime` |
| apply_patch 事务目录 | `.axon-apply-patch-txn*` | `crates/axon-apply/src/apply_patch_transaction.rs:28` | `axon-apply` |

关于「brain event log」：全仓 `brain` 字面量只有 4 处，且都只是清理任务的**命名**，不存在独立的 brain 事件日志文件或表：

- `SESSION_CLEANUP_CRON_SCRIPT = "axon.internal.brain-cleanup"` `crates/axon-cli/src/cron_commands/session_cleanup.rs:25`
- `SESSION_CLEANUP_DRY_RUN_CRON_SCRIPT = "axon.internal.brain-cleanup-dry-run"` `crates/axon-cli/src/cron_commands/session_cleanup.rs:27`
- `SESSION_CLEANUP_CRON_JOB_ID = "axon.system.brain-cleanup"` `crates/axon-cli/src/cron_commands/mod.rs:41`
- `SESSION_CLEANUP_CRON_NAME = "Axon brain cleanup"` `crates/axon-cli/src/cron_commands/mod.rs:42`

`memory` 的物理落位由 `MemoryConfig` 三路径给出：`db_path` / `tantivy_index_dir` / `hnsw_index_path`，`crates/axon-memory/src/lib.rs:186-199`。

---

## 2. 运行态库（`control.sqlite3` / `outbox.sqlite3`）完整表清单与职责

Schema 全部定义在一个 DDL 批次：`crates/axon-session/src/infrastructure/sqlite_runtime_store/schema.rs:107-499`。
Schema 身份常量 `SCHEMA_IDENTITY = "jarvis-runtime-goal-v1"`：`.../schema.rs:9`；身份表与校验：`:500-516`。
追加式列迁移 `migrate_additive_columns`：`.../schema.rs:529-585`。

### 2.1 表与索引一览

| 表 | 行号 | 主键 | 索引 | 职责（一句话） |
| --- | --- | --- | --- | --- |
| `daemon_hosts` | `schema.rs:108-114` | `host_id` | `daemon_hosts_retention(lease_expires_at_ms, host_id)` `:113-114` | Daemon Host 进程 lease（pid、generation、状态、到期时间），fencing 的唯一锚点 |
| `session_runtimes` | `schema.rs:115-128` | `session_id` | `session_runtimes_recovery(status, owner_host_id, owner_generation)` `:123-124`；`session_runtimes_retention(last_activity_at_ms, status, session_id)` `:125-126`；`session_runtimes_owner(owner_host_id)` `:127-128` | 会话运行态投影（run_session_id、cwd、status、owner 代际、事件/检查点水位） |
| `session_events` | `schema.rs:129-138` | `(session_id, sequence)` | `session_events_retention(emitted_at_ms, session_id, sequence)` `:137-138`；`session_events_session_interaction(session_id, interaction_id, sequence)` `:578-579` | 会话 durable 事件流（正文可外置，见第 6 节） |
| `session_checkpoints` | `schema.rs:139-154` | `(session_id, checkpoint_seq)` | `session_checkpoints_latest(session_id, checkpoint_seq DESC)` `:153-154` | 每次 action 提交的检查点（context_version、token/耗时遥测、transcript_ref） |
| `runtime_schedules` | `schema.rs:155-164` | `schedule_id` | `runtime_schedules_due(status, next_run_at_ms, lease_expires_at_ms)` `:161-162`；`runtime_schedules_session(session_id)` `:163-164` | 持久调度计划（kind/expression/payload、租约、失败计数） |
| `daemon_upgrade_admissions` | `schema.rs:165-178` | `admission_id` | `_ready(state, deadline_at_ms)` `:173-174`；`_owner(old_host_id, admission_id)` `:175-176`；`_expiry(deadline_at_ms, admission_id)` `:177-178` | 升级交接准入（一次性 handoff_token、deadline、claim 状态） |
| `transport_ingress_cursors` | `schema.rs:179-182` | `(adapter, partition_key)` | 无 | 外部传输适配器的 ingress 游标 |
| `jarvis_workspaces` | `schema.rs:183-195` | `workspace_id` | `jarvis_workspaces_recent(last_seen_at_ms DESC, workspace_id)` `:194-195` | Workspace 注册表（root、git remote、别名、memory namespace、pinned） |
| `jarvis_interactions` | `schema.rs:196-226` | `interaction_id` | UNIQUE `jarvis_interactions_source(source_channel, source_message_id)` `:215-216`；`_claim(status, lease_expires_at_ms, received_at_ms, interaction_id)` `:217-218`；`_retention(received_at_ms, status)` `:219-220`；`_scope_status_recent` `:221-222`；`_scope_channel_recent` `:223-224`；`_scope_channel_status_recent` `:225-226`；`_lane_recent(lane_id, received_at_ms DESC, interaction_id DESC)` `:580-581`；`_lane_processing(lane_id, status, lease_expires_at_ms)` `:582-583` | 入站请求/回合收件箱（来源、作用域、受众、回复路由、状态、租约、模型与工具摘要） |
| `jarvis_goals` | `schema.rs:227-252` | `goal_id` | `_active(status, updated_at_ms, goal_id)` `:247-248`；`_scope_active(scope_json, status, updated_at_ms, goal_id)` `:249-250`；`_priority(status, priority DESC, updated_at_ms, goal_id)` `:251-252` | 长期目标（revision、plan/acceptance/progress JSON、优先级、取消原因） |
| `jarvis_executions` | `schema.rs:253-297` | `execution_id` | `_goal_history(goal_id, created_at_ms DESC, execution_id DESC)` `:281-282`；`_goal_dedupe` `:283-284`；UNIQUE 部分索引 `_active_dedupe(COALESCE(session_id, goal_id), dedupe_key) WHERE dedupe_key IS NOT NULL AND status NOT IN (...)` `:285-288`；`_claim(status, priority DESC, created_at_ms, execution_id, retry_at_ms)` `:289-290`；`_session_claim(...)` `:291-292`；`_stale_running(...) WHERE status='running'` `:293-295`；`_conclusive(status, updated_at_ms ASC, execution_id ASC)` `:296-297` | 可执行动作队列（与 SessionActionRecord 同构，claim/lease/retry/cancel 全在此表） |
| `jarvis_results` | `schema.rs:298-316` | `result_id` | `_goal(goal_id, created_at_ms DESC, result_id)` `:311-312`；`_execution(execution_id)` `:313-314`；`_created(created_at_ms ASC, result_id ASC)` `:315-316` | Execution 终态结果（成功标记、summary、receipt/artifact 引用、error_kind） |
| `jarvis_result_reads` | `schema.rs:317-322` | `result_id` | 无 | Result 的已读确认（时间 + 来源） |
| `jarvis_client_presence` | `schema.rs:323-334` | `client_id` | `_workspace(workspace_id, expires_at_ms DESC)` `:331-332`；`_expiry(expires_at_ms ASC, client_id ASC)` `:333-334` | 客户端在线登记（工作区、渠道、投递目的地、过期） |
| `jarvis_completion_notifications` | `schema.rs:335-358` | `notification_id` | `_state` `:348-349`；`_workspace` `:350-351`；`_retention` `:352-353`；`_expiry`（部分，`state IN ('pending','dispatched')`）`:354-356`；`_expiry_all` `:357-358` | 完成通知本体（execution_id UNIQUE、content/summary、result_ids、状态、过期） |
| `jarvis_completion_deliveries` | `schema.rs:359-374` | `(notification_id, channel, destination)` | `_inbox` `:369-370`；`_target_inbox` `:371-372`；`_retention` `:373-374` | 通知的各渠道投递派生记录 |
| `jarvis_agent_messages` | `schema.rs:375-401` | `message_id` | UNIQUE `_idempotency(idempotency_key)` `:394-395`；`_recipient(owner_id, recipient, state, created_at_ms, message_id)` `:396-397`；`_retry` `:398-399`；`_retention` `:400-401` | Agent 间 mailbox 消息（幂等、触发轮次、continuation、重试/过期） |
| `jarvis_continuations` | `schema.rs:402-435` | `continuation_id` | UNIQUE `_idempotency` `:428-429`；`_owner` `:430-431`；`_child(child_session_id, created_at_ms DESC, continuation_id)` `:432-433`；`_retention` `:434-435` | 子代理终态 continuation 引用（父/子会话、termination、token 计量、消费状态） |
| `jarvis_decisions` | `schema.rs:436-451` | `decision_id` | `_scope(effect, expires_at_ms, created_at_ms, decision_id)` `:445-446`；UNIQUE `_scope_revision(scope_json, revision)` `:447-448`；`_expiry(expires_at_ms) WHERE NOT NULL` `:449-451` | 作用域决策（actor/effect/revision/过期） |
| `jarvis_events` | `schema.rs:452-472` | `event_id` | UNIQUE `_idempotency` `:461-462`；UNIQUE `_sequence` `:463-464`；`_goal_lookup(COALESCE(json_extract(payload_json,'$.goal_id'), json_extract(payload_json,'$.goalId')), sequence)` `:465-472` | 目标域 ledger 事件流（幂等键 + 单调 sequence） |
| `jarvis_user_questions` | `schema.rs:473-498` | `question_id` | UNIQUE `_idempotency` `:493-494`；`_lane_state(lane_id, state, created_at_ms DESC)` `:495-496`；`_reconcile(state, delivered, expires_at_ms)` `:497-498` | ask_user_question 的提问/作答状态机（deadline、answer、delivery_mode） |
| `jarvis_schema_identity` | `schema.rs:500-504` | `identity` | 无 | 库身份单行，用于拒绝陌生库；写入 `INSERT OR IGNORE` `:505-508`，校验 `:509-516` |

已废弃索引：`DROP INDEX IF EXISTS jarvis_executions_attempt`（`schema.rs:280`）。

### 2.2 写入者 / 读者（按表）

> 「写」给出实际 SQL 语句所在行；「读」给出 `SELECT ... FROM <表>` 所在行。

**`daemon_hosts`**

- 写：`runtime_repository.rs:133`（`claim_host`）、`:306`（`claim_upgrade_replacement`）、`:171`（`renew_host`）、`:204`（`request_upgrade`）、`:253`（`mark_handoff_ready`）、`:319`；`:51`（`reap_dead_hosts` 置 stopped）；`helpers.rs:131`（`ensure_fence` 的 no-op UPDATE，用 `changed==0` 判定被 fence）。
- 读：`runtime_repository.rs:24`、`:39`、`:114`；`crates/axon-cli/src/system_http_adapter.rs:206`；`crates/axon-cli/src/cron_commands/runtime_cleanup_families.rs:528`；`crates/axon-cli/src/cron_commands/runtime_cleanup_counts.rs:62`。
- 清理：`crates/axon-cli/src/cron_commands/runtime_cleanup_families.rs:528-535`（保留被 session_runtimes / upgrade_admissions 引用的 Host）。

**`session_runtimes`**

- 写：`helpers.rs:76`（`create_session_impl`，`ON CONFLICT DO NOTHING`）；`runtime_repository.rs:371`（恢复为 `recovering`）；`session_control_repository.rs:27`（`stop_session`）、`:63`（`reset_session`）；`sqlite_runtime_store.rs:329`（`interrupt_host_actions`）、`:413`（`set_session_status`）；`event_repository.rs:204`（事件追加后推进 `last_event_sequence`）；`commit_repository.rs:211`（置 `idle` 并推进 checkpoint/event 水位）；`failure_repository.rs:73`；`interrupted_execution_repository.rs:293`；`interrupted_interaction_repository.rs:227`。
- 读：`helpers.rs:104`（`get_session_impl`）；`runtime_repository.rs:72`（可运行会话）、`:355`、`:379`（下一次序号）、`:404`；`event_repository.rs:128`；`commit_repository.rs:91`、`:178`；`failure_repository.rs:55`；`action_repository.rs:183`（context_version）；`goal_commit_repository.rs:218`；`crates/axon-cli/src/workbench_http_lane_readings.rs:248`；`crates/axon-cli/src/cron_commands/runtime_cleanup.rs:287`、`:461`、`:626`；`crates/axon-cli/src/cron_commands/runtime_cleanup_families.rs:275`；`crates/axon-cli/src/cron_commands/runtime_event_archive.rs:115`、`:474`。

**`session_events`**

- 写：
  - `event_repository.rs:153`（`append_event_refs` 的 `INSERT`）— `append_event`（`:34`）/ `append_events`（`:57`）/ `append_interaction_events`（`:79`）共用的唯一常规写入语句；
  - `commit_repository.rs:108`（`commit_action` 事务内批量插入事件）；
  - `failure_repository.rs:67`（`fail_action`）；
  - `runtime_repository.rs:386`（`recover_expired_sessions`）；
  - `interrupted_execution_repository.rs:269`、`interrupted_interaction_repository.rs:203`（隔离中断时补终态事件）。
- 改：`crates/axon-cli/src/cron_commands/runtime_cleanup_session_events.rs:194`（`UPDATE session_events SET payload_json=?1, payload_ref=NULL` — 悬空引用收口为内联占位）。
- 删：`crates/axon-cli/src/cron_commands/runtime_event_archive.rs:321`（`DELETE ... RETURNING rowid`）。
- 读：`event_repository.rs:340`（`replay_events`）、`:378`（`interaction_tail_window`）；`crates/axon-cli/src/workbench_http_lane_readings.rs:100`（`turn_completed` 载荷取正文）；`interrupted_interaction_repository.rs:288`；`crates/axon-cli/src/cron_commands/runtime_event_archive.rs:113`、`:295`；`crates/axon-cli/src/cron_commands/runtime_cleanup_session_events.rs:152`；`crates/axon-cli/src/cron_commands/runtime_cleanup.rs:477`。

**`session_checkpoints`**

- 写：`commit_repository.rs:194`（`commit_action` 内 `INSERT`）。
- 读：**生产代码中无任何 `SELECT`**（全仓 `session_checkpoints` 命中仅：schema 定义 `schema.rs:139,153`、上述写入 `commit_repository.rs:194`、以及清理侧 `crates/axon-cli/src/cron_commands/runtime_cleanup_families.rs:300-302` 与计数 `runtime_cleanup.rs:347-349`）。
- 索引 `session_checkpoints_latest` 在生产代码中无查询使用。

**`runtime_schedules`**

- 写：`schedule_repository.rs:37`（`upsert_schedule`，`ON CONFLICT DO UPDATE`）、`:66`（lease 领取）、`:87`（`settle_schedule` 结算并回写下次触发）。
- 读：`schedule_repository.rs:55`（到期候选）、`:67`（回读单条）。

**`daemon_upgrade_admissions`**

- 写：`runtime_repository.rs:214`（创建）、`:263`（置 `ready`）、`:328`（置 `claimed`）。
- 读：`runtime_repository.rs:294`（按 token 校验并接管）。
- 清理：`crates/axon-cli/src/cron_commands/runtime_cleanup_families.rs:479`。

**`transport_ingress_cursors`**

- 写：`helpers.rs:205`（`save_ingress_cursor`，upsert）。
- 读：`helpers.rs:194`（`load_ingress_cursor`）。

**`jarvis_workspaces`**

- 写：`workspace_repository.rs:18`（`register_workspace`）。
- 读：`workspace_repository.rs:43`（单条）、`:56`（列表）。

**`jarvis_interactions`**

- 写：`interaction_repository.rs:20` 为 SQL 常量，执行于 `:521`（`insert_interaction`），外层事务 `:259-283`（`InteractionStore::append`）；`:309`、`:360`（`claim_processing` / `claim_processing_by_id`）；`:407`（续租）；`:600`（`update_status`）；`event_repository.rs:218`（累加 `tool_summary`）。
- 读：`interaction_repository.rs:17`、`:19`、`:23`、`:25`、`:41`、`:62`、`:89`、`:123`、`:163`、`:193`、`:231`、`:318`、`:377`、`:609`；`lane_query_repository.rs:19`；`health_repository.rs:12`；`crates/axon-cli/src/workbench_http_lane_readings.rs:50`、`:169`；`crates/axon-cli/src/workbench_http_adapter.rs:214`；`crates/axon-cli/src/cron_commands/runtime_cleanup_families.rs:494`；`crates/axon-cli/src/cron_commands/runtime_cleanup_counts.rs:33`。

**`jarvis_goals`**

- 写：`goal_repository.rs:18`（创建）、`:108`（更新，带 `revision` 乐观锁）、`:199`（取消）、`:342`（更新）；`goal_commit_repository.rs:69`、`:153`（确保存在）；`ledger_commit_repository.rs:128`（结算更新）；`interrupted_execution_repository.rs:220`。
- 读：`goal_repository.rs:59`、`:73`、`:93`、`:264`；`goal_commit_repository.rs:172`；`health_repository.rs:13`；`crates/axon-cli/src/cron_commands/runtime_cleanup.rs:485`。

**`jarvis_executions`**

- 写：`action_repository.rs:64`（claim）、`:112`（续租）、`:144`（取消）、`:198`（入队）；`execution_repository.rs:35`、`:66`（入队）、`:223`（批量 claim）、`:268`（续租）、`:323`（结算）、`:382`（终态）；`commit_repository.rs:26`、`:145`（插入下一次 execution）、`:163`；`goal_repository.rs:156`、`:162`、`:209`、`:213`；`goal_commit_repository.rs:237`；`ledger_commit_repository.rs:74`；`failure_repository.rs:49`；`interrupted_execution_repository.rs:193`；`session_control_repository.rs:92`。
- 读：`execution_repository.rs:111`、`:129`、`:152`、`:181`、`:405`；`action_repository.rs:28`、`:39`、`:80`、`:155`、`:228`、`:255`；`helpers.rs:115`；`commit_repository.rs:130`、`:185`；`goal_commit_repository.rs:257`；`interrupted_execution_repository.rs:143`、`:154`；`health_repository.rs:14`、`:15`；`client_delivery_repository.rs:213`；`runtime_repository.rs:72`、`:241`；`crates/axon-cli/src/cron_commands/runtime_cleanup.rs:303`；`runtime_cleanup_families.rs:283`；`runtime_cleanup_counts.rs:83`。

**`jarvis_results`**

- 写：`ledger_commit_repository.rs:108`；`interrupted_execution_repository.rs:204`；`crates/axon-cli/src/run_commands/run_request_payload.rs:545`、`:641`。
- 读：`result_repository.rs:26`（未读列表）、`:73`、`:138`。

**`jarvis_result_reads`**

- 写：`result_repository.rs:136`（`confirm_results`，`INSERT OR IGNORE`）。
- 读：`result_repository.rs:138`。

**`jarvis_client_presence`**

- 写：`client_delivery_repository.rs:37`（`register_client_presence`）；`:63`（注销删除）。
- 读：`client_delivery_repository.rs:79`。

**`jarvis_completion_notifications`**

- 写：`client_delivery_repository.rs:97`（创建）、`:430`（回写 summary）、`:453`（结算状态）。
- 读：`client_delivery_repository.rs:128`、`:149`、`:174`（`INDEXED BY jarvis_completion_notifications_expiry_all`）。

**`jarvis_completion_deliveries`**

- 写：`client_delivery_repository.rs:242`、`:285`、`:386`（`settle_completion_delivery`）、`:411`。
- 读：`client_delivery_repository.rs:262`、`:314`、`:352`、`:481`。

**`jarvis_agent_messages`**

- 写：`agent_mailbox_repository.rs:42`（`append_agent_message`）、`:202`、`:226`、`:301`、`:336`（置 `expired`）。
- 读：`agent_mailbox_repository.rs:75`、`:98`、`:115`、`:139`、`:173`、`:249`、`:286`、`:338`、`:376`。

**`jarvis_continuations`**

- 写：`agent_continuation_repository.rs:35`（`append_continuation`）、`:176`、`:203`、`:231`（置 `expired`）。
- 读：`agent_continuation_repository.rs:77`、`:101`、`:120`、`:144`、`:233`、`:275`。

**`jarvis_decisions`**

- 写：`decision_repository.rs:54`（`append_decision`）。
- 读：`decision_repository.rs:36`（下一个 revision）、`:79`（最新有效 decision）。

**`jarvis_events`**

- 写：`goal_repository.rs:178`、`:224`、`:376`；`goal_commit_repository.rs:349`；`ledger_commit_repository.rs:168`；`ledger_event_repository.rs:59`（`append_ledger_event`）；`interrupted_interaction_repository.rs:157`；`interrupted_execution_repository.rs:229`。
- 读：`ledger_event_repository.rs:22`、`:45`、`:87`、`:115`；`goal_repository.rs:168`、`:218`、`:369`；`goal_commit_repository.rs:289`、`:342`；`ledger_commit_repository.rs:154`；`interrupted_interaction_repository.rs:159`；`interrupted_execution_repository.rs:231`。
- 清理：`crates/axon-cli/src/cron_commands/runtime_cleanup_families.rs:517-519`（按 `sequence` 保底 `LEDGER_EVENT_MIN_RETAINED = 1000`，`crates/axon-cli/src/cron_commands/runtime_cleanup.rs:49`）。

**`jarvis_user_questions`**

- 写：`user_question_repository.rs:23`（`insert_question`，`ON CONFLICT(idempotency_key) DO NOTHING`）、`:152`、`:202`、`:255`、`:276`。
- 读：`user_question_repository.rs:59`、`:73`、`:88`、`:110`、`:123`、`:166`、`:175`、`:229`、`:279`；`crates/axon-cli/src/workbench_http_adapter.rs:214`；`crates/axon-cli/src/cron_commands/runtime_cleanup_families.rs:665`。

**`jarvis_schema_identity`**

- 写：`schema.rs:506`；读/校验：`schema.rs:98-105`、`:509-516`。

### 2.3 保留期与清理口径（作用于上述表）

| 事实 | 位置 |
| --- | --- |
| 会话与 SQLite 事实表保留 `SESSION_RETENTION_DAYS = 7` | `crates/axon-cli/src/cron_commands/session_cleanup.rs:29` |
| 文件面（logs/tmp/archive/attachments）保留 `FILE_RETENTION_DAYS = 3` | `crates/axon-cli/src/cron_commands/session_cleanup.rs:34` |
| 单批删除行数 `RUNTIME_CLEANUP_BATCH_LIMIT = 256` | `crates/axon-cli/src/cron_commands/runtime_cleanup.rs:28` |
| 单轮墙上时钟预算 60 秒 | `crates/axon-cli/src/cron_commands/runtime_cleanup.rs:34` |
| 清理连接 busy_timeout 5 秒 + `foreign_keys=ON` + `synchronous=NORMAL` | `crates/axon-cli/src/cron_commands/runtime_cleanup.rs:579-592` |
| 只读清理连接（`SQLITE_OPEN_READ_ONLY`） | `crates/axon-cli/src/cron_commands/runtime_cleanup.rs:594-604` |
| 会话事件保底 `SESSION_EVENT_MIN_RETAINED = 200`（仅活跃会话） | `crates/axon-cli/src/cron_commands/runtime_cleanup.rs:55` |
| ledger 事件保底 `LEDGER_EVENT_MIN_RETAINED = 1000` | `crates/axon-cli/src/cron_commands/runtime_cleanup.rs:49` |
| 增量回收页数 `INCREMENTAL_VACUUM_PAGE_BATCH = 4096` | `crates/axon-cli/src/cron_commands/runtime_cleanup.rs:37`、执行 `:661-667` |
| 活跃 Session 状态白名单（不参与回收） | `crates/axon-cli/src/cron_commands/runtime_cleanup.rs:46-47` |
| 终态 Interaction / Task 状态白名单 | `crates/axon-cli/src/cron_commands/runtime_cleanup.rs:40`、`:42` |
| 清理顺序：先归档 → 再删事件 → 修悬空引用 → goals/interactions/decisions/events/hosts/presence/deliveries/... | `crates/axon-cli/src/cron_commands/runtime_cleanup.rs:164-229` |
| 清理入口在 Cron 内 | `crates/axon-cli/src/cron_commands/session_cleanup.rs:189-202`（先 `SqliteRuntimeStore::initialize`，再 `cleanup_runtime_data`） |

---

## 3. 写入路径与频率

### 3.1 会写运行态库的全部入口

| # | 入口 | 代码位置 | 写入的表 | 运行环境 |
| --- | --- | --- | --- | --- |
| 1 | `SqliteRuntimeStore::initialize` → schema DDL | `sqlite_runtime_store.rs:238-261` → `schema.rs:51-79` | 全部表 + `jarvis_schema_identity` | Daemon / CLI run / Cron / 网关（首次打开） |
| 2 | `claim_host` | `runtime_repository.rs:99-160` | `daemon_hosts` | Daemon 启动 |
| 3 | `renew_host`（lease worker 循环） | `runtime_repository.rs:162-187`；循环 `crates/axon-cli/src/daemon_host_runner_workers.rs:54-72` | `daemon_hosts` | Daemon 主循环（后台线程） |
| 4 | `reap_dead_hosts` | `runtime_repository.rs:14-62` | `daemon_hosts`（置 `stopped`） | Daemon 启动 |
| 5 | `request_upgrade` / `mark_handoff_ready` / `claim_upgrade_replacement` / `interrupt_host_actions` | `runtime_repository.rs:189-341`；`sqlite_runtime_store.rs:317-355` | `daemon_hosts`、`daemon_upgrade_admissions`、`session_runtimes`、`jarvis_executions` | Daemon 升级流程 |
| 6 | `create_session` | `helpers.rs:65-96` | `session_runtimes` | Daemon（子代理/执行会话登记）、CLI |
| 7 | `stop_session` / `reset_session` | `session_control_repository.rs:13-83` | `session_runtimes`、`jarvis_executions` | Daemon / CLI |
| 8 | `set_session_status` | `sqlite_runtime_store.rs:401-418` | `session_runtimes` | Daemon |
| 9 | `recover_expired_sessions` | `runtime_repository.rs:343`（SQL `:371`、`:386`、`:398`） | `session_runtimes`、`session_events` | Daemon supervisor tick |
| 10 | `enqueue_action` / `claim_next_action` / `renew_action_lease` / `cancel_action` / `update_action_status` / `fail_action` | `action_repository.rs:13-273`；`commit_repository.rs:11-41`；`failure_repository.rs:11-81` | `jarvis_executions`、`session_events`、`session_runtimes` | Daemon |
| 11 | `commit_action`（单事务：execution + checkpoint + 事件 + 下一次 execution） | `commit_repository.rs:43-218` | `jarvis_executions`、`session_events`、`session_checkpoints`、`session_runtimes` | Daemon 执行 worker |
| 12 | `append_event` / `append_events` / `append_interaction_events` | `event_repository.rs:34-230` | `session_events`、`session_runtimes`、`jarvis_interactions` | Daemon 主 turn / 子代理 / 执行会话 / 迟到作答 |
| 13 | `quarantine_interrupted_executions` / `quarantine_interrupted_interactions` / `quarantine_host_executions` | `interrupted_execution_repository.rs:47-305`；`interrupted_interaction_repository.rs:21-233` | `jarvis_executions`、`jarvis_results`、`jarvis_goals`、`jarvis_events`、`jarvis_interactions`、`session_events`、`session_runtimes` | Daemon supervisor tick、启动恢复 |
| 14 | `upsert_schedule` / `claim_due_schedules` / `settle_schedule` | `schedule_repository.rs:28-88` | `runtime_schedules` | Daemon scheduler tick |
| 15 | `InteractionStore::append`（网关 ingress） | `interaction_repository.rs:258-283`；调用方 `crates/axon-daemon/src/application/interaction_ingress.rs:207`、`:233` | `jarvis_interactions` | Daemon 网关 HTTP/WS、外部事件 |
| 16 | `claim_processing` / `claim_processing_by_id` / 续租 / `update_status` | `interaction_repository.rs:285-620` | `jarvis_interactions` | Daemon |
| 17 | `append_agent_message` 系列 | `agent_mailbox_repository.rs:34-390` | `jarvis_agent_messages` | Daemon / CLI run（mailbox） |
| 18 | `append_continuation` 系列 | `agent_continuation_repository.rs:27-290` | `jarvis_continuations` | Daemon / CLI run（子代理收口） |
| 19 | `register_client_presence` / `unregister_client_presence` / completion 系列 | `client_delivery_repository.rs:29-480` | `jarvis_client_presence`、`jarvis_completion_notifications`、`jarvis_completion_deliveries` | Daemon |
| 20 | `append_decision` | `decision_repository.rs:9-86` | `jarvis_decisions` | Daemon / CLI run |
| 21 | `append_ledger_event` / `GoalStore` / `GoalCommitStore` / `LedgerCommitStore` | `ledger_event_repository.rs:9-124`；`goal_repository.rs:13-390`；`goal_commit_repository.rs`；`ledger_commit_repository.rs:52-182` | `jarvis_events`、`jarvis_goals`、`jarvis_executions`、`jarvis_results` | Daemon（执行 worker 结算）、CLI run |
| 22 | `ResultStore::acknowledge_results` / `confirm_results` | `result_repository.rs:40-149` | `jarvis_result_reads`（`jarvis_results` 的过期由 `expire_results` `:54`） | Daemon / CLI |
| 23 | `UserQuestionStore::insert_question` 系列 | `user_question_repository.rs:14-290` | `jarvis_user_questions` | Daemon（ask_user_question 工具） |
| 24 | `WorkspaceStore::register_workspace` | `workspace_repository.rs:12-50` | `jarvis_workspaces` | Daemon / CLI run |
| 25 | `SessionIngressCursorStore::save_ingress_cursor` | `helpers.rs:197-207` | `transport_ingress_cursors` | Daemon 外部事件 ingress |
| 26 | `save_goal_result_document` | `goal_result_document.rs:11-53` | 落盘文件（非表），返回绝对路径 | Daemon / CLI |
| 27 | `checkpoint_wal`（PRAGMA 写，不写表） | `sqlite_runtime_store.rs:205-216` | — | Daemon supervisor 每 5 分钟 |
| 28 | Cron 清理：归档 + 删除 + 悬空引用修复 + VACUUM | `crates/axon-cli/src/cron_commands/runtime_cleanup.rs:121-681`；`runtime_event_archive.rs:65-334`；`runtime_cleanup_session_events.rs:194`；`session_cleanup_sqlite.rs:16-31` | 全部业务表（删）、`session_events`（改/删） | Cron |
| 29 | CLI run 写入 `jarvis_results` | `crates/axon-cli/src/run_commands/run_request_payload.rs:545`、`:641` | `jarvis_results` | CLI `axon run` |
| 30 | CLI 侧 runtime store 单例（工作台 HTTP 适配器） | `crates/axon-cli/src/workbench_http_adapter.rs:162`（`entry(...).or_insert_with(SqliteRuntimeStore::from_root)`） | 读为主；写入经各 trait 方法 | `axon serve` 工作台 HTTP |

### 3.2 `session_events` 的追加路径（重点）

**唯一常规 INSERT 语句**：`crates/axon-session/src/infrastructure/sqlite_runtime_store/event_repository.rs:153`，由 `append_event_refs`（`:100-230`）承载，三个公开端口：

| 端口 | 定义 | 每批行数 | 事件类型来源 |
| --- | --- | --- | --- |
| `append_event` | `event_repository.rs:34-54`（单条包装 `append_event_refs`，`std::iter::once`） | 1 | 调用方给定 |
| `append_events` | `event_repository.rs:57-77`（`events: &[(String, String)]`，空批直接返回） | N（调用方传入切片长度） | 调用方给定 |
| `append_interaction_events` | `event_repository.rs:79-98`（额外校验 interaction 租约） | N | 调用方给定 |

`append_interaction_events` 额外做租约校验的 SQL：`event_repository.rs:116-121`（`status='processing' AND lease_token=?2 AND lease_expires_at_ms>?3`，不满足返回 `RuntimeStoreError::Fenced`）。

**事务边界**：三者共用同一个 `TransactionBehavior::Immediate` 写事务（`event_repository.rs:109-111`），事务内顺序为：

1. `ensure_fence`（`event_repository.rs:112` → `helpers.rs:126-136`）：对 `daemon_hosts` 做 no-op UPDATE，`changed==0` 即 `Fenced`；
2. interaction 租约校验（仅带租约时，`:113-125`）；
3. 读 `session_runtimes.last_event_sequence`（`:126-134`）；
4. 逐条 `persist` 外置正文 → `INSERT session_events`（`:150-201`）；
5. `UPDATE session_runtimes` 推进水位与 owner（`:202-213`）；
6. 逐条 `UPDATE jarvis_interactions SET tool_summary`（仅带租约时，`:214-227`）；
7. `transaction.commit()`（`:228`）。

**每批行数与写入批处理**：仓储层没有批量窗口/延迟；批次规模完全由调用方决定（见下）。仓储层唯一的「节流」是按 `payload_json.len()` 决定内联还是外置（见第 6 节）。

**调用方（谁在什么时机追加）**

| 调用点 | 位置 | 端口 | 批量 | 上下文 |
| --- | --- | --- | --- | --- |
| Primary Turn durable sink `commit_and_publish_slice` | `crates/axon-daemon/src/application/interaction_turn_event_sink.rs:469-479` | `append_interaction_events` | 单个 `Durable` 帧（`std::slice::from_ref`，`:207`）| Daemon 主 turn；只在语义边界落库 |
| Primary Turn 边界快照冲刷 `flush_live_parts` → `commit_durable_only` | `.../interaction_turn_event_sink.rs:309-351`、`:354-374` | `append_interaction_events` | 每个累积正文 part 1 行（`:324-346`）；空排空直接返回（`:319-321`） | Daemon 主 turn；工具调用前后、流结束、轮次收口等边界 |
| 用量快照补持久副本 `stage_replay_copy` → `flush_replay_copy` | `.../interaction_turn_event_sink.rs:524-536`、`:538-586` | `append_interaction_events` | 每条 provider 流 1 行（流收口、语义事件前、轮次边界各冲刷一次） | Daemon 主 turn；流内逐帧只做内存替换，同值快照跳过 |
| `InteractionTurnRunner` 逐帧提交 | `crates/axon-daemon/src/application/interaction_turn_runner.rs:299-309` | `append_interaction_events` | `payloads` 全部（该次收集到的帧） | Daemon 主 turn 运行器 |
| 执行会话 sink `ExecutionSessionSink::publish` | `crates/axon-daemon/src/application/execution_session_sink.rs:69-89` | `append_events` | 1 | Daemon 后台执行（goal/cron 执行会话） |
| 子代理会话 sink `ChildSessionSink::publish` | `crates/axon-daemon/src/application/child_session_event_sink.rs:66-81` | `append_events` | 1 | Daemon 子代理会话（先 `create_session` `:39-41`） |
| 执行 worker 结算（无 Ledger 时）`commit_conclusion` | `crates/axon-session-runtime/src/application/execution_worker_settlement.rs:185-191` | `append_event` | 1 | Daemon 执行 worker |
| 迟到作答 durable 写入 | `crates/axon-cli/src/daemon_host_runner_question_durable.rs:38-45` | `append_events` | 1 | Daemon 主循环之外（问答迟到） |
| `commit_action` 事务内批量事件 | `crates/axon-session/src/infrastructure/sqlite_runtime_store/commit_repository.rs:108` | 直接 SQL（走同一事务） | `events.len()` | Daemon 执行 worker |
| `fail_action` | `crates/axon-session/src/infrastructure/sqlite_runtime_store/failure_repository.rs:67` | 直接 SQL | 1 | Daemon |
| `recover_expired_sessions` | `crates/axon-session/src/infrastructure/sqlite_runtime_store/runtime_repository.rs:386` | 直接 SQL | 1/会话 | Daemon supervisor tick |
| 中断隔离补终态 | `.../interrupted_execution_repository.rs:269`、`.../interrupted_interaction_repository.rs:203` | 直接 SQL | 1 | Daemon supervisor tick / 启动恢复 |

**是否 IMMEDIATE**：是。常规追加（`event_repository.rs:110`）、`commit_action`（`commit_repository.rs` 经同一 store 连接开启事务）、`InteractionStore::append`（`interaction_repository.rs:266-268`）、`confirm_results`（`result_repository.rs:129-131`）、`interrupt_host_actions`（`sqlite_runtime_store.rs:324-326`）、`set_session_status`（`sqlite_runtime_store.rs:409-411`）均使用 `TransactionBehavior::Immediate`。schema 首次建库同样用 IMMEDIATE（`schema.rs:71`）。

**是否有节流或批处理窗口**：

- 仓储层：无时间窗、无定时 flush、无写入队列。每次 `append_*` 调用即一次 IMMEDIATE 事务 + 一次 `commit()`。
- 帧级节流在上层 `RuntimeTurnEventSink::dispatch`（`crates/axon-daemon/src/application/interaction_turn_event_sink.rs:313-368`）：按 `classify_frame`（`crates/axon-daemon/src/application/interaction_turn_event_frame_class.rs:45-80`）把帧分成四类：
  - `LivePart`（`reasoningDelta`、`delta=true` 的 `assistantMessage`）→ 只进内存累积缓冲（`:322-324`、`:421-449`）；
  - `LiveOnly`（`providerStatus`）→ 只走实时通道，`remember=true` 时留可补发副本（`:325-330`、`:404-419`）；
  - `Snapshot`（`usageUpdate`）→ 逐帧走实时通道，持久副本只在流收口/边界冲刷一次（`:331-336`、`:524-586`）；
  - `Durable` → 先 `retire_part_for` + `flush_live_parts` + `flush_replay_copy`，再 `commit_and_publish_slice`（`:337-357`）。
- 因此每个 turn 的 `session_events` 行数 ≈ 语义事件数 + 正文 part 快照数 + 每条 provider 流一条用量快照，与 token/增量帧数解耦（设计说明见 `.../interaction_turn_event_sink.rs:1-7`）。
- 节流计数有两处：「同值用量快照不重复写」与「同一条流内后到的用量帧覆盖先到的」（`.../interaction_turn_event_sink.rs:538-586`）。

### 3.3 三条运行形态下的写入归属

**Daemon 主循环（`axon daemon`，后台 4 个 worker 线程）**

| Worker | 循环位置 | tick 周期 | 写入 |
| --- | --- | --- | --- |
| lease | `crates/axon-cli/src/daemon_host_runner_workers.rs:54-72` | `HOST_RENEW_INTERVAL_MS = HOST_LEASE_DURATION_MS/3 = 10_000ms`（`crates/axon-cli/src/daemon_host_runner.rs:74-75`） | `renew_host`（`daemon_hosts`） |
| primary turn | `.../daemon_host_runner_workers.rs:134-158` | `MAINTENANCE_TIMER_INTERVAL_MS = 1_000ms`（`crates/axon-cli/src/daemon_host_runner.rs:79`） | `claim_processing`、`append_interaction_events`、completion/presence |
| supervisor | `.../daemon_host_runner_workers.rs:172-209` | 1_000ms（同上） | `scheduler_tick`（`runtime_schedules`）、`supervisor_tick`（隔离/恢复）、`checkpoint_wal` 每 `WAL_CHECKPOINT_INTERVAL_MS = 5*60*1000`（`crates/axon-cli/src/daemon_host_runner_workers.rs:244`） |
| cron | `.../daemon_host_runner_workers.rs:213-227` | `CRON_TICK_INTERVAL_MS = 1_000ms`（`crates/axon-cli/src/daemon_host_runner.rs:82`） | `cron.sqlite3` + 到点触发清理/归档 |

批量上限：`SUPERVISOR_BATCH_LIMIT = 32`（`crates/axon-cli/src/daemon_host_runner.rs:78`）；调度租约 `SCHEDULE_LEASE_DURATION_MS = 30_000`（`:77`）；action 租约 `ACTION_LEASE_DURATION_MS = 120_000`（`:76`）；Host 租约 `HOST_LEASE_DURATION_MS = 30_000`（`:74`）。

**CLI `axon run`**

| 写入 | 位置 |
| --- | --- |
| 共享 runtime 库解析 | `crates/axon-cli/src/run_commands/run_agent_store.rs:12-15` |
| Interaction 追加/领取 | `crates/axon-cli/src/run_commands/interaction_adapter.rs:125-136`（`interaction_store`），使用 `InteractionStore` trait（`:144`） |
| mailbox 持久化 | `crates/axon-cli/src/run_commands/run_mailbox_persistence.rs:27-32`、`:144` |
| continuation 装配 | `crates/axon-cli/src/run_commands/run_continuation_assembly.rs:53` |
| 子代理 completion | `crates/axon-cli/src/run_commands/run_subagent_completion.rs:53`、`:68` |
| Result 落库 | `crates/axon-cli/src/run_commands/run_request_payload.rs:545`、`:641` |
| Ledger 上下文读 | `crates/axon-cli/src/run_commands/run_request_payload.rs:305-320` |
| 历史装载（读） | `crates/axon-cli/src/run_commands/daemon_history_loader.rs:32`、`:49`、`:67` |

CLI 侧**没有** `session_events` 的追加调用点（全仓 `append_event*` 命中里，CLI 侧仅 `crates/axon-cli/src/daemon_host_runner_question_durable.rs:39` 的问答帧）。

**Cron**

| 写入/删除 | 位置 |
| --- | --- |
| cron 库自身 | `crates/axon-cli/src/cron_commands/repository.rs:55-586` |
| runtime 清理入口 | `crates/axon-cli/src/cron_commands/session_cleanup.rs:82-202` |
| runtime 清理主体 | `crates/axon-cli/src/cron_commands/runtime_cleanup.rs:121-681` |
| 事件归档（先归档再删） | `crates/axon-cli/src/cron_commands/runtime_event_archive.rs:65-95`、`:100-139` |
| 悬空 payload_ref 修复 | `crates/axon-cli/src/cron_commands/runtime_cleanup_session_events.rs:194` |
| 全库 VACUUM | `crates/axon-cli/src/cron_commands/session_cleanup_sqlite.rs:16-31` |
| 会话族清理（thread 事件面 + 目录） | `crates/axon-cli/src/cron_commands/session_family_cleanup.rs` |
| 文件面清理 | `crates/axon-cli/src/cron_commands/session_cleanup_files.rs:49-117` |

---

## 4. 连接与 PRAGMA 配置

### 4.1 运行态库（`axon-session` 连接池：`control.sqlite3` 与 `outbox.sqlite3`）

| 事实 | 位置 |
| --- | --- |
| 连接池结构 `ConnectionPool { path, busy_timeout, idle: Mutex<Vec<Connection>> }` | `crates/axon-session/src/infrastructure/sqlite_runtime_store/connection_pool.rs:45-49` |
| 空闲连接上限 `MAX_IDLE_CONNECTIONS = 8` | `connection_pool.rs:16` |
| 写锁等待 `BUSY_TIMEOUT_MS = 1_000` → `DEFAULT_BUSY_TIMEOUT` | `connection_pool.rs:18-20` |
| 逐连接 PRAGMA 批次 | `connection_pool.rs:39-40`：`PRAGMA foreign_keys = ON; PRAGMA synchronous = NORMAL; PRAGMA journal_size_limit = 67108864; PRAGMA wal_autocheckpoint = 1000;` |
| 打开顺序：`create_dir_all(parent)` → `Connection::open` → `busy_timeout` → `execute_batch(PRAGMA)` | `connection_pool.rs:92-108` |
| `journal_mode = WAL`：只在首次建库路径确保一次（库级持久属性），不逐连接切换 | `schema.rs:50-63`（`ensure_wal_journal_mode`），调用点 `schema.rs:108` |
| 取用/归还（`PooledConnection` + `Drop` 归还） | `connection_pool.rs:76-90`、`:110-115`、`:118-144` |
| 连接池在 Store 内共享（每个库一份 `Arc<DatabaseHandle>`，内部持有 `ConnectionPool`） | `sqlite_runtime_store.rs:139-157` |
| 注入自定义 busy_timeout 的构造器 | `sqlite_runtime_store.rs:229-231`（`with_busy_timeout`） |
| 只读查询复用池连接 `with_read_connection` | `sqlite_runtime_store.rs:268` |
| WAL 被动回写 `PRAGMA wal_checkpoint(PASSIVE)`，回写结果经 `WalCheckpoint` 暴露 | `sqlite_runtime_store.rs:198-209` |

`synchronous` 取 `NORMAL` 而非 `FULL`：`FULL` 让每批流式帧都承担一次持久性代价，直接决定单帧延迟上限；`NORMAL` 下 `COMMIT` 不逐事务 fsync，只在 checkpoint 与显式同步点落盘，代价是断电可能丢失最近一小段已提交事务但库结构不损坏，崩溃恢复按「以日志为准回退水位」设计，可接受该语义弱化（判据见 `connection_pool.rs:21-38` 的常量文档）。


**是否只读打开**：运行态库（`control.sqlite3` / `outbox.sqlite3`）在 `axon-session` 内**没有**只读连接；`SqliteRuntimeStore` 只有可写连接池。Cron 清理侧对同一文件用 `SQLITE_OPEN_READ_ONLY` 打开做候选统计与过期会话扫描（`crates/axon-cli/src/cron_commands/runtime_cleanup.rs:594-604`，调用点 `:623`）。

**`auto_vacuum`**：运行态库并非从不设置 `auto_vacuum`。`ensure_incremental_auto_vacuum`（`schema.rs:75-90`，调用点 `schema.rs:113`）只对**空库**在建表之前设 `INCREMENTAL`；既有非空库保持 `NONE`，对它执行 `PRAGMA incremental_vacuum` 是空操作，要改只能整库 VACUUM。

### 4.2 其他库

| 库 | journal_mode | synchronous | busy_timeout | foreign_keys | journal_size_limit | auto_vacuum | 只读打开 | 位置 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `cron.sqlite3` | `WAL`（批次内） | `NORMAL` | 5_000ms | `ON` | 未设置 | 未设置 | 无 | `crates/axon-cli/src/cron_commands/repository.rs:347-372`（PRAGMA 批次 `:353-355`） |
| `usage.sqlite3` | `WAL`（批次内） | `NORMAL` | 250ms（`USAGE_BUSY_TIMEOUT_MS`） | `ON` | 未设置 | `INCREMENTAL`（`retention.rs:400-422`，含 VACUUM 重试 3 次） | 未设置 | `crates/axon-cli/src/usage_statistics/repository.rs:49-73`（PRAGMA 批次 `:60`、超时常量 `:25`） |
| `persona.sqlite3` | `WAL`（`ensure_wal`，`inline fn` 于 `:613-618`） | 未设置 | 写：`PERSONA_WRITE_BUSY_TIMEOUT_MS`；读：`PERSONA_READ_BUSY_TIMEOUT_MS` | 未设置 | 未设置 | 未设置 | 有（`open_read_only` `:224-234`，语义上复用同一 `Connection::open`） | `crates/axon-persona/src/infrastructure/sqlite_persona_store.rs:205-212`、`:228-232` |
| `post_run_governance.sqlite3` | 未设置（默认） | 未设置 | 2_000ms | 未设置 | 未设置 | `INCREMENTAL`（`:48`） | 无 | `crates/axon-cli/src/run_commands/run_post_run_governance_store.rs:45-52` |
| `mcp_tool_schemas.sqlite3` | 未设置 | 未设置 | 2_000ms | 未设置 | 未设置 | 未设置 | 无 | `crates/axon-mcp-runtime/src/runtime/mcp_schema_cache.rs:203-206` |
| `memory.db` | `WAL`（`pragma_update` + 回读校验） | 未设置 | `MEMORY_SQLITE_BUSY_TIMEOUT_MS` | 未设置 | 未设置 | 未设置 | 有独立只读打开所需路径常量 | `crates/axon-memory/src/store/store_schema.rs:187-196` |

其它 SQLite 连接点（非上述主库）：

| 连接点 | PRAGMA / 配置 | 位置 |
| --- | --- | --- |
| Daemon `system_http_adapter` 读 host lease | `busy_timeout(HOST_LEASE_BUSY_TIMEOUT)` | `crates/axon-cli/src/system_http_adapter.rs:204-206` |
| Cron 会话族清理 | `busy_timeout(SQLITE_BUSY_TIMEOUT)` + `PRAGMA incremental_vacuum(256)` | `crates/axon-cli/src/cron_commands/session_family_cleanup.rs:375`、`:389` |
| Cron 文件面清理（mcp schema 缓存行） | `busy_timeout(SQLITE_BUSY_TIMEOUT)` | `crates/axon-cli/src/cron_commands/session_cleanup_files.rs:245` |
| Cron 全库 VACUUM | `busy_timeout(2s)`，`VACUUM;` | `crates/axon-cli/src/cron_commands/session_cleanup_sqlite.rs:13`、`:90-97`、`:77-79` |
| governance 保留清理 | `PRAGMA incremental_vacuum(256)` | `crates/axon-cli/src/run_commands/run_post_run_governance_store.rs:205` |
| cron 历史清理 | `PRAGMA incremental_vacuum(256)` | `crates/axon-cli/src/cron_commands/repository.rs:288` |

**连接池实现总结**：全仓只有一处连接池实现（`axon-session` 的 `ConnectionPool`，`connection_pool.rs`）。其余库均为「每次操作 `Connection::open`」，无池化（依据：`auto_vacuum`/`busy_timeout` 的逐库命中点，及 `connection_pool` 模块仅被 `sqlite_runtime_store.rs:32-33,72` 引用）。

---

## 5. 读侧 API 面

### 5.1 `RuntimeStore` trait（`crates/axon-session/src/ports/runtime_store.rs`）

trait 头（继承的 15 个 supertrait + `Send + Sync`）：`runtime_store.rs:55-73`

```text
RuntimeStore: AgentMailboxStore + AgentContinuationStore + GoalStore + GoalMutationStore
  + GoalCommitStore + DecisionStore + ExecutionStore + InteractionStore + InteractionQueryStore
  + ClientDeliveryStore + ResultStore + LedgerEventStore + LedgerCommitStore
  + WorkspaceStore + HealthStore + Send + Sync
```

本 trait 自带的方法（20 个）与方法签名行号：

| 方法 | 签名行 | 说明 |
| --- | --- | --- |
| `initialize()` | `:75` | 初始化 schema |
| `save_goal_result_document(session_id, execution_id, content) -> String` | `:77-82` | 返回绝对路径 |
| `claim_host(host_id, pid, process_start_id, now_ms, lease_duration_ms)` | `:84-91` | |
| `renew_host(host, now_ms, lease_duration_ms)` | `:93-98` | |
| `request_upgrade(host, handoff_token, target_version, deadline_at_ms, now_ms)` | `:100-107` | |
| `mark_handoff_ready(host, handoff_token, now_ms)` | `:109-114` | |
| `interrupt_host_actions(host, now_ms) -> u32` | `:118-122` | |
| `claim_upgrade_replacement(host_id, pid, process_start_id, handoff_token, now_ms, lease_duration_ms)` | `:124-132` | |
| `create_session(session_id, cwd, now_ms)` | `:134-139` | |
| `stop_session(session_id, host, now_ms)` | `:141-146` | |
| `reset_session(session_id, cwd, host, now_ms)` | `:148-154` | |
| `get_session(session_id)` | `:156-159` | |
| `recover_expired_sessions(host, now_ms, limit)` | `:161-166` | |
| `set_session_status(session_id, host, status, now_ms)` | `:168-174` | |
| `enqueue_action(action, now_ms)` | `:176-180` | |
| `claim_next_action(session_id, host, now_ms, lease_duration_ms)` | `:182-188` | |
| `renew_action_lease(action_id, session_id, host, now_ms, lease_duration_ms)` | `:199-206` | |
| `list_pending_actions(session_id, limit)` | `:208-212` | |
| `cancel_action(action_id, session_id, now_ms)` | `:214-219` | |
| `list_runnable_execution_sessions(now_ms, limit)` | `:221-225` | |
| `update_action_status(action_id, session_id, host, status, now_ms)` | `:227-234` | |
| `fail_action(action_id, session_id, host, error, max_attempts, now_ms)` | `:236-244` | |
| `quarantine_interrupted_executions(host, now_ms, limit)` | `:248-253` | |
| `quarantine_interrupted_interactions(host, now_ms, limit)` | `:255-260` | |
| `commit_action(action_id, session_id, host, events, next_execution, settlement, checkpoint, now_ms)` | `:262-272` | |
| `append_event(session_id, host, event_type, payload_json, now_ms)` | `:274-281` | |
| `append_events(session_id, host, events, now_ms)` | `:283-289` | |
| `append_interaction_events(session_id, host, interaction_id, lease_token, events, now_ms)` | `:291-299` | |
| `replay_events(session_id, after_sequence, limit)` | `:301-306` | |
| `interaction_tail_window(session_id, interaction_id, limit)` | `:310-315` | |
| `upsert_schedule(schedule)` | `:317` | |
| `claim_due_schedules(host, now_ms, lease_duration_ms, limit)` | `:319-325` | |
| `settle_schedule(schedule_id, host, next_run_at_ms, now_ms, failed)` | `:327-334` | |

错误类型 `RuntimeStoreError`（5 变体）：`runtime_store.rs:20-37`。`CheckpointTelemetry`（5 字段）：`:40-52`。

trait 对象实现：`impl RuntimeStore for SqliteRuntimeStore`，`crates/axon-session/src/infrastructure/sqlite_runtime_store.rs:237-657`。

### 5.2 各消费方用到的读侧方法

**Gateway（daemon HTTP/WS，`crates/axon-daemon`）**

| 方法 | 调用位置 | 用途 |
| --- | --- | --- |
| `replay_events` | `crates/axon-daemon/src/infrastructure/gateway_replay.rs:176`、`:268`、`:499`；`gateway_bridge.rs:225` | durable 回放到订阅游标 |
| `interaction_tail_window` | `crates/axon-daemon/src/infrastructure/gateway_replay.rs:382` | 首屏取交互自身尾窗 |
| `get_session` | `crates/axon-daemon/src/infrastructure/gateway_replay.rs:231`；`gateway_bridge.rs:363` | 解析会话与尾序号 |
| 回放参数常量 | 单页 100（`gateway_replay.rs:23`）、字节预算 64 MiB（`:31`）、页数兜底 100_000（`:33`）、老库扫描上限 2000 页/32 MiB（`:39-41`） | |
| 事件类型常量 | `INTERACTION_STATUS_EVENT = "interaction.status"` `gateway_replay.rs:20` | |
| WS 路由 | `WS_PATH = "/gateway/v1/ws"` `crates/axon-daemon/src/infrastructure/gateway_websocket.rs:30` | |

**桌面端（`crates/axon-desktop`）**

桌面端**不直连数据库**，只经网关 HTTP/WS：

| 端点常量 / 用法 | 位置 |
| --- | --- |
| `EVENTS_PATH = "/gateway/v1/events"` | `crates/axon-desktop/src/conversation/wire.rs:11` |
| `WS_PATH = "/gateway/v1/ws"` | `crates/axon-desktop/src/conversation/wire.rs:13` |
| `LANES_PATH = "/api/v1/lanes"` | `crates/axon-desktop/src/conversation/wire.rs:15` |
| `SESSIONS_PATH = "/api/v1/sessions"` | `crates/axon-desktop/src/conversation/wire.rs:17` |
| `INTERACTIONS_PATH = "/api/v1/interactions"` | `crates/axon-desktop/src/conversation/wire.rs:19` |
| 历史帧：`{LANES_PATH}/{lane}/interactions?limit=`；`{INTERACTIONS_PATH}/{id}/events?after=&limit=&tail=` | `crates/axon-desktop/src/conversation/history.rs:223`、`:269` |
| 子代理消息 / 列表 | `crates/axon-desktop/src/conversation/subagent_message.rs:45`；`session_read.rs:73` |
| 提问对账 / 作答 | `crates/axon-desktop/src/conversation/question_sync.rs:178`；`answer_question.rs:41` |
| 作业列表 / 停止 / 输出 | `lane_jobs.rs:32`；`job_stop.rs:41`；`job_output.rs:67` |
| 会话中断 | `crates/axon-desktop/src/conversation/interrupt.rs:31` |
| 用量与费率：桌面端不读 `usage.sqlite3` | 声明 `crates/axon-desktop/src/usage.rs:4`；`crates/axon-desktop/src/pricing.rs:4` |
| 架构门禁禁止依赖 `axon-session`/`rusqlite` | 仓库 `AGENTS.md`「数据一律经网关」条；`tools/verify-architecture.ps1` |

**CLI（`crates/axon-cli`）**

| 用途 | 使用的方法 / 位置 |
| --- | --- |
| 会话历史装载 | `replay_events` 经 `crates/axon-cli/src/run_commands/daemon_history_loader.rs:32`、`:49`、`:67` |
| 工作台 HTTP `lanes` | `LaneQueryService::list_lanes`（`Connection` 级读）`crates/axon-cli/src/workbench_http_adapter.rs:184-185`、`:491` |
| 工作台 HTTP 交互事实 | 裸 SQL 读 `jarvis_interactions` `crates/axon-cli/src/workbench_http_lane_readings.rs:46-51` |
| 工作台 HTTP 最终正文 | 裸 SQL 读 `session_events`（`turn_completed`）`crates/axon-cli/src/workbench_http_lane_readings.rs:98-103` |
| 会话 run_session 解析 | 裸 SQL 读 `session_runtimes` `crates/axon-cli/src/workbench_http_lane_readings.rs:248` |
| 提问计数 | 裸 SQL 读 `jarvis_user_questions` `crates/axon-cli/src/workbench_http_adapter.rs:214` |
| Ledger 上下文 | `list_active_goals_for_scope` / `get_latest_decision` / `list_unread_results` `crates/axon-cli/src/run_commands/run_request_payload.rs:309-320` |
| 子代理名册 | `child_session_rows`（thread 事件面）`crates/axon-cli/src/workbench_http_subagents_jobs.rs:59` |
| 作业唤醒读会话 | `LaneQueryStore` `crates/axon-cli/src/run_commands/run_shell_job_wake.rs:21`、`:65` |
| `axon serve`（headless）/v1/lanes、/v1/sessions | `crates/axon-cli/src/commands/serve.rs:150`、`:154`、`:187`、`:224` |
| usage 看板 / pricing | `UsageRepository` `crates/axon-cli/src/usage_statistics/repository.rs:53-73` |

**session-query（会话检索面）**

代码中没有名为 `session-query` 的 crate 或模块。对应的两层检索面为：

1. **CLI/daemon 的 workspace 读模型**：`LaneQueryStore` trait（`crates/axon-session/src/ports/lane_query_store.rs`，导出见 `crates/axon-session/src/ports/mod.rs:57`），实现 `crates/axon-session/src/infrastructure/sqlite_runtime_store/lane_query_repository.rs:48`（SQL 在 `:19`），消费方 `crates/axon-cli/src/workbench_http_adapter.rs:184-185`。
2. **Gateway durable replay**：`replay_events` + `interaction_tail_window`（第 5.2 节 gateway 行）。

另有一处 `usage --session <id>` 的过滤查询：`crates/axon-cli/src/tests/usage_cli_tests.rs:124-148`（读 `usage.sqlite3`，非 runtime 库）。

### 5.3 其他端口 trait（`crates/axon-session/src/ports/`）

| trait | 文件 |
| --- | --- |
| `AgentContinuationStore` | `ports/agent_continuation_store.rs` |
| `AgentMailboxStore` | `ports/agent_mailbox_store.rs` |
| `ClientDeliveryStore` | `ports/client_delivery_store.rs` |
| `DecisionStore` | `ports/decision_store.rs` |
| `ExecutionStore` | `ports/execution_store.rs` |
| `GoalCommitStore` | `ports/goal_commit_store.rs` |
| `GoalStore` / `GoalMutationStore` | `ports/goal_store.rs` |
| `HealthStore` | `ports/health_store.rs` |
| `SessionIngressCursorStore` | `ports/ingress_cursor_store.rs` |
| `InteractionStore` / `InteractionQueryStore` | `ports/interaction_store.rs` |
| `LaneQueryStore` | `ports/lane_query_store.rs` |
| `LeaseStore` | `ports/lease_store.rs` |
| `LedgerCommitStore` | `ports/ledger_commit_store.rs` |
| `LedgerEventStore` | `ports/ledger_event_store.rs` |
| `ResultStore` | `ports/result_store.rs` |
| `ScheduleStore` | `ports/schedule_store.rs` |
| `SessionActionStore` | `ports/session_action_store.rs` |
| `SessionEventStore` | `ports/session_event_store.rs` |
| `UserQuestionStore` | `ports/user_question_store.rs` |
| `WorkspaceStore` | `ports/workspace_store.rs` |
| `ThreadEventStore` | `ports/event_store.rs` |

导出清单：`crates/axon-session/src/ports/mod.rs:32-66`。

---

## 6. 会话事件的外部化与归档

### 6.1 内联阈值

| 常量 | 值 | 位置 | 语义 |
| --- | --- | --- | --- |
| `SESSION_EVENT_INLINE_PAYLOAD_LIMIT_BYTES` | `4 * 1024`（4096 B） | `crates/axon-session/src/infrastructure/sqlite_runtime_store.rs:102` | 非流式事件的正文内联上限；超过则外置 |
| `EVENT_TYPE_WIRE` | `"wire_event"` | `.../sqlite_runtime_store/event_payload_store.rs:31` | 流式帧事件类型 |
| `WIRE_EVENT_INLINE_PAYLOAD_LIMIT_BYTES` | `0` | `.../event_payload_store.rs:33` | 流式帧一律外置 |
| 选择函数 | `inline_limit_for(event_type)` | `.../event_payload_store.rs:180-186` | `wire_event` → 0；其余 → 4096 |
| 判定条件 | `payload_json.len() <= inline_limit_for(...)` → 返回 `None`（留在库内） | `.../event_payload_store.rs:50-52` | |

### 6.2 目录布局与文件名规则

| 事实 | 位置 |
| --- | --- |
| 归档根 = `<db 目录>/archive/session-events` | `crates/axon-session/src/infrastructure/sqlite_runtime_store.rs:222-227`（拼接 `EVENT_ARCHIVE_DIR` + `SESSION_EVENT_ARCHIVE_SUBDIR`） |
| 目录常量 `EVENT_ARCHIVE_DIR = "archive"` | `.../sqlite_runtime_store.rs:231` |
| 子目录常量 `SESSION_EVENT_ARCHIVE_SUBDIR = "session-events"`（`pub`，供清理任务排除） | `.../sqlite_runtime_store.rs:233` |
| 分片跨度 `EVENT_PAYLOAD_SHARD_ROWS = 2_000` | `.../event_payload_store.rs:29` |
| 分片相对路径 `"{session_id}/{shard:05}.jsonl"`，`shard = sequence / 2000` | `.../event_payload_store.rs:53-57` |
| 引用格式 `"{relative}@{offset}+{len}"` | `.../event_payload_store.rs:72-75`；分隔符常量 `REFERENCE_SPAN_SEPARATOR = '@'` `:35`、`REFERENCE_LENGTH_SEPARATOR = '+'` `:37` |
| 追加方式 | `OpenOptions::new().create(true).append(true)`，先 `metadata().len()` 取偏移，再 `write_all(payload)` + `write_all(b"\n")`：`.../event_payload_store.rs:64-71` |
| 写文件失败时的降级 | 返回 `None` → 正文内联落库（`.../event_payload_store.rs:41-42`、`:50-52`、`:59-75` 的 `.ok()?` 链） |
| 外置成功的行内联列清空 | `crates/axon-session/src/infrastructure/sqlite_runtime_store/event_repository.rs:167-172`（`inline_payload = ""`） |
| 历史单文件格式兼容 `{session}/{sequence}.json` | 读取时若无 `@` 即按整文件读：`.../event_payload_store.rs:83-85`；测试 `:262-277` |
| Windows verbatim 安全路径解析 `payload_storage_path` | `.../event_payload_store.rs:121-128` |
| 引用是否独占文件 `is_exclusive_payload_reference` | `.../event_payload_store.rs:134-136`（无 `@` 即独占） |
| 读侧降级：任何读取失败只把该帧降级为占位事件 | `.../event_repository.rs:288-315`（事件类型 `SESSION_EVENT_MISSING_ARCHIVE_TYPE = "archive_missing"`，`.../sqlite_runtime_store.rs:235`） |

### 6.3 回收逻辑

| 路径 | 触发 | 位置 |
| --- | --- | --- |
| 整片回收 `reclaim_shards_before(archive_root, session_id, min_live_sequence)` | 删行事务提交后，按会话取 `MIN(sequence)` 作保底线；`shard_end <= min_live` 或会话已无存活行则删整片 | `.../event_payload_store.rs:146-178`；调用点 `crates/axon-cli/src/cron_commands/runtime_event_archive.rs:286-305` |
| 独占引用文件回收 `reclaim_exclusive_payloads(archive_root, refs)` | 只对 `deleted` 集合中确认为独占引用的行删除文件，并清理空会话目录 | `crates/axon-cli/src/cron_commands/runtime_event_archive.rs:233-261`（`remove_file` `:254`，`remove_dir` `:258`） |
| 会话族整体回收 `reclaim_session_archive_dir` | 会话家族被删除后清空 `<archive>/<session_id>` 目录（保留 `.jsonl.zst`） | `crates/axon-cli/src/cron_commands/runtime_event_archive.rs:264-276` |
| 悬空引用修复 | 读取失败的行改为内联占位并置 `payload_ref = NULL` | `crates/axon-cli/src/cron_commands/runtime_cleanup_session_events.rs:194`；入口 `crates/axon-cli/src/cron_commands/runtime_cleanup.rs:197-201`（`repair_dangling_payload_refs`） |
| 文件保留期不误删外置目录 | `archive` 目录递归清理时显式排除 `session-events` 子目录 | `crates/axon-cli/src/cron_commands/session_cleanup_files.rs:104-109` |

### 6.4 `runtime_event_archive`（`.jsonl.zst` 归档）

| 事实 | 位置 |
| --- | --- |
| 模块文件 | `crates/axon-cli/src/cron_commands/runtime_event_archive.rs` |
| 归档前删除的硬约束（先归档再删行） | `runtime_event_archive.rs:64-95`、`:428-457`（测试）；主流程 `crates/axon-cli/src/cron_commands/runtime_cleanup.rs:164-193` |
| 归档文件路径 `<archive_root>/<session_id>.jsonl.zst` | `runtime_event_archive.rs:190-195` |
| 写入方式：按 session 分组、追加打开、每行一个 JSON、段末 `encoder.finish()` | `runtime_event_archive.rs:179-216` |
| zstd 压缩级别 `ARCHIVE_ZSTD_LEVEL = 3` | `runtime_event_archive.rs:35` |
| 每批行数 `ARCHIVE_BATCH_ROWS = 2_000` | `runtime_event_archive.rs:33` |
| 归档行字段 | `sessionId`、`sequence`、`eventType`、`emittedAtMs`、`payload`：`runtime_event_archive.rs:200-206` |
| 正文解析：有 `payload_ref` 则从外置文件读，失败写 `<正文归档不可读：{reference}>` | `runtime_event_archive.rs:218-225` |
| 删除回执：单条 `DELETE ... RETURNING rowid` | `runtime_event_archive.rs:313-334` |
| 触发 1：保留期过期（`session_event_expiry_filter()`） | `runtime_event_archive.rs:65-95`（选择 SQL `:71-74`） |
| 触发 2：磁盘高水位淘汰 | `runtime_event_archive.rs:100-139` |
| 磁盘上限 `RUNTIME_MAX_DISK_BYTES = 4 GiB` | `runtime_event_archive.rs:25` |
| 高水位 `RUNTIME_HIGH_WATER_BYTES = 4 GiB / 10 * 8`（3.2 GiB） | `runtime_event_archive.rs:27` |
| 低水位 `RUNTIME_LOW_WATER_BYTES = 4 GiB / 2`（2 GiB） | `runtime_event_archive.rs:29` |
| 水位淘汰最小事件年龄 `WATERMARK_MIN_AGE_MS = 1 小时` | `runtime_event_archive.rs:31` |
| 水位淘汰排除运行中 lane（`status IN ('running','recovering')`） | `runtime_event_archive.rs:112-116` |
| 磁盘占用口径：主库 + `-wal` − `page_size * freelist_count` | `runtime_event_archive.rs:340-354`；`PRAGMA page_size` `:345`，`PRAGMA freelist_count` `:348` |
| 归档根与被外置正文共用 | `runtime_event_archive.rs:59-62`（`event_archive_root(database_path)`） |
| 清理任务标识 | 由 `axon.internal.brain-cleanup` / `axon.system.brain-cleanup` 触发，见 1.3 节末 4 条 |

---

## 7. crate 依赖关系

### 7.1 `axon-session` 的直接依赖（`crates/axon-session/Cargo.toml:4-19`）

| 依赖 | 声明位置 |
| --- | --- |
| `axon-crypto`（workspace） | `crates/axon-session/Cargo.toml:10` |
| `axon-domain`（path `../axon-domain`） | `crates/axon-session/Cargo.toml:11` |
| `axon-jsonl`（path `../axon-jsonl`） | `crates/axon-session/Cargo.toml:12` |
| `axon-process`（path `../axon-process`） | `crates/axon-session/Cargo.toml:13` |
| `axon-time`（path `../axon-time`） | `crates/axon-session/Cargo.toml:14` |
| `fs4`（workspace，文件锁） | `crates/axon-session/Cargo.toml:15` |
| `rusqlite`（workspace） | `crates/axon-session/Cargo.toml:16` |
| `serde` / `serde_json` / `thiserror` / `uuid` | `crates/axon-session/Cargo.toml:17-20` |
| dev：`axon-test-support`、`tempfile` | `crates/axon-session/Cargo.toml:26-28` |

### 7.2 谁依赖 `axon-session`

| crate | 声明位置 | 具体使用点（示例） |
| --- | --- | --- |
| `axon-cli` | `crates/axon-cli/Cargo.toml:38` | `crates/axon-cli/src/run_commands/run_agent_store.rs:9`；`crates/axon-cli/src/daemon_host_runner.rs:182`；`crates/axon-cli/src/cron_commands/session_cleanup.rs:195`；`crates/axon-cli/src/workbench_http_lane_readings.rs:114`（`read_event_payload`） |
| `axon-daemon` | `crates/axon-daemon/Cargo.toml:15` | `crates/axon-daemon/src/application/interaction_turn_event_sink.rs:12`；`crates/axon-daemon/src/application/execution_session_sink.rs:19`；`crates/axon-daemon/src/application/child_session_event_sink.rs:14`；`crates/axon-daemon/src/infrastructure/gateway_replay.rs:16` |
| `axon-session-runtime` | `crates/axon-session-runtime/Cargo.toml:12` | `crates/axon-session-runtime/src/application/execution_worker_settlement.rs`（`store.append_event` `:185`） |

间接依赖：`axon-tui-runtime` 依赖 `axon-session-runtime`（`crates/axon-tui-runtime/Cargo.toml:19`），因此经 `axon-session-runtime` 间接依赖 `axon-session`；`axon-desktop` 不依赖 `axon-session`（依据：全仓 `Cargo.toml` 中 `axon-session = { path = ... }` 形式的依赖声明仅 3 处，见上表；`crates/axon-desktop/Cargo.toml` 无该条目）。

### 7.3 依赖方向

```text
                         axon-crypto   axon-domain   axon-jsonl   axon-process   axon-time
                              \            |             |            |           /
                               \           |             |            |          /
                                +-------- axon-session --------+------+
                                               ^      ^      ^
                                               |      |      |
                        axon-cli --------------+      |      |
                             |                        |      |
                             v                        |      |
                        axon-daemon ------------------+      |
                                                             |
                        axon-session-runtime -----------------+
                             ^
                             |
                        axon-tui-runtime

注：axon-cli / axon-daemon 之间无 axon-session 相关依赖边；
    axon-desktop 经 HTTP/WS 网关访问运行态，不依赖 axon-session。
```

依据：

- `axon-session` → 5 个内部 crate：`crates/axon-session/Cargo.toml:10-14`
- `axon-cli` → `axon-session`：`crates/axon-cli/Cargo.toml:38`
- `axon-daemon` → `axon-session`：`crates/axon-daemon/Cargo.toml:15`
- `axon-session-runtime` → `axon-session`：`crates/axon-session-runtime/Cargo.toml:12`
- `axon-tui-runtime` → `axon-session-runtime`：`crates/axon-tui-runtime/Cargo.toml:19`

### 7.4 存储层归属（crate → 存储单元）

| crate | 拥有的持久化 |
| --- | --- |
| `axon-session` | `control.sqlite3` 与 `outbox.sqlite3` schema 与全部仓储；`data/sessions/threads/*.jsonl`（thread 事件）；`data/archive/session-events/**`（外置正文） |
| `axon-cli` | `cron.sqlite3`；`usage.sqlite3`；`post_run_governance.sqlite3`；清理与 `.jsonl.zst` 归档；`channel-metrics`、资源遥测 JSONL；`update.json`、`daemon.pid`、`daemon.stop` |
| `axon-daemon` | `data/gateway/auth-keys.json`、`data/logs/gateway-keys-audit.jsonl`；不拥有独立 SQLite 库（复用 `axon-session` 的运行态库） |
| `axon-persona` | `data/persona/persona.sqlite3` |
| `axon-memory` | `data/memory/memory.db`、`tantivy/`、`hnsw.idx`、工作区产物 |
| `axon-memory-ops` | `memory_sync_audit.jsonl` |
| `axon-memory-write` | `write_pipeline` 锁文件 |
| `axon-mcp-runtime` | `cache/mcp_tool_schemas.sqlite3`、`mcp-oauth/tokens`、`mcp_registry_health_negative.json` |
| `axon-agent-runtime` | `search-index/index.json`、`session_index.jsonl`、`data/archive`（内容归档）、`data/attachments`、`data/logs/tee/*.shell.log`、自迭代/反思 JSONL |
| `axon-jsonl` | 通用分片 JSONL 存储原语（被 `axon-session` 复用） |
| `axon-desktop` | `~/.axon-desktop`（连接配置、密钥库引用）、`draft_history.jsonl`、`relaunch-failed*.json`；不拥有运行态 SQLite |
| `axon-tui-runtime` | `terminal_agent_pane_turn_events.jsonl`、`axon-tui-state-dumps/` |
| `axon-apply` | `.axon-apply-patch-txn*` 事务目录 |

---

## 附录 A：运行态库打开点汇总（`control.sqlite3` / `outbox.sqlite3`）

| 打开点 | 位置 | 用途 |
| --- | --- | --- |
| `SqliteRuntimeStore::from_root`（通用） | `crates/axon-session/src/infrastructure/sqlite_runtime_store.rs:287-289` | 所有按根目录解析的调用方 |
| `SqliteRuntimeStore::new`（按路径） | 同上 `:219-221` | 测试与显式路径调用方 |
| `SqliteRuntimeStore::with_busy_timeout` | 同上 `:229-231` | 注入更小写锁上限（有界失败验证） |
| Daemon 主链路 | `crates/axon-cli/src/daemon_host_runner.rs:182` | `axon daemon` |
| 共享 runtime 库（run 级协作事实） | `crates/axon-cli/src/run_commands/run_agent_store.rs:14` | CLI run |
| Interaction 适配 | `crates/axon-cli/src/run_commands/interaction_adapter.rs:136` | CLI run |
| 工作台 HTTP 单例 | `crates/axon-cli/src/workbench_http_adapter.rs:162` | `axon serve` / 工作台 |
| 调试 mailbox | `crates/axon-cli/src/commands/management/debug_mailbox.rs:137` | `axon debug` |
| 历史装载 | `crates/axon-cli/src/run_commands/daemon_history_loader.rs:32`、`:49`、`:67` | CLI 读侧 |
| Ledger 上下文 | `crates/axon-cli/src/run_commands/run_request_payload.rs:305` | CLI run |
| Cron 清理（可写） | `crates/axon-cli/src/cron_commands/session_cleanup.rs:195`（`from_root`） | Cron |
| Cron 清理（可写裸连接） | `crates/axon-cli/src/cron_commands/runtime_cleanup.rs:579-592` | Cron |
| Cron 清理（只读裸连接） | `crates/axon-cli/src/cron_commands/runtime_cleanup.rs:594-604` | Cron |

## 附录 B：事件类型常量的落点

| 常量 | 值 | 位置 |
| --- | --- | --- |
| 外置正文缺失占位类型 | `"archive_missing"` | `crates/axon-session/src/infrastructure/sqlite_runtime_store.rs:235` |
| 流式帧类型 | `"wire_event"` | `crates/axon-session/src/infrastructure/sqlite_runtime_store/event_payload_store.rs:31`；`crates/axon-cli/src/daemon_host_runner_question_durable.rs:21` |
| 工具名提取判据 `toolCall` | `.../event_repository.rs:237-250` | |
| 轮次终态事件 `turn_completed` | `.../interaction_turn_event_sink.rs:575`；常量来源 `axon_session_runtime::model::TURN_COMPLETED_EVENT`（`crates/axon-daemon/src/application/execution_session_sink.rs:22`） | |
| Interaction 状态帧 `interaction.status` | `crates/axon-daemon/src/infrastructure/gateway_replay.rs:20` | |
| 目标取消 ledger 事件 `goal.cancelled` | `crates/axon-session/src/infrastructure/sqlite_runtime_store/goal_repository.rs:178`、`:224` | |
| schema 身份 `jarvis-runtime-goal-v1` | `crates/axon-session/src/infrastructure/sqlite_runtime_store/schema.rs:9` | |
