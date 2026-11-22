# 会话真相链路验收台账

> 逐项登记 `docs/dev/conversation-truth-wbs.md` 中每个条目的状态、判据与证据。
>
> 状态口径：**已完成** 表示判据已被可复跑的证据覆盖；**进行中** 表示实现已落地但判据尚未全部取到；**未开始** 表示尚未实施。
>
> 证据口径：只写可以直接复跑或直接查阅的东西（测试名、命令、提交号、文件位置）。离线证据与真机证据分开标注，离线门禁不冒充真机通过。

## 1 总表

| 编号 | 优先级 | 状态 | 服务问题 | 证据摘要 |
| --- | --- | --- | --- | --- |
| WP-01 | P0 | 已完成 | Q6 / R1 | `sequence_alignment.rs` 三条 + 存储层三条（含反证） |
| WP-02 | P0 | 已完成 | Q6 | `work_item_settlement_tests.rs` 五条 + 公开 INV-2 检查 SQL |
| WP-03 | P0 | 已完成 | Q6 / R2 | `gateway_replay_contract.rs` 三条 + 桌面 `replayTruncation.test.mjs` 两条 |
| WP-04 | P0 | 已完成 | Q6 | tick 用例 + 路由幂等用例 + 客户端重读索引用例 |
| WP-05 | P1 | 已完成 | Q2 | `sqlite_runtime_store_recovering_tests.rs` 四条 + 名册派生用例 |
| WP-06 | P1 | 已完成 | Q3 | 归属契约四条 + 派发 join 两条 + 修复工具两条 |
| WP-07 | P1 | 已完成 | Q5 | `turn-folding.test.mjs` 两条 |
| WP-08 | P2 | 已完成 | Q4 | 投影两条 + 路由两条 + 桌面 `contextSnapshot.test.mjs` 两条 |
| WP-09 | P2 | 已完成 | Q5 | 与 WP-07 同一组形态一致用例 |
| WP-10 | P3 | 已完成 | Q1 | `live_payload_marks_stale_lease_as_not_running` + `desktop_shows_evidence_expired_instead_of_running` |
| WP-11 | P0 | 已完成 | 现场 | `field_manual_repair_is_reproduced_by_official_settlement` |
| WP-12 | P1 | 已完成 | 三端一致 | TUI 与 CLI 七条用例（见分条） |
| WP-13 | P1 | 已完成 | 双气泡 | `userBubbleMerge.test.mjs` 两条 |

## 2 分条证据

### WP-01 序号与文件尾对齐（INV-1）

- 实现：`open_at` 在**持有跨进程独占锁之后**读文件尾记录序号，与调用方声明的期望序号比对；不一致返回新错误变体 `SequenceTailMismatch` 且不写任何字节。空日志（没有任何记录，含只有 header 帧）只接受事件日志层原点 `0` 或控制面首次分配 `1`，header 帧不算记录。
- 补偿：水位回退改为 CAS（只有当前水位仍等于本次写入推进到的值时才回退），并按落盘尾重排序号后有界重试（上限 3 次），失败方的事件最终仍然落盘。
- 证据：
  - `crates/axon-eventlog/tests/sequence_alignment.rs`：`append_once_rejects_sequence_that_skips_file_tail`、`empty_and_header_only_logs_accept_only_origin_sequences`、`concurrent_appends_never_reorder`。
  - `event_log_repository` 内部用例：`watermark_rollback_then_retry_succeeds`、`compensation_never_rolls_back_a_later_writers_watermark`、`compensation_then_realign_recovers_a_writer_left_behind`。最后一条做过反证：把 CAS 条件改回 `=` 时该用例失败（`水位落后于落盘尾时必须允许重排`），恢复后通过。
- 口径修正：WBS 风险段写的「先只记日志、观察一版再切拒绝」未采用。本项目未上线、目标态优先，直接切为「显式失败 + 结构化观测」。

### WP-02 终态唯一出口与口径同源（INV-2、INV-3）

- 实现：`work_item_settlement::settle_work_item` 是唯一出口，固定「改 status / 写 terminal_kind / 归档 / 追流水账」四步，顺序不可颠倒（先 `terminal_kind` 再归档，否则历史行派生列带回旧词）。`settle_uncertain` / `settle_interrupted` / `mark_understood` / `mark_rejected` / `mark_interrupted` / `finish_execution` 全部改走同一出口。
- 口径同源：`crates/axon-session/src/domain/interaction_status.rs` 由领域枚举派生状态取值、终态集合与结论状态映射；`inbox_state_expr()` 由它生成 SQL 派生列，并补结论状态兜底分支（`uncertain` 派生 `interrupted`）；`parse_status` 从同一张表解析，存量 `uncertain` 历史行不再让整条车道读不出来。
- 不变式入口：`axon_session::conclusive_work_items_without_terminal_kind_sql()`（`axon doctor --check inv-2` 直接引用）。
- 证据：`work_item_settlement_tests.rs` 的 `uncertain_settlement_writes_terminal_kind_and_archives`、`every_conclusive_status_has_terminal_kind`、`terminal_status_enum_matches_inbox_state_expression`、`legacy_uncertain_history_row_still_decodes`、`field_manual_repair_is_reproduced_by_official_settlement`。
- 顺带修掉的缺口：`finish_execution`（目标执行收口）原来既不写 `terminal_kind` 也不归档，会持续留下 INV-2 违规行；现已并入统一出口。

