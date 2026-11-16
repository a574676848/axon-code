# W2-12 专项：rollout 存储退役

> 目标：`RolloutSqliteStore` 与 `run_rollout_store/**` 全部删除，会话历史、轮次事实、窗口投影、
> 用量与保留策略都落在 `data/sessions/threads/*.jsonl` 与控制面库；删除 rollout 库后多轮对话
> 回归全绿、`--continue` 可接续。
>
> 依据：`docs/dev/storage-architecture.md` §12.5 曾把该项登记为「专门一轮 + 多轮对话回归」的
> 延后项。本轮已完成代码迁移并删除 rollout 库；剩真实多轮对话真机回归。

## 现状盘点（迁移前，生产引用）

rollout 对外能力共 25 个入口，按用途分六组：

| 组 | 能力 | 生产调用方 |
| --- | --- | --- |
| A 恢复与历史窗口 | `load_indexed_run_session`、`load_rollout_history_window` | `run_session_store.rs`、`run_thread_store.rs` |
| B 窗口投影写入 | `install_rollout_compaction`、`install_active_rollout_window`、`restore_rollout_compaction` | `run_thread_store.rs`、`run_provider_loop/inline_compaction.rs` |
| C 轮次事实 | `load_session_turn`、`load_turn_sequence`、`load_recent_turn_facts`、`load_previous_reflection_notice`、`load_latest_reflection_notice_from_root` | 治理 worker、收口产物、反思 |
| D 用量与会话读数 | `load_session_usage`、`load_session_usage_totals_from_root`、`session_usage_totals`、`load_latest_session_model_from_root` | 桌面读数、TUI、成本看板 |
| E 子代理与链 | `mark_child_session_status`、`mark_child_session_origin`、`load_child_sessions_from_root`、`replay_session_chain`、`replay_rollout_events`、`read_rollout_tool_output` | 子代理视图、回放、工具输出 |
| F 保留与清理 | `stale_session_family_ids`、`finalize_stale_session_family_removal` | 会话清理链 |

目标落点：A/B → thread 文件；C → thread 轮次 + turn metadata；D → `session_metrics` 与控制面；
E → thread 子会话关系 + 事件面；F → 会话目录/thread 家族（已完成）。

## 已完成

1. **A 组迁移**：`prepare_run_session` 不再读 rollout。恢复目标先用 `ThreadService` 定位
   （`--session` 按 id，`--continue` 先读恢复指针，再回退最近未归档 thread），模型可见历史由
   `ThreadDetail` 直接装配；`PreparedRunSession` 的 `record: RunSessionRecord` 换成
   `facts: ResumedSessionFacts`（session_id / 父会话 / cwd / provider / model / 别名），
   全部来自 thread settings 与最近一轮 metadata。
2. **B 组收口**：活动窗口投影改为只写 thread（`install_active_compaction_projection` 的
   `through_sequence` 由 thread 当前轮推导），删除 rollout 侧的窗口镜像与两条门面；
   `publish_active_window` 不再维护第二份镜像（thread 压缩快照就是活动窗口）。
   窗口边界语义定案：`through_sequence` 是**首个仍然可见的轮次**，恢复侧保留
   `sequence >= through_sequence`，已同步 `storage-architecture.md`。
3. **C 组收口（轮次事实）**：治理 worker 与 `builtin.turn_trace` 不再读 rollout。
   - `run_thread_store::turn_record` / `turn_record_by_sequence`：按
     `(thread, assistant_part_id)` 或轮序，把 thread 轮次反向重建为整条
     `RunSessionRecord`（`run_thread_history::turn_item_projection` 是历史投影的逆运算：
     注入上下文、用户输入、工具调用与回执、reasoning、助手响应逐个还原；用量、模型
     别名、推理强度、失败标记来自轮次 metadata）。
   - 回执反投影保留 `provider_call_id` / `output_ref` / `part_id` / 错误类型 / 写盘标记，
     `builtin.turn_trace` 因此照样能按 provider 调用读归档原文。
