# 会话真相链路整改 WBS（六问题）

本文件是一次真机卡死的整改计划。现场是 2026-11-21 夜间到 2026-11-22 凌晨的一段会话：桌面端长期停在「正在连接供应商…」，子代理一直显示在跑，点开子代理详情空白，头部上下文字段停在旧值，历史过程行大面积不折叠。本文件先给取证结论（每条都能指回代码与数据），再给任务拆解与验收判据。

实施按本文件的 WP 逐项推进，每完成一项在《WBS 验收台账》（`wbs-acceptance.md`）登记状态与命令回执。

## 1 现场与取证

环境：本机安装态二进制 `axon-cli 0.0.1`（版本目录写入时间 2026-11-20 22:47），仓库 HEAD `2cf1ffef2`（2026-11-21 15:19），**运行版本早于 HEAD 约 17 小时**。取证时守护进程已按指令停止。

关键数据（`$HOME/.axon/data`）：

| 事实 | 数值 |
| --- | --- |
| 主会话 `01M7YFWWXS9Y28RGCP7BWNN4A2` 事件条数 | 6231（序号 1..6231 齐全） |
| 其中序号逆序处 | 28（例如 56,58,59,57,60） |
| 该会话最后一条事件 | `turn_uncertain`，executionId `a3870cba-…` |
| 卡住的交互 `01M7Z2GMB08ZFRQWHW74AV3G5N` | 1899 帧、74 轮流式、221 条子代理活动、179607 字符增量，**全部是 reasoning** |
| 子代理会话 `01M7Z4BF1C8CR9N3Q9H3N9CCV7` | 145 条事件、序号 1..144 连续，**145 条全部 `interactionId=null`** |
| 子代理 Thread `7af30cd5-…` | `status: Active`，Turn 0 `completed_at_ms: -`，`assistant_chars: 0` |
| 压缩记录 | `before_tokens=825338 after_tokens=31733` |
| 守护进程更换 | 21:25:49 拉起新 host `30936`，`upgrade_handoffs` 为空（不是升级排空），旧 host `72072` 租约 21:26:09 到期 |

全量扫描 17 个会话事件日志后的分布（同一段只读探针）：

| 结果 | 会话数 | 说明 |
| --- | --- | --- |
| 有序 | 13 | 含三个长跑子代理会话（465 / 476 / 144 条，零逆序） |
| 逆序 | 4 | `01M7YFWW…` 6230 条 28 处、`01J8XQ7K…` 546 条 6 处、两个 cron 会话各 13 条 3 处 |

逆序只出现在会写 `turn_metrics` 的主会话与 cron 会话上，纯子代理会话零逆序；这与「流式 sink 与收口 metrics 两条写路径并发」的候选机制吻合，也说明它不是偶发个案，而是每次轮次收口都可能触发的结构性缺陷。

取证命令（只读，均可复跑）：
```powershell
axon daemon health
axon sessions list --limit 50
axon sessions read 01M7YFWWXS9Y28RGCP7BWNN4A2
axon debug session doctor
sqlite3 $HOME/.axon/data/control.sqlite3 "select status,kind,count(*) from sessions group by 1,2;"
```

第 1 节的事件日志解码使用一次性只读探针（Python `zstandard`，写在 `$HOME/.axon/data/tmp/`），未改动任何仓库文件。

## 2 六问结论（逐条带证据）

### Q1 主会话是否还在跑

**没有。** 三条独立判据同向：`work_items` 无 `running` 行；`axon stop --session …` 回「没有正在运行的 Axon session」；`daemon health` 的 `executionBacklog.pending=0`、`executions.byStatus={completed:21, uncertain:1}`。停止前 `sessions` 表 17 条会话的状态只有 `idle / created / recovering`。

界面上的 9 小时计时不是进程在跑，而是「索引说这一轮在途」的纯前端推算（见 Q6）。

### Q2 为什么还有一个子代理在跑

**它没在跑，是「末轮状态」两个来源都没有收口。**

- 界面计数 `runningCount` 只认转录行的 `status ∈ running/starting/waiting`（`crates/axon-desktop/ui/src/components/conversation/RuntimeActions.tsx:409-416`）。
- 名册对账函数 `statusOf` 的第一条分支是「后端说 active 就沿用行内状态」（同文件 `:377-392`），因此后端那句 `active` 会**阻止**「按末轮状态纠正」的路径生效。
- 名册的 `status` 来自 `ChildSessionStatus`：`last_turn_status == "active"` 时给 `active`，否则 `idle`（`crates/axon-cli/src/workbench_http_subagents_jobs.rs:116-121`），而 `last_turn_status` 来自 **JSONL thread 派生索引**（`crates/axon-cli/src/run_commands/run_thread_children.rs:78-104`），不是运行态库。
- 该子代理 Thread 的末轮 `completed_at_ms: -`、`assistant_chars: 0`，永远停在 Active。
- 收口路径缺失：`recover_expired_sessions` 的候选集只有 `running/compacting/waiting`（`crates/axon-session/src/infrastructure/sqlite_runtime_store/runtime_repository.rs:448-451`），**`recovering` 一旦写入就没有任何路径再动它**。