### WP-03 截断可见与读到尾（INV-5）

- 服务端实现：载荷预算不再参与「是否读到尾」的判定（只留告警），页数兜底是唯一硬上界；订阅确认帧在真截断时带 `replayTruncated` 与**真实已读位置** `resumeCursor`；与实时帧去重用的水位同源下调，缺口由实时通道补上（宁可重叠，不丢帧）。
- 证据：`crates/axon-daemon/tests/gateway_replay_contract.rs` 的 `replay_reads_through_byte_budget_to_tail`、`replay_truncation_receipt_carries_resume_cursor`、`replay_resume_cursor_does_not_skip_frames`。
- 客户端消费见 WP-10 车道（同一条订阅回执）。

### WP-04 判死：中断回执与静默超时

- 服务端：`silent_turn_settlement` 提供判死快照与显式收口，接进 supervisor tick（顺序：恢复接管 → 执行中断隔离 → 交互中断隔离 → `recovering` 收敛 → 静默轮次收口 → 目标重投；注释写明顺序理由）。显式收口路由 `POST /api/v1/sessions/<id>/settle-silent-turn`，判据词表固定为 `lease_expired` / `first_token_deadline_exceeded` / `still_running` / `no_in_flight_turn` / `already_settled`。
- 客户端：回执为假时重读一次轮次索引；索引仍说在途就如实提示「服务端无运行轮次，界面状态待修复」，不静默保持 spinner，也不做纯前端状态改写。
- 证据：`silent_turn_with_stale_lease_offers_server_side_settle`、`silent_turn_settlement_runs_on_supervisor_tick`、`settle_silent_turn_route_is_idempotent`、`interrupt_receipt_false_triggers_index_refetch`。

### WP-05 `recovering` 收敛与名册改派生（INV-7）

- 实现：两条收敛挂在 supervisor 维护路径上（无在途工作项或全部终态 → 回 `idle`；超过 TTL 仍未派发 → 收口 `interrupted` 并追加会话事件）；名册 `status` / `lastTurnStatus` 改由 `interaction_turn_heads` 派生，JSONL 索引的末轮状态降级为对账旁证 `threadTurnStatus`。
- 删除无调用方的 `SessionRecovery::recover_batch`（删除前 `git grep` 证据：除自身定义与两处再导出外零引用）。
- 证据：`recovering_session_without_work_item_is_settled_to_idle`、`recovering_session_past_ttl_settles_interrupted`、`recovering_sessions_never_stay_without_an_exit`、`crashed_turn_leaves_recovering_within_one_supervisor_tick`、`subagent_roster_status_comes_from_turn_heads`。

### WP-06 子代理事件归属与存量迁移（INV-4）

- 写入侧：`ChildSessionSink` 构造时按派发行（`inbox.session_id` 等于子会话且来源键带 `agent-dispatch:`）反查归属并缓存，改走带归属入口；查不到时结构化日志 + 计数器，下一次写入前重试（派发行晚到可自愈）。
- 读侧：删除载荷子串扫描，改为「归属列 → 同一条派发 join」两级；归属不唯一时返回空页并在 `unattributedReason` 里给出中文原因。
- 收窄口径：`wire_event` 在**子代理会话**上无归属即拒绝；主会话的会话级 wire 写入（回合之外的迟到作答）放行并计数留痕（白名单是具名常量，理由写在 `ports/event_log_store.rs`）。
- 存量工具：`axon debug session repair --attribute [--apply]`，默认只出清单，执行前备份、原子替换、幂等。
- 证据：`crates/axon-session/tests/event_log_attribution_contract.rs` 四条、`gateway_replay_dispatch_join.rs` 两条、`child_session_event_sink_tests.rs` 两条、`session_repair_tests.rs` 的 `attribute_dry_run_classifies_backfillable_rows` 与 `attribute_repair_writes_interaction_columns`。

### WP-07 / WP-09 折叠判据与形态一致