4. **治理产物回写改点对点**：`publish_turn_artifacts` 不再整条重写记录，改为
   `run_thread_store::patch_turn_artifacts`：只补写本轮 metadata 的 `reflectionNotice` 与
   `affectSequence`，其余原样保留；反思回执仍单独入表。`turn_trace` 不再落库：它只被写、
   从不被读回，读取方一律按轮次事实重建。
5. **D 组（用量与会话读数）**：用量事实源改为 thread 轮次 metadata。
   - `ThreadTurnMetadata` 新增 `upstreamModel`、`requestPath`、`speedMode`、`ttftMs`、
     `generationSpeedTps`、`providerCallCount`、`providerDurationMs`，投影器不再反序列化整条
     轮次正文。
   - `usage_statistics` 的水位改为「上次投影到的记录时间（毫秒）」，同毫秒边界重读一次、
     事实按 `(sessionId, assistantPartId)` 幂等覆盖；`usage` 页面的 `stale` 依据 thread 事件
     文件的最新写入时间判定。
   - `run_provider_turn` 的 token 用量读数改走 `latest_turn_usage_tokens_at_root`。
6. **E 组（子代理、链回放、工具归档）**：
   - 名册：`run_thread_children::child_session_rows` 由 `ThreadSummary`（父会话、标题、模型、
     末轮状态与时间）直接给出；`ThreadSummary` 因此新增 `lastTurnStatus` / `lastTurnAtMs`。
   - 来源：新增 `ThreadAnnotated` 事件与 `Thread.childOrigin`、`annotate_child_origin`，
     内务子代理不再进名册；缺省来源按委派处理（兼容旧日志）。
   - 回放：`run_thread_replay::replay_session_chain` 沿 `parent_thread_id` 回溯 resume 链；
     `replay_session_turns` 给出轮次级摘要（状态、时间、turn 事件数、回执数、反思提示）；
     `read_task_output` 按 `(thread, providerCallId)` 点查回执并读内容归档。
   - `axon debug session replay` 的 JSON 输出字段随之改为 `chain` / `turnCount` / `turns` /
     `toolOutput`，同时更新 `operate-axon` Skill 与安装态副本。
   - 子代理起跑/收尾补写 thread 标题，名册标题不再依赖 rollout。
7. **F 组（保留与清理）**：`stale_session_family_ids` 只按 thread 家族判定：
   `scan_threads` 给出会话、父子关系与最近活动时间；当前 `--continue` 指针所在的家族、
   治理队列活跃作业与运行态活跃会话整族保护；清理顺序改为「先删会话上下文、后删 thread
   文件」，上下文删除失败时 thread 仍是下一轮的重试入口。`vacuumed_rollout` 报告字段删除。
8. **整删**：`crates/axon-cli/src/run_commands/run_rollout_store/**` 与
   `resolved_rollout_db_path`、`RolloutSqliteStore`、`commit_*_to_rollout` 全部删除；
   `usage_statistics` 不再读 SQLite 源；相关测试改为 thread 事件面夹具，并补
   `current_resume_session_family_is_protected` 与六条子代理名册用例。
9. **文档同步**：`storage-architecture.md` 的会话正文/用量/数据布局/延后项四处改口径；
   `wbs-acceptance.md` 的 W2-12 行指向本文件。

## 门禁证据

- `cargo fmt --all --check` 绿。
- `cargo check --workspace --all-targets` 绿。
- `cargo clippy --workspace --all-targets -- -D warnings` 绿。
- `cargo test --workspace` 全绿（58 个测试目标、0 失败；`axon-cli` 1163 passed / 12 ignored）。
- `tools/verify-architecture.ps1` 绿（含文件行数硬限与文档校验）。
- `tools/verify-docs.ps1` 绿。

## 剩余（真机回归）

安装后跑 C2/C4/C5（`--session`、`--continue`、同 cwd 连续 run）与 X1-X7 跨端续聊，
把回执登记到 `docs/dev/device-verification.md`；`inline_compaction_tests` 里仍有一个
`#[ignore]` 的两轮换窗用例，需在真机回归同批打开。