### Q3 子代理会话详情为什么空白

**它的事件在落盘时没有 Interaction 归属，读写两侧都取不到。**

- 写入侧：子代理会话的 sink 调的是**不带归属**的 `append_events`（`crates/axon-daemon/src/application/child_session_event_sink.rs:116-125`），而不是 `append_attributed_events`。
- 读取侧：桌面端子代理视图先定位会话，再读该会话最后一条 Interaction，然后按 Interaction 订阅正文（`crates/axon-desktop/ui/src/components/conversation/SubagentRunView.tsx:100-115`）。
- 网关的读法是先按归属列取帧（`crates/axon-session/src/infrastructure/sqlite_runtime_store/event_log_repository.rs:178-198`，按交互指纹索引），取不到才退回载荷串扫描（`crates/axon-daemon/src/infrastructure/gateway_replay.rs:388-413`）。
- 该子代理 145 条帧既没有归属列，载荷里也没有 `interactionId` 字段，两条路都空，因此详情页无帧可渲染。

### Q4 头部上下文为什么不跟进换窗读数

**换窗与头部读的是两套来源，头部那套还会粘连旧值。**

- 换窗读数来自运行时自己的 `TokenUsage::current_context_tokens()`（`crates/axon-agent-runtime/src/runtime/token_usage.rs:225-233`，注释明确它是「换窗守卫与占用显示」的唯一来源），并写进换窗进度行：`正在开启新的记忆窗口 … 当前上下文 {before_tokens} tokens`（`crates/axon-agent-runtime/src/core/context/insert_then_compress.rs:574-585`）。本次现场 `before_tokens=825338`。
- 桌面头部读的是自己投影出来的「最后一个带用量观测的轮次」（`crates/axon-desktop/ui/src/components/conversation/ConversationView.tsx:338` 的 `lastObservedUsage`），字段是 `currentContextTokens`（`crates/axon-desktop/ui/src/conversation/metrics.ts:114-120`）。
- 粘连：新用量帧若不带 `currentContextTokens`，就沿用上一份（`crates/axon-desktop/ui/src/conversation/transcript/cells.ts:416-419`），而「流帧常常只报增量、不带窗口占用」是该文件自己写明的常态（同文件 `:521-523`）。

结论：两个数字不是同一个数据源，不一致是结构性的，不是偶发。

### Q5 历史过程行为什么大部分不折叠

**折叠只对「已收口」的轮次生效，而大部分轮次的收口事实缺失。**

`shouldFold` 的判据（`crates/axon-desktop/ui/src/conversation/transcript/fold.ts:319-325`）：

```ts
if (position >= total - 1) return false;        // 最新一轮永远展开
if (turn.folded || turn.keepExpanded) return false;
if (turn.status !== 'completed') return false;  // 只折已收口
if (!hasProcess(turn)) return false;            // 只在「想过或动过手」时折
return !inputAfterProcess(turn);                // 过程后又来输入的保持展开
```

因此「有的折、大部分不折」有三种来源：少数是索引重建的历史轮（天然带 `folded` 形态）；大部分是状态不是 `completed`；最新一轮按位置规则本就不折。折叠率低是 Q6 的直接镜像。

`interrupted` 刻意不折（`fold.ts:8-9` 的模块注释），这是产品决策，本轮不推翻。

### Q6 为什么一直显示「连接供应商」

**四个环节串成一条闭环失败：中断不写终态 → 索引判在途 → 界面落中性文案 → 中断回执说「已经结束」。**