- 实现：`shouldFold` 改为 `settledAt != null || 静默 ≥ 5 分钟`，落位规则与待接手轮次仍然豁免；`interrupted / failed / rejected` 保持展开并在正文末尾给一条结论标记（沿用既有提示行形态，不新增视觉语言）。`settledAt` 取服务端记录时间而非墙钟，保证整批回放与逐条增量投影逐字段相等。
- 证据：`turn-folding.test.mjs` 的 `fold_form_is_identical_for_realtime_and_index_projection`、`interrupted_turn_stays_expanded_with_marker`。

### WP-08 上下文快照单一来源（INV-6）

- 服务端：`context_snapshots` 会话级投影表，写入点只有 `apply_projections`；用量帧给当前占用、窗口与构成，轮转生命周期帧给换窗前后读数；没有读数的帧既不覆盖也不刷新时间戳；压缩后的展示值（`825K`）一律留空，不反推。
- 读路由：`GET /api/v1/sessions/<id>/context-snapshot`（`snapshot` 为 `null` 表示「还没有读数」，与会话不存在 404 严格区分）。
- 证据：`context_snapshot_follows_usage_frames`、`context_snapshot_records_rollover_before_after`、`context_snapshot_route_projects_reading_with_gateway_field_names`、`context_snapshot_reads_exact_numbers_and_leaves_compacted_text_unknown`、`context_snapshot_route_returns_null_snapshot_for_empty_session`。
- 已知限制：换窗读数的结构化字段来自运行时审计事件，落到持久化日志的 CLI 进度帧只把数字压进摘要文本，因此真机上这两个字段多数为 `null`（未知显示未知，符合 INV-6）。

### WP-10 运行态证据链

- 服务端：`GET /api/v1/sessions/<id>/live`，`running` 只认「`work_items.status` 为 `running` 且租约未过期」的行；`lastFrameSequence` 取会话 durable 水位，`idleForMs` 取最后活动时刻。
- 桌面：租约到期、静默超阈值或连续读不到证据时不再显示「在跑」，改用既有错误卡说明「运行证据已过期」并给出重启入口；按 ESC 会请求服务端收口并按判据词表给结论。
- 证据：`live_payload_marks_stale_lease_as_not_running`、`desktop_shows_evidence_expired_instead_of_running`、`interrupt_settles_via_server_route_when_index_is_stale`。

### WP-11 现场存量收口

- 现场两处手工修库（`work_items` 的 execution `a3870cba-…`、`inbox` 的 interaction `01M7Z4BFYY8QX8DKF83S6DHBRE`，均置 `terminal_kind=interrupted`，改前已备份到 `$HOME/.axon/data/backup-control-20261122-055907.sqlite3`）。
- 回归夹具：`field_manual_repair_is_reproduced_by_official_settlement` 用等价最小行重放两条官方路径，断言得到与手工修库一致的 `terminal_kind`，且 INV-2 违例零行。

### WP-12 三端回归门禁

- TUI 与桌面同源：头部占用段与新增换窗段都读会话级快照，删除「新帧值为 0 时保留上一次非零值」的粘连。证据：`tui_header_and_rollover_read_the_same_snapshot`、`tui_context_reading_does_not_stick_to_previous_value`。
- 逆序日志可见且不丢终态：普通迟到帧仍拒绝并计入 `staleCount`；终态语义帧（收口 `TurnLifecycle` 与非 delta 的 `AssistantMessage`）非单调也接受，水位取最大值，单独计入 `late_terminal_count`。证据：`stale_frames_are_counted_and_visible`、`terminal_body_survives_stale_frame_drops`。
- 活动行三种收口：失败、中断与进程重启后活动行都清空，历史回放不复活。证据：`subagent_activity_rows_clear_after_restart`。
- CLI 追读口径：`sessions read` / `debug session read` / `debug session doctor` 在 `uncertain` 轮次与无归属子会话上给出可读结论与原因，正文为空时补说明。证据：`sessions_read_explains_uncertain_turn`、`session_read_explains_unattributed_subagent_frames`。

### WP-13 同一条消息两条气泡

- 实现：`applyUserMessage` 认领范围改为「全部同可见正文的本地占位格」，优先取运行中排队那一格并把其余 `local-*` 同文本格一并摘掉；合并后仍按 `orderTurnCells` 决定轮内落位；摘空会让某轮变成空轮时保留该格，避免空 `streaming` 轮常驻转圈。
- 证据：`userBubbleMerge.test.mjs` 的 `queued_steer_and_skeleton_cell_merge_into_one`、`structured_input_message_renders_single_bubble`。

## 3 现场只读取证（安装态旧二进制，未改任何用户数据）

`axon debug session repair` 与 `axon doctor` 在真实数据目录上的只读结果：

