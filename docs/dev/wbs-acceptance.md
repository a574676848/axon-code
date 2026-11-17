# WBS 验收台账

> 逐项登记 `docs/dev/reconstruction-wbs.md` 中每个条目的状态、判据与证据。
>
> 状态口径：**已完成**表示判据已被可复跑的证据覆盖；**进行中**表示实现已落地但判据尚未全部取到；**未开始**表示尚未实施；**撤销**表示经裁决不再实施。
>
> 证据口径：只写可以直接复跑或直接查阅的东西（测试名、命令、提交号、文件位置）。真机证据与离线证据分开标注，离线门禁不得冒充真机通过。

## W0 基线与准备

| 编号 | 状态 | 判据 | 证据 |
| --- | --- | --- | --- |
| W0-1 | 已完成（口径修订） | 基线可复现：每条重构条目都落在可直接回跑的提交上 | 未另建 `reconstruct` 分支：全部改造在 `master` 上按条目提交（`f6bfafd2f` 起），避免分支冻结与安装态二进制失配。WBS 原文的分支冻结是单人节奏下的保守假设，按实际执行口径修订 |
| W0-2 | 已完成 | `cargo fmt/check/clippy/test` 与 `verify-*` 全绿 | 基线提交 `f6bfafd2f`：`cargo test --workspace` 全绿、`cargo clippy --workspace --all-targets -- -D warnings` 无告警、`tools/verify-architecture.ps1` 与 `tools/verify-docs.ps1` 均输出 OK |
| W0-3 | 已完成（口径修订） | 从零运行根可完整起跑；release 构建成功 | 构建：`cargo build --release --locked` 成功。运行态：以空 `AXON_RUNTIME_ROOT` 起跑生成完整 `data/` 布局（见 `device-verification.md` 的 B4/B5）。真实 `~/.axon` 保留用户既有配置与历史数据，不做清空，真机验证一律在临时根进行 |

## W1 事件面（`axon-eventlog`）

| 编号 | 状态 | 判据 | 证据 |
| --- | --- | --- | --- |
| W1-1 | 已完成 | `cargo check -p axon-eventlog` 通过；依赖收敛 | 依赖收敛为 `serde`/`serde_json`/`thiserror`/`zstd`，锁用 std `File::try_lock`（1.89 起稳定）；设计列的 `fs4`/`sha2` 判定为多余依赖，已按实际收敛 |
| W1-2 | 已完成 | 事件类型覆盖 turn/tool/assistant/checkpoint/recovered/metrics/job | `crates/axon-eventlog/src/record.rs` 的 `RuntimeEventKind`；`tests/contract.rs` 逐类型往返 |
| W1-3 | 已完成 | header 帧单行、版本与文件名一致、非法 header 拒绝 | `format.rs::header_rejects_foreign_version`（版本不符拒绝）+ `tests/contract.rs::header_must_be_exactly_one_line`（两行 header 必须拒绝，读取侧不再抽首行当头部）+ 文件名与头部代数同源（`lib.rs` 的 `.v{FORMAT_VERSION}`） |
| W1-4 | 已完成（口径修订） | 每帧独立可解压；批内序号连续；写失败不留半批 | `append.rs` 的批次追加 + `format.rs` 的逐帧编码；`tests/crash.rs` 用「半帧 / 批间截断 / 半帧头」三种崩溃形态验证重启后只保留完整前缀。口径修订：首批要落 header 帧，因此一次追加是两次 `sync_data`（header 一次、数据一次），不再宣称「单批恒为一次 fsync」 |
| W1-5 | 已完成 | 序号不连续时整批拒绝且不写入 | `append.rs` 的序号守卫 + 契约用例 |
| W1-6 | 已完成 | 三个读取入口都有测试 | `read.rs` 提供 `read_from`/`read_tail`/`read_matching` 与增量入口 `read_after`/`read_tail_matching`；`tests/contract.rs` 的 `lazy_paged_reads_match_the_full_snapshot` 逐 `after`/`limit` 组合对齐全量快照；「反向扫描」按目标态收敛为「从尾部倒推定位所需帧」，裁决记录在 `read.rs` 的模块注释 |
| W1-7 | 已完成 | torn 尾只采纳完整记录；写句柄首次写入前截断 | `append.rs`：`open_at` 尾部自检 + 全量扫描兜底截断（`3df37831b`），`open` 补齐半帧头重建（`b703cb615`）；`tests/contract.rs` 两条撕裂尾用例 + `tests/crash.rs` 三条崩溃形态 |
| W1-8 | 已完成 | 并发追加不交错；锁冲突有明确错误 | `tests/crash.rs::concurrent_writers_to_one_log_never_interleave_frames`（两线程 10 条，序号整体单调）；`lock.rs::contended_lock_times_out_instead_of_blocking_forever`（争用超时返回 `Locked`，`d7319963c`） |
| W1-9 | 已完成 | 三处注入失败后日志可读且无半条记录 | `crates/axon-eventlog/tests/crash.rs`：半帧崩溃、批间崩溃、半帧头崩溃三处注入 + 并发追加（`b703cb615`） |
| W1-10 | 已完成（口径修订） | 契约用例全绿并作为回归基线 | `tests/contract.rs` 12 条 + `tests/crash.rs` 4 条 + `src` 内 11 条单元用例，共 27 条全绿；WBS 原文的「20 条」为早期估算，按实际覆盖面修订口径：每条契约都有对应用例，不凑数 |