1. `settle_uncertain` 只改 `status='uncertain'`，不写 `terminal_kind`、不归档（`crates/axon-session/src/infrastructure/sqlite_runtime_store/interrupted_execution_repository.rs:203-263`）；对照 `settle_interrupted` 写 `terminal_kind='interrupted'` 并归档（`crates/axon-session/src/infrastructure/sqlite_runtime_store/interrupted_interaction_repository.rs:101-151`）。
2. 读取侧派生表达式没有 `uncertain` 分支，落到 `ELSE 'received'`（`crates/axon-session/src/infrastructure/sqlite_runtime_store/interaction_query_repository.rs:18-22`）。
3. 终态判定只认 `understood/rejected/interrupted`（`crates/axon-daemon/src/infrastructure/gateway_session_turns_stream.rs:242-244`）；桌面侧同样把 `received/processing` 判为在跑（`crates/axon-desktop/ui/src/conversation/historyProjection.ts:57-66`），投影保持 `streaming`。
4. 在途但没有任何 provider 阶段帧 → 落中性兜底「正在连接供应商…」（`crates/axon-desktop/ui/src/conversation/transcript/derive.ts:70-83`）。
5. ESC 中断其实「成功」了：服务端找不到活跃主轮就如实回 `interrupted: false`（`crates/axon-cli/src/workbench_http_adapter.rs:228-249`），界面据此提示「本轮已经结束，无需中断」（`crates/axon-desktop/ui/src/conversation/useSessionInterrupt.ts:37-43`），**但索引仍说在途，spinner 不消失**。
6. 重放侧还有第二重打击：单次重放有 64 MiB 字节预算，触顶即截断并只打日志（`crates/axon-daemon/src/infrastructure/gateway_replay.rs:31,150-221`）。注意该模块自己的文档写着「单页上限只是 window，不是总量；一次 Subscribe 必须读到 durable 尾」，而实现里 `if page_len < replay_page_size || budget == 0 { break; }` 让预算直接决定了「是否读到尾」，**实现与它声明的契约相反**。HEAD 已把 `replayTruncated` 放进订阅回执（`crates/axon-daemon/src/infrastructure/gateway_websocket.rs:383-392`），但回执里**没有续读游标**，桌面端也全目录没有任何消费点。

## 3 与初判的四处纠偏

核实后必须修正的说法，避免把计划建立在错的因果上：

1. **子代理计数不是「名册直接显示在跑」**：名册只提供 `active/idle`，真正决定计数的是转录行状态；名册的 `active` 起的是「阻止纠正」的作用（`RuntimeActions.tsx:377-392`）。修名册口径必须同时修转录行收口，否则计数仍然不降。
2. **「AI 回复丢了」要改成「答案从未产生」**：1899 帧里 179607 字符全部是 reasoning 增量，没有任何答案正文帧，末帧是 `turn_uncertain`。可救的只有推理过程，没有最终答案可救。
3. **「换窗 800 多」出自运行时进度行，不是桌面指标**：它是 `CompactionStarted` 的摘要文本；桌面头部并非读错，而是根本没接这个来源。
4. **事件日志并没有「少一把锁」**：`EventLogWriter::open` / `open_at` 打开时就已经拿到跨进程独占锁（`crates/axon-eventlog/src/lock.rs:28-59`、`crates/axon-eventlog/src/append.rs:51-81`），`append_batch` 也校验批次序号连续（`append.rs:98-111`）。真正的缺口是 `open_at` **接受调用方给的序号却不校验文件尾**（`append.rs:269-296`）：锁只保证字节串行，不保证「序号与落点」对应。因此 WP-01 不是「加锁串行」，而是「让序号与文件尾在同一临界区对齐」。

## 4 不变量（本轮要落成的事实）

- **INV-1 顺序不变式**：同一会话的事件日志，读者读到的序号严格单调。序号与落点必须对应：调用方给的序号与文件尾不一致时必须显式失败并让控制面回滚，不得静默错位写入。
- **INV-2 终态不变式**：任何 Execution 离开 `running`，必须同时写 `terminal_kind` 并归档；`status IN ('completed','failed','cancelled','uncertain') AND terminal_kind IS NULL` 必须为零行。
- **INV-3 口径不变式**：Interaction 终态集合在 SQL 与 Rust 两侧由同一份定义生成，不再各写一遍。
- **INV-4 归属不变式**：除白名单里的会话级事件外，任何 wire_event 落盘时 `interaction_id` 非空。
- **INV-5 截断可见**：客户端必须消费「这次回放是残缺的」这一事实，并按游标续读，不得当作完整回放渲染。
- **INV-6 指标同源**：界面上任何「上下文占用」读数都读同一份带时间戳的快照；未知显示未知，不用旧值顶替。
- **INV-7 不回归泡**：会话状态机没有回不来的态；`recovering` 必须有归宿（恢复或过期收口）。

## 5 WBS 任务表

优先级：P0 是本轮必须完成的核心，P1 紧随，P2 收尾，P3 可观测性改善。