| 命令 | 结果 |
| --- | --- |
| `axon debug session repair --reorder --dry-run` | `scanned_sessions: 17`、`dirty_sessions: 4`、`inversion_count: 13`；主会话 `01M7YFWW…` 事件 6230 条、逆序 9 处（实测 `#58 序号 59 -> 57 wire_event -> turn_metrics`，与 WBS 举例 `56,58,59,57,60` 同型） |
| `axon debug session repair --attribute --dry-run` | `scanned_sessions: 12`、`backfillable_frames: 150`、`undetermined_frames: 941`；子代理会话 `01M7Z4BF…` 144 条帧全部可回填 |
| `axon doctor --check invariants`（守护进程未运行） | `inv-2` / `inv-7a` / `inv-7b` 三项均 `unverifiable` 并给出原因，不误报零行通过 |

计数口径差异（WBS 与本工具）：WBS 记该会话 6231 条事件、28 处逆序，本工具记 6230 条、9 处。前者按「最大序号 + 1」与「由此产生的序号缺口」计数，后者按「读到的序号不再严格递增」计（每个错位帧 1 处）；两者逐条形状同型。真实记录条数以后者为准。

## 4 门禁回执

整合期在 `master` 上执行（离线证据，不含真机）：

| 命令 | 结果 |
| --- | --- |
| `cargo fmt --all --check` | 通过 |
| `cargo check --workspace --all-targets` | 通过 |
| `cargo clippy --workspace --all-targets -- -D warnings` | 通过，零告警 |
| `cargo test --workspace` | 全绿 |
| `pwsh -File tools/verify-architecture.ps1` | 输出 `Architecture verification OK` |
| `pwsh -File tools/verify-docs.ps1` | 输出 `Documentation verification OK` |
| 前端 `npm ci` / `npm run build` / `npm run lint` / `npm test` / `verify-ui-contract.ps1` | 604/604 通过，契约门禁 ALL GATES PASSED |

真机证据（安装态二进制 + 真实窗口 + CDP `OK:` 回执）见下一节，不与上表混记。

## 5 真机证据（安装态 + 真实窗口 + CDP）

安装态：`$HOME/.axon/bin/versions/0.0.1/`。`axon.exe` 与 `axon-desktop.exe` 均以本次 `cargo build --release --locked` 产物覆盖，覆盖后与构建产物 **SHA-256 逐字节一致**；`current.json` 前后不变（`{"versionId":"0.0.1"}`）；旧产物留档为 `*.pre-ctxsnapshot`。

安装方式说明：标准入口 `scripts/install-from-source.ps1` 的后端安装需要管理员权限注册计划任务，本会话无法代点 UAC；经长官授权改为**免提权覆盖安装**（直接覆盖版本目录内二进制）。因此「计划任务注册 / 服务化安装」本轮未验证，其余（版本树、入口解析、二进制一致性）已验证。恢复标准路径只需在交互式终端重跑该脚本。

守护进程：`axon daemon run --root $HOME/.axon --config-path $HOME/.axon/config.toml`，`daemon health` 回 `host.status=running`、租约剩余约 28 秒。

schema 升级：v14 的 `control.sqlite3` 已改名保留为 `control.v14-before-context-snapshot.sqlite3`（未删除），新库按 v15 自动建立。

| 验证项 | 回执 |
| --- | --- |
| `axon --version` | `axon-cli 0.0.1` |
| `axon doctor --check invariants` | `ok`：`inv-2` / `inv-7a` / `inv-7b` 全部 ok，违规 0 行（证明新 schema 与三条不变式检查在安装态生效） |
| 真实会话 | `01M80PMDTJ80DBKWAQZT5P8MAQ`，三轮（其中一轮含一次 shell 工具调用） |
| CDP 断言 | `OK:{"userBubbles":3,"foldedTurns":1,"headerFreshness":"更新于 20 秒前","contextReading":"66.7K/未知"}` |

逐项结论：

- **WP-13 一条消息一格**：三轮各一个用户气泡，没有「一条等待注入、一条无标记」的重复格。
- **WP-07 折叠**：带工具调用的历史轮折成「过程 · 执行了命令」，最终正文与用量行照常保留；最新一轮保持展开（思考与执行行可见）。
- **WP-08 上下文读数**：头部显示 `66.7K/未知` 与 `更新于 20 秒前`：占用取自会话快照，窗口分母缺失时如实显示未知而不是 `0`，并带更新时间。
- 收尾：验证结束后已关闭 `--remote-debugging-port` 并重启桌面端（CDP 端口确认已关闭），守护进程保持运行。

未在真机触发（如实登记，只有单测证据）：`WP-10` 的「运行证据已过期」卡（需要在跑轮次叠加守护进程停机才能构造）、`WP-03` 的截断续读（正常会话不可达）、`WP-04` 的 ESC 收口（当时没有在途轮次）、TUI 头部读数（未在交互式 TTY 取证）。