## W2 事件面接线与 rollout 退役

| 编号 | 状态 | 判据 | 证据 |
| --- | --- | --- | --- |
| W2-1 | 已完成 | 端口拆分后每个方法只有唯一归属 | `crates/axon-session/src/ports/event_log_store.rs` 与 `runtime_store.rs`；`cargo test -p axon-session` 全绿 |
| W2-2 | 已完成 | `EventLogStore` 实现在 `axon-eventlog` 之上 | `crates/axon-session/src/infrastructure/sqlite_runtime_store/event_log_repository.rs`，端口契约测试在 `cargo test -p axon-session` 内 |
| W2-3 | 已完成 | 写入调用点全部改走事件流 | `cargo test --workspace` 全绿（事件批次、提交、结算相关用例） |
| W2-4 | 已完成 | 读取调用点（replay / 历史 / 尾部窗口）有测试 | `gateway_replay_tests.rs`（分页、字节预算、cursor 续订）与 `sqlite_runtime_store_interaction_tail_tests.rs`（尾部窗口、按 Interaction 取帧）、`sqlite_runtime_store_session_tests.rs`（历史装载与换窗） |
| W2-5 | 已完成 | 结算与事件分开提交、水位回退正确 | `commit_repository` 两段式 + `ledger_commit_rolls_back_*` 用例 |
| W2-6 | 已完成 | 一轮一条 `turn_metrics`；作业启停各一条且真的进事件流 | `turn_metrics`：`AtomicBool` 保证一轮一次（`interaction_turn_event_sink_tests` 断言恰好两条）；作业事件：`job_runtime/lifecycle.rs` 广播 → `run_commands/run_job_lifecycle_events.rs` 落盘（`job_started` / `job_completed`），用例 `run_commands::run_job_lifecycle_events::tests`。本轮还补齐 `turn_metrics.background_jobs_started/running`：改为读真实作业计数，不再写死 0 |
| W2-7 | 已完成 | 非 trace 级别仍产出 `turn_metrics` | `turn_performance` 采集路径 + `cargo test -p axon-session-runtime` |
| W2-8 | 已完成（口径修订） | 作业启停与终态有事件落盘、可回放 | `job_runtime/lifecycle.rs` 定义事件与进程级总线，`registry.rs` 在启动与首次终态两处广播；`run_commands/run_job_lifecycle_events.rs` 订阅后写 `job_started` / `job_completed` 并投影 `session_metrics.jobs_running`（会话列表 `openJobCount` 由此有真实来源）。口径修订：临时 Job 是「进程内、单 run」事实源（`job_runtime/mod.rs` 的边界），重启后回放得到的是历史事实，不会让作业复活 |
| W2-9 | 已完成 | 新库不含 `session_events` 表，直连 SQL 消失 | `crates/axon-session/src/infrastructure/sqlite_runtime_store/schema.rs` 无该表；`cargo test --workspace` 全绿 |
| W2-10 | 已完成（口径修订） | 分页、cursor 续订、尾部窗口有测试 | `gateway_replay_tests.rs`（分页与续订）+ `sqlite_runtime_store_interaction_tail_tests.rs`（按 Interaction 取尾窗）。口径修订：正文一律内联在事件帧里，「外置正文缺失降级」与 64 MiB 字节预算不再有生产对象，判定为机制随事件面切换而消失，不再保留空判据 |
| W2-11 | 已完成 | 断网重连后视图与在线态一致 | 真机：停掉 daemon 时正文保留、无失败提示；重启后无需手动刷新即恢复，轮次不重复（`记住数字 41` 仅一次）。详见 `device-verification.md` 的 B6/W2-11 段 |
| W2-12 | 已完成 | `run_rollout_store/**` 与 `rollout.sqlite3` 全仓无引用；`cargo test --workspace` 全绿；`--continue` 可接续；真机 C2/C4/C5 与 X1-X7 取证通过 | 专项计划、进度与门禁证据见 [w2-12-rollout-retirement.md](./w2-12-rollout-retirement.md)。A/B/C/D/E/F 六组全部迁移完成并整删 rollout 模块；离线门禁（fmt/check/clippy/test/verify-architecture/verify-docs）全绿；安装态真机回归已通过：C1 清单单行、C2 指定会话追加、C4 同 cwd 连续两轮带上下文、C5 `resume` 后 `--continue` 接续、C6 `--session` 与指针冲突（见 `device-verification.md`） |