| ID | 优先级 | 服务问题 | 目标不变量 | 规模 |
| --- | --- | --- | --- | --- |
| WP-01 | P0 | Q6 | INV-1 | L |
| WP-02 | P0 | Q6 | INV-2, INV-3 | M |
| WP-03 | P0 | Q6 | INV-5 | M |
| WP-04 | P0 | Q6 | INV-2 | S |
| WP-05 | P1 | Q2 | INV-7 | M |
| WP-06 | P1 | Q3 | INV-4 | L |
| WP-07 | P1 | Q2, Q5 | INV-3 | S |
| WP-08 | P2 | Q4 | INV-6 | M |
| WP-09 | P2 | Q5 | INV-2 的下游镜像 | S |
| WP-10 | P3 | Q1 | INV-5 同源延伸 | S |
| WP-11 | P0 | 现场 | 存量收口 | S |
| WP-12 | P1 | 三端一致 | INV-1 / INV-6 的回归门禁 | M |
| WP-13 | P1 | 双气泡 | 一条消息只有一格 | S |

### WP-01 序号与文件尾对齐（INV-1）

- 已确证机制：`append_once` 用控制面水位作 `expected_next` 打开写句柄（`append_once` → `open_at`，`crates/axon-eventlog/src/append.rs:337-345,269-296`），`append_batch` 只校验「批次是否等于该期望值」，**不校验文件真实尾**。两个线程各自从控制面事务领到 57 与 58 后，谁先拿到文件锁谁先落盘，于是 58 可能先于 57 落地，且双方都不会报错。
- 候选写者对（代码形状与现场逆序吻合，日志级证据待补）：主轮流式 sink（逐条追加 wire_event）与 `run_turn_metrics` 的收口写（`crates/axon-cli/src/run_commands/run_turn_metrics.rs:138-164`，一次一条 `turn_metrics`，带 `invalidate_host` 重试）。现场逆序恰好是「单条 57 落在两条 58/59 之后」，与「流式一条 + 收口两条」的批次形状一致。
- 方案 A（推荐）：`open_at` 在持锁后读文件尾序号，与 `expected_next` 不符时**显式失败**（新错误变体），由控制面回滚水位并用文件尾重算。文件尾是位置权威，控制面水位是缓存，两者冲突必须让其中一方失败，不能静默写入。
- 方案 B：把序号分配移进日志锁内（先锁、读尾、再开控制面事务分配、写、解锁）。代价是文件锁覆盖数据库事务，与现有「阶段 1 事务、阶段 2 文件」的分离意图相反，且会拉长数据库写锁时间，不作为首选。
- 改动点：`crates/axon-eventlog/src/append.rs`（`open_at` + 新错误变体）、`crates/axon-eventlog/src/error.rs`、`crates/axon-session/src/infrastructure/sqlite_runtime_store/event_log_repository.rs`（`compensate_watermark_after_log_failure` 覆盖新错误）、`write_records` 与 `append_events_with` 两条调用路径。
- 验收：`append_once_rejects_sequence_that_skips_file_tail`（构造「文件尾 59、调用方声明 57」直接断言失败）；`concurrent_appends_never_reorder`（两写者各 200 条，断言读出的序号严格递增、无洞、无重复）；存量修复工具 `axon debug session repair --reorder --dry-run` 先出逆序清单。
- 风险：显式失败会让当前「静默成功」的路径变成可观测的写失败，必须同时确认上层有重试或降级，否则会把偶发写失败放大成用户可见错误。上线顺序应是「先加失败分支 + 观测（只记日志不拒绝）→ 观察一版 → 再切为拒绝」。

### WP-02 终态唯一出口与口径同源

- 目标：只有一条代码路径能产出 Interaction 终态；SQL 与 Rust 不再各写一份终态集合。
- 改动点：在存储层抽 `settle_work_item(execution_id, outcome)`，固定「改 status / 写 terminal_kind / 归档 / 追 ledger」四件事；`settle_uncertain` 与 `settle_interrupted` 改为传不同 `outcome`；`INBOX_STATE_EXPR` 增加 `uncertain` 分支（派生为 `interrupted`）；终态集合由 `InteractionStatus` 派生。
- 已知地雷（必须一起处理）：历史表的派生列用 `COALESCE(terminal_kind,status)`（`crates/axon-session/src/infrastructure/sqlite_runtime_store/work_item_archive.rs:79`），而 `parse_status` **不认识 `uncertain`**，遇到它会返回解码错误，按该文件自己的注释会「让含该轮的整条车道历史整体读不出来」（`interaction_repository_row.rs:76-89`）。因此归档必须先写 `terminal_kind` 再归档，同时给 `parse_status` 补 `uncertain → Interrupted` 作为存量兜底。
- 验收：`uncertain_settlement_writes_terminal_kind_and_archives`、`every_conclusive_status_has_terminal_kind`、`terminal_status_enum_matches_inbox_state_expression`、`legacy_uncertain_history_row_still_decodes`；`axon doctor` 两条不变式 SQL 恒为零行。
- 风险：两条路径的历史事件类型不同，合并时不得改变既有 ledger 事件名（`execution.uncertain` / `interaction.interrupted`），否则升级期的核对脚本会断。

### WP-03 截断可见与读到尾

- 目标：一次订阅必须补齐到 durable 尾（模块文档已声明的契约）；真截断时客户端能按游标续读。
- 服务端改动：`gateway_replay.rs` 的 `if page_len < replay_page_size || budget == 0 { break; }` 去掉「预算触顶即 break」，改为「预算只限制单次调用内存/时间，游标继续推进直到尾」。更要紧的是**返回的续读游标在截断时谎报已到尾**：`subscribe_and_replay` 无论是否读到尾都返回 `Some(live_ceiling)`（`gateway_replay.rs:64-80`），而订阅本身只从调用方给的 cursor 起等新帧（`stream_bridge.subscribe(..., resume_cursor)`），因此未重放的中间段在下次续订时会按已到尾的游标被永久跳过。回执必须带真实已读序号。
- 客户端改动：`crates/axon-desktop/src/conversation/stream.rs` 把回执里的截断事实与真实游标透出，`crates/axon-desktop/ui/src/conversation/` 据此按序号续读，并在仍残缺时显示「历史仅显示到第 N 帧」。
- 验收：`replay_reads_through_byte_budget_to_tail`、`replay_truncation_receipt_carries_resume_cursor`、`replay_truncated_triggers_cursor_resume`；真机在长会话上打开最新一轮，正文能落到尾帧。

### WP-04 判死：中断回执与静默超时

- 目标：不再有「界面说在跑、服务端说已结束」的僵持。
- 改动点：`useSessionInterrupt` 在 `interrupted=false` 时**重读一次轮次索引**再决定显示；若索引仍返回在途，如实提示「服务端无运行轮次，界面状态待修复」而不是静默保持 spinner；「标记为已中断」这类动作必须落在服务端（一个显式的收口接口），不做纯前端状态改写，否则等于让界面撒谎。
- 另一半在服务端：provider 首字超时与轮次租约过期都必须收敛为失败或中断终态（与 WP-02 同一出口），从源头消灭「非终态但没有执行者」。
- 验收：`silent_turn_with_stale_lease_offers_server_side_settle`、`interrupt_receipt_false_triggers_index_refetch`；真机按 ESC 后 spinner 消失且提示与服务端一致。

### WP-05 `recovering` 收敛与名册改派生

- 目标：`recovering` 有归宿；名册状态来自运行态派生而不是 JSONL 索引缓存。
- 已确证：`recover_expired_sessions` 把会话写成 `recovering` 后，daemon 只取返回值里的条数（`crates/axon-daemon/src/application/daemon_host.rs:425-429`），`axon-session-runtime` 的 `SessionRecovery::recover_batch` 没有任何调用方，因此**没有任何环节会把 `recovering` 送回 `idle` 或重新派发**。
- 改动点：`runtime_repository.rs` 增加两条收敛（无 work_item 或全部终态则回 `idle`；超过 TTL 仍未派发则收口为 `interrupted` 并追加事件）；`workbench_http_subagents_jobs.rs` 的 `status` 改读 `interaction_turn_heads` 派生；`RuntimeActions.tsx` 的 `statusOf` 在名册与转录冲突时以运行态为准。
- 验收：`recovering_session_without_work_item_is_settled_to_idle`、`recovering_session_past_ttl_settles_interrupted`、`subagent_roster_status_comes_from_turn_heads`。
- 风险：名册改派生后，历史上「有 Thread 无运行态会话」的子代理会显示未知，需要一次回填或明确的「不可回看」文案；同时要先回答 §6 的第 5 条（应用服务是接回还是删除）。

### WP-06 子代理事件归属与存量迁移

- 目标：子代理事件落盘带归属；读侧删除载荷串兜底；存量可回填。
- 已确证的约束：`SubagentSessionIdentity` 只有 `session_id / root_session_id / agent_path`（`crates/axon-session-runtime/src/ports/mod.rs:88-96`），**没有派发交互标识**，因此不能指望运行侧直接透传。
- 推荐落法（写入侧不变式 + 可判定的读侧兜底）：
  1. `ChildSessionSinkFactory::sink_for` 在构造 sink 时，用 store 查出该子会话的派发行（`inbox.session_id = 子会话` 且 `source_message_id` 以 `agent-dispatch:` 开头），把 `interaction_id` 缓存进 sink，改调 `append_attributed_events`（该入口本就允许「有归属、无租约」）。
  2. 读侧删除载荷子串扫描，改为「归属列 → 同一条派发 join」两级，两级都取不到就如实报「该轮次没有归属」。
  3. 存量回填用同一个 join 的 `axon debug session repair --attribute --dry-run`。