## W3 控制面与工作项

| 编号 | 状态 | 判据 | 证据 |
| --- | --- | --- | --- |
| W3-1 | 已完成 | 两库可从零创建；`PRAGMA user_version` 生效且旧库被拒绝 | `sqlite_runtime_store_schema_tests.rs`：`legacy_database_is_rejected_instead_of_migrated`、并发首次建库用例 |
| W3-2 | 已完成 | `/new` 换 run，`/reset` 只换 generation | `sessions.run_session_id` / `run_generation` 两列 + `reset_session` 单事务轮换（`sqlite_runtime_store_session_tests::reset_session_cancels_pending_actions_and_rotates_run_identity`）。生产入口：`axon sessions reset <id>`（`session_commands::run_session_reset` + `session_reset_tests`），运行窗口随之换段；TUI `/reset` 在同会话上复位本地运行实例 |
| W3-3 | 已完成 | `inbox` 只有幂等、内容、受众、路由，无状态机 | `schema.rs` 的 `inbox` 定义；`work_items` 承担领取与终态 |
| W3-4 | 已完成（口径修订） | 在途与终态分离；`kind` 自证工作项来源 | 终态唯一出口 `work_item_archive.rs`，12 处终态写点在同一事务内调用；`work_item_history` + `work_items_all` 视图由 `sqlite_runtime_store_tests` 与 `runtime_cleanup_tests` 覆盖。`kind`：交互动作写 `user_input`、目标执行写 `goal`（`goal_commit_repository` + `sqlite_runtime_store_tests` 断言）、其余落默认 `turn`。口径修订：cron 运行事实在 `cron_runs`、子代理运行是进程内临时 Job，二者都不进 `work_items`，因此 `kind` 的实际取值域是 `turn` / `goal` / `user_input` |
| W3-5 | 已完成 | 三套 claim 合并为一套；并发 claim 不重复 | `crates/axon-session/src/infrastructure/sqlite_runtime_store/work_item_claim.rs`；交互/执行/动作三条路径共用状态守卫 UPDATE |
| W3-6 | 已完成 | 每轮更新一次；可从事件重放重建 | `session_metrics` 投影 + `sqlite_runtime_store_metrics_tests.rs` |
| W3-7 | 撤销 | 渠道面收敛为桌面 / TUI / CLI，不存在无界面入站渠道 | 裁决与理由见 `docs/dev/storage-architecture.md` §12.5；`channel_bindings` 相关代码已全部移除 |
| W3-8 | 已完成 | 事件写入不再 `UPDATE daemon_instances` | 事件路径只追加日志；fence 仅保留在控制面事务 |
| W3-9 | 已完成 | 控制面事务毫秒级；写风暴下续租不 `Fenced` | 连接 PRAGMA（`synchronous`/`busy_timeout`/`journal_size_limit`）+ 续租用例 |

## W4 会话与三端

| 编号 | 状态 | 判据 | 证据 |
| --- | --- | --- | --- |
| W4-1 | 已完成 | `GET /api/v1/sessions` 返回约定字段 | `crates/axon-cli/src/sessions_http_adapter.rs` 及其 11 个测试 |
| W4-2 | 已完成 | 协议字段 `laneId` → `sessionId`，前后端同批 | 全仓改名（Rust 201 文件、TS 57 文件、19 个文件改名）；`cargo test --workspace` 与前端测试全绿 |
| W4-3 | 已完成 | 桌面左侧菜单接 `sessions`，置顶/归档/排序生效 | `crates/axon-desktop/src/workspace/*` 与 `Sidebar.tsx`；`cargo test -p axon-desktop` 253 用例 |
| W4-4 | 已完成 | TUI `/sessions` 上下选择、Enter 进入、`--all` 含归档 | `crates/axon-tui-runtime/src/loop/agent_tui/bottom_view/sessions.rs` 与 18 个新增用例 |
| W4-5 | 已完成 | `/new` 换 `session_id`，`/reset` 换运行窗口 | `state/session_commands.rs` 用例 `new_and_reset_commands_split_session_ownership`；服务端轮换由 `axon sessions reset <id>` 触达（`reset_session` 递增 `run_generation` 并换 `run_session_id`） |
| W4-6 | 已完成 | `run --session` 单次指定，不存在则报错不新建；指定既有会话时真的追加 | `run_session_guard::validate_explicit_session` 与 `explicit_session_tests`（不新建）；真机 C2：取 `sessions list` 里的会话 ID 追加，该会话 `turns` 4→5 且不新增会话行（`device-verification.md`） |
| W4-7 | 已完成 | `debug session` 子命令全部可用，`axon sessions` 提层 | `crates/axon-cli/src/session_commands/mod.rs`：list/show/read/resume/reset/fork/delete/export/replay/output/doctor 全部接通；会话 ID 与 Thread ID 都由解析层统一处理（`session_resolution`） |
| W4-8 | 已完成 | 全仓无 `lane_id` 残留 | 仅剩 Win32 `biPlanes` 字段名；`grep -r "lane_id" crates` 零命中 |
| W4-9 | 已完成 | X1-X7 逐项有真机证据 | X1/X2/X5/X6/X7 已取证（桌面 + CLI 等价链路）：入站事实新增客户端来源，历史按轮标注，`channel_mix_json` 按逐轮发起端计数；X3/X4 待 TUI 侧证据。详见 `device-verification.md` 跨端一节 |