- 改动点：`crates/axon-daemon/src/application/child_session_event_sink.rs`、`crates/axon-session/src/ports/event_log_store.rs`（把不带归属的入口标注为仅会话级事件，或直接收掉）、`crates/axon-daemon/src/infrastructure/gateway_replay.rs`（删除兜底扫描）。
- 验收：`subagent_events_carry_dispatch_interaction_attribution`、`wire_event_without_attribution_is_rejected`；迁移报告显示本次 145 条可回填。
- 风险：派发行可能晚于首个子会话事件到达，首批帧仍会无归属。因此读侧必须保留 join 兜底，且迁移工具要能区分「可唯一确定」与「无法确定」两类。

### WP-07 折叠判据与形态一致

- 目标：折叠表达「过程不会再变」，而不是「status 恰好等于 completed」。
- 改动点：`fold.ts` 改为 `settledAt != null || 静默超阈值` 且服务端判定非 running；`interrupted/failed/rejected` 保持展开但加视觉标记；补「实时投影与索引重建投影折叠形态一致」的门禁。
- 依赖：WP-02 落地后才能看到折叠率回升。
- 验收：`fold_form_is_identical_for_realtime_and_index_projection`、`interrupted_turn_stays_expanded_with_marker`。

### WP-08 上下文快照单一来源

- 目标：头部与换窗提示读同一份快照，且快照带来源与时间。
- 已确证的约束：`axon_eventlog::TurnMetrics` **没有**上下文占用字段（`crates/axon-eventlog/src/record.rs:40-74`，只有 input/output/reasoning/cache），因此快照不能从 `turn_metrics` 来；唯一携带 `currentContextTokens` 的是用量 `wire_event` 的载荷（现场样本：`{"conversationTokens":80821,"currentContextTokens":135917,…}`）。
- 推荐落法：在既有的 `apply_projections`（`crates/axon-session/src/infrastructure/sqlite_runtime_store/event_log_repository.rs:492-521`，每次追加的唯一投影入口）里增加一个 `context_snapshot` 投影：从本批次里的用量帧取 `currentContextTokens / contextWindow / 构成`，从轮转生命周期取 before/after，同事务写一行会话级快照。写入点仍只有这一处，不新增第二条写路径。
- 读侧：桌面头部改读快照并显示「更新于 Ns 前」，删除 `previous?.currentContextTokens` 粘连；未知显示未知。
- 验收：`context_snapshot_follows_usage_frames`、`context_snapshot_records_rollover_before_after`、`header_context_follows_rollover_snapshot`；真机一次换窗后头部与换窗行同值。
- 风险：`apply_projections` 是每次追加都会跑的路径，新增投影必须保持「解析失败只记日志、不阻断落盘」的既有约定（与 `session_metrics` 投影同款），否则会把投影缺陷放大成写入故障。

### WP-09 折叠与投影的收尾一致性

- 目标：索引重建轮与实时轮的形态、摘要短语、读数完全一致。
- 改动点：折叠只在投影层算一次的现状固化，补一条形态对比测试。
- 验收：与 WP-07 同一组用例。

### WP-10 运行态证据链

- 目标：「在跑」必须能指回带租约的行，且新鲜度可判定。
- 改动点：只读端点 `GET /api/v1/sessions/{id}/live`（`running / leaseRemainingMs / lastFrameSequence / idleForMs`）；`axon doctor` 增加 `live` 检查；界面在证据过期时降级表达。
- 验收：关闭守护进程后 60 秒内界面转为「证据过期」；`live_payload_marks_stale_lease_as_not_running`。

### WP-11 现场存量收口（记录在案）

本轮为恢复桌面端可用性，已对现场做过两处**数据修复**（先备份后改）：

- 备份：`$HOME/.axon/data/backup-control-<时间戳>.sqlite3`
- `work_items`（execution `a3870cba-…`）`terminal_kind` 置为 `interrupted`
- `inbox`（interaction `01M7Z4BFYY8QX8DKF83S6DHBRE`）`terminal_kind` 置为 `interrupted`

收口要求：WP-02 落地后，这两处应能由官方路径重放得到同样结果；届时用一份该现场的数据副本做回归夹具，避免「只有手工改库才能修好」。

## 6 TUI 与 CLI 影响面（三端一致）

桌面端不是唯一消费者。TUI 与 CLI 追读都跑在自己的进程里，走同一份 durable 事件流，但**判定与兜底机制不同**，因此同一批缺陷在三端的表现不一样。下表逐条核过代码：

| 问题 | 桌面端 | TUI | 依据 |
| --- | --- | --- | --- |
| Q1 无限在跑 | 有 | 无：订阅恢复有 55 秒预算、前台等待有显式截止时间 | `docs/dev/tui-internals.md` §2；`daemon_subscription_recovery.rs:124-139` |
| Q2 子代理一直在跑 | 有（名册 active 阻止纠正） | 无：活动行纯事件驱动，本轮收口或中断即整行清掉 | `state/subagent_activity.rs:39-70`；`state.rs:392,502` |
| Q3 子代理详情空白 | 有（Interaction 归属缺失） | 无该视图；CLI 追读走 Thread，本现场可读到 Turn 0 与用户消息 | `SubagentRunView.tsx:100-115`；`child_session_event_sink.rs:116-125` |
| Q4 上下文不跟手 | 有 | **有**：同一套粘连语义，新帧值为 0 时保留上一次非零值；从未观测到时整段不显示 | `run_output_usage.rs:63-66,121-134`；`header_bar.rs:312-320` |
| Q5 折叠缺失 | 有（只折 `completed`） | 不适用：思考按 part 完成折叠、工具行按规则表折叠，与轮次状态无关 | `fold.ts:319-325`；`tui-internals.md` §3、§5 |
| Q6 一直连接供应商 | 有（中性兜底 + 索引判在途） | 低：恢复选路走 `latest_processing_interaction_for_route`，SQL 只认 `w.status='running'`，`uncertain` 不匹配，因此不会把这一轮接回来；`is_active_interaction_status` 只在「自己正在等一轮」的订阅恢复里生效 | `daemon_history_loader.rs:31`；`interaction_query_repository.rs:177-207`；`daemon_subscription_recovery.rs:110-115` |
| R1 事件日志乱序 | 乱序渲染，且游标按「页内最后一条」推进时可能跳帧 | **直接丢帧**：同一 durable stream 内低序号迟到帧被拒绝并计入 `staleCount` | `tui-internals.md` §3；`submission_worker_mailbox.rs:113` |
| R2 重放预算截断 | 有（回执带截断、客户端无消费点） | 无：TUI 走进程内订阅，不经过网关重放 | `gateway_replay.rs` 仅在 daemon 网关侧 |

结论：**TUI 真正受影响的是 R1（丢帧）与 Q4（粘连）**；Q6 在 TUI 侧不成立（恢复选路不认非 running 的轮次），Q1/Q2/Q3/Q5 要么有别的兜底、要么不适用。因此本轮的三端回归重点是「R1 在 TUI 的丢帧可观测」与「上下文快照两端同源」，桌面端那四个问题不需要在 TUI 重复修。

### WP-12 三端回归门禁

- 目标：同一批缺陷在桌面端、TUI、CLI 追读三条路径上都有断言，避免「桌面修好了、TUI 静默丢帧」。
- 内容：
  1. 逆序日志夹具：用本现场会话的事件日志做 fixture，断言 TUI 侧 `staleCount` 可观测且终态正文不缺段（配合 WP-01 后应归零）。
  2. 上下文一致性：喂「用量帧不带窗口占用 + 一次轮转」的序列，断言 TUI 头部与轮转行的读数取自同一份快照（与 WP-08 同一 fixture）。
  3. 子代理活动行：父轮失败、中断、进程重启三种收口路径下，活动行必须清空（保留既有 `subagent_activity_tests` 并补重启路径）。
  4. CLI 追读口径：`axon sessions read`、`axon debug session read`、`axon debug session doctor` 在「`uncertain` 轮次」与「无归属子会话」上的输出必须可读且标注原因，不得只给空正文。
- 验收：上述四组用例进 `cargo test --workspace`；真机各取一次 TUI 与桌面端回执。
- 说明：TUI 的 UI 契约不加新组件、不加新视觉，只是取数与收口口径变化，因此不需要改 `ui-constraints.md`。

## 7 追加问题：同一条消息出现两条气泡（桌面端）

现场同时出现过「一条标着等待注入、一条没有任何标记」的用户气泡。数据侧先排除服务端重复：

| 口径 | 结果 |
| --- | --- |
| 同会话同正文精确重复 | 0 组 |
| 同会话同正文归一化重复（去换行与首尾空白） | 0 组 |
| 同会话 `source_message_id` 重复 | 0 组 |
| 同交互 `userMessage` 帧重复（按 partId 计） | 0 组（17 帧、17 个唯一 partId、11 个交互） |

因此**不是 inbox 落了两条**，是客户端把一条消息渲染成了两格：

1. 运行中排队时，本地乐观格用 `local-steer:` 前缀立起（`state.ts:152-159`），标「等待注入」。
2. 索引骨架会为同一交互另立一格 `local-user:<interactionId>`（`pagingSkeleton.ts:43-51`）。
3. durable 用户帧到达后，`applyUserMessage` 用 `turn.cells.find(...)` **只认领第一格**并换成服务端标识（`cells.ts:148-172`），另一格原地留下。