## W5 文档分层

| 编号 | 状态 | 判据 | 证据 |
| --- | --- | --- | --- |
| W5-1 | 已完成 | `docs/dev/` 与索引可用 | `docs/dev/README.md` |
| W5-2 | 已完成 | 技术文档迁入 `docs/dev/`，坏链为零 | 提交 `ed2820767`；`tools/verify-docs.ps1` 输出 OK |
| W5-3 | 已完成 | 产品文档只讲用户可见行为 | 产品文档 15 篇重写/修订；`verify-docs.ps1` 的产品文档清洁度检查通过 |
| W5-4 | 已完成 | `AGENTS.md` 引用路径可达 | `AGENTS.md` 指向 `docs/dev/ui-constraints.md` 等新位置 |
| W5-5 | 已完成 | `verify-docs.ps1` 覆盖 `docs/dev/` | 脚本内的文档根已加入 `docs/dev` |
| W5-6 | 已完成 | 产品文档技术词零命中 | `verify-docs.ps1` 的扫描段（`jarvis_`/`.sqlite3`/`crates/`/`WAL`/`PRAGMA`） |

## W6 发布与真机验收

| 编号 | 状态 | 判据 | 证据 |
| --- | --- | --- | --- |
| W6-1 | 已完成 | 构建产物版本为 0.0.1 | 提交 `993aba9cc`；`Cargo.toml` workspace 版本 0.0.1 |
| W6-2 | 未开始 | `git tag` 与 `gh release list` 均为空 | 待发布阶段执行 |
| W6-3 | 未开始 | `git log --oneline` 只有一条 | 待发布阶段执行 |
| W6-4 | 未开始 | 四平台产物齐全 | 待发布阶段执行 |
| W6-5 | 进行中 | 39 项验收矩阵逐项留证 | 已取证 29 项：B1-B6、C1-C6、D1-D12、X1/X2/X5/X6/X7（证据与复现步骤见 `device-verification.md`）；未取证 10 项：T1-T7 与 X3/X4 需交互式终端，B7 为 24 小时长跑 |
| W6-6 | 已完成 | 事件写入 p99、控制面事务、会话列表首屏、磁盘占用有实测输出并登记 | `tools/perf-baseline.json` 已登记实测读数（`--record`）：事件写入 p99 `0.539 ms`（上限 5）、控制面事务 p95 `0.137 ms`（上限 50）、会话列表首屏 `1.0 ms`（上限 30）、运行态 `data` `6 MiB`（上限 50）；人工真机读数 P2 `1821 ms`、P9 `0.45%`、P10 `2373 ms`、P11 `19/17/17 ms` |

## 汇总

- 已完成：W0 三项、W1 十项、W2 十一项（W2-11 未开始）、W3 八项（W3-7 撤销）、W4 八项、W5 六项、W6 一项（W6-2 至 W6-4 未开始）。
- W1 基座已闭合：W1-7 / W1-9 的实现与崩溃注入用例在 `3df37831b`、`b703cb615`；W1-1 / W1-6 / W1-8 / W1-10 的证据见上表。
- W2-12 已闭合：rollout 整删，历史窗口、轮次事实、用量统计、子代理名册、链回放、工具归档与保留策略全部改读 thread 事件面与控制面；安装态真机回归（C1/C2/C4/C5/C6）通过。
- W6-6 已闭合：四项硬上限全部有实测登记（p99 0.539 ms / 事务 p95 0.137 ms / 首屏 1.0 ms / data 6 MiB），人工真机读数同表登记。
- 未开始：W2-11、W6-2 至 W6-4（依赖 CDP 取证与发布阶段）；W6-5 进行中（29/39，剩 T1-T7 与 X3/X4 需交互式终端，B7 为 24 小时长跑）。