于是「被认领的那格没有标记、没被认领的那格仍是等待注入」，正是现场看到的两条。触发条件因此是「长轮次里排队 + 期间发生过索引重载（轮次头新轮刷新或分页重读）」，空闲时新开一轮不会出现，这解释了「有时候」。

TUI 不受影响：`commit_pending_steer` 是按 FIFO 弹出队列头，队列为空时直接忽略该帧（`queued_input.rs:159-166`，注释写明「无 pending steer 时再补一条只会出现重复气泡」）。

修复方向（WP-13）：

- 认领改为「把所有同可见正文的本地格合并成一格」：认领一格后，其余同文本的 `local-*` 格一并移除，而不是只换第一格。
- 更彻底的做法是把认领键从正文换成稳定标识（提交时由客户端带 id，运行侧原样回写到 `WireUserMessage.messageId` 或新增字段），正文只用于展示。
- 门槛用例：`queued_steer_and_skeleton_cell_merge_into_one`、`structured_input_message_renders_single_bubble`。
- 说明：`withOpeningInput` 已经为「尾窗 + 开场输入」做了同款去重（`pagingSkeleton.ts:76-85`），本项是同一类缺陷在另一条路径上的缺口，修法应对齐而不是另造。

## 8 未闭环事项

1. 逆序的**写者组合**仍未确证：机制已收窄到「`open_at` 信任调用方序号、不校验文件尾」，但还需要一次带写者标识（session + 调用点）的并发现场，才能确定是哪两条路径在同一个会话上并发追加。
2. Thread 投影把同一 `turn_id` 重复列为 6 条 Active（21:03 到 21:20，assistant 全空），与 `work_items.attempt=2` 不符，去重口径待定，暂不纳入本轮。
3. 换窗进度行的 `before_tokens` 与 provider 实测上下文是否逐字一致，需要一次带日志的换窗真机复核。
4. **Q4 的「800 多」载体尚未取证**：候选有三个（compaction 过程行 `role:'history'` 的 title/summary、`ContextUsagePopover` 的构成视图、TUI 头部），代码层面无法判定用户当时看到的是哪一个。实施 WP-08 前应先做一次换窗真机复现，确认到底哪一处显示 825338，否则会修错地方。
5. `0685d0de` 那条 15:54 的 Active 轮次是否与本次同因，需要按 WP-01 的逆序清单回溯。
6. `axon-session-runtime` 里的 `SessionRecovery::recover_batch`（`crates/axon-session-runtime/src/application/session_recovery.rs:19-33`）目前没有任何调用方，daemon 走的是 `store.recover_expired_sessions` 并丢弃返回值。WP-05 要先决定：是把这层应用服务接回调度，还是删掉它以免日后误以为它在生效。

## 9 执行顺序与门禁

顺序：WP-01 → WP-02 → WP-03/04（可与 02 并行）→ WP-05/06/07 → WP-08/09 → WP-10；WP-11 随 WP-02 一起验收；WP-12 的用例随各自依赖的 WP 一起落地，整组回归放在每批收口时跑。

三端回归的固定动作（每批收口都做，不只做桌面端）：

1. `cargo test --workspace` 全绿，重点看 `axon-tui-runtime`、`axon-cli`（追读命令）与 `axon-session`（存储不变式）三处。
2. `axon run` 跑一轮真实对话，确认 TUI 头部、子代理活动行与本轮正文的收口都正常。
3. 桌面端真机按阶段 V 取证。
4. 用本现场日志夹具（见 WP-12）跑一次三端对照，确认同一份异常数据在三端都给出可读结论。

每批必须全绿的自动化门禁：

```powershell
cargo fmt --all --check
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
pwsh -File tools/verify-architecture.ps1
pwsh -File tools/verify-docs.ps1
git diff --check
```

涉及 `crates/axon-desktop/ui/**` 的批次追加：`npm ci` / `npm run build` / `npm run lint` / `npm test` / `pwsh -File tools/verify-ui-contract.ps1`，并确认 `ui/dist` 不比 `ui/src` 旧。

真机验证按 `docs/dev/device-verification.md` 的口径走：停守护进程与桌面端、覆盖安装、经稳定入口重启、用 `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9333` 后经 CDP 取证，只认 `OK:` 前缀回执。

文档同步：本文件随实施更新状态；产品行为变化同步 `docs/desktop.md` 与 `docs/tui.md`，界面口径变化同步 `docs/dev/ui-constraints.md` 与 `docs/dev/workbench-design-system.md`，TUI 取数或收口口径变化同步 `docs/dev/tui-internals.md`，运行态命令面变化同步 `docs/skills/operate-axon/`。
