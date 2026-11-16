# WBS 验收台账

> 逐项登记 `docs/dev/reconstruction-wbs.md` 中每个条目的状态、判据与证据。
>
> 状态口径：**已完成**表示判据已被可复跑的证据覆盖；**进行中**表示实现已落地但判据尚未全部取到；**未开始**表示尚未实施；**撤销**表示经裁决不再实施。
>
> 证据口径：只写可以直接复跑或直接查阅的东西（测试名、命令、提交号、文件位置）。真机证据与离线证据分开标注，离线门禁不得冒充真机通过。

## W0 基线与准备

| 编号 | 状态 | 判据 | 证据 |
| --- | --- | --- | --- |
| W0-1 | 已完成 | 分支存在、`master` 无新提交 | 开发在 `master` 上按重构条目推进，工作树干净（`git status` 无改动） |
| W0-2 | 已完成 | `cargo fmt/check/clippy/test` 与 `verify-*` 全绿 | 基线提交 `f6bfafd2f`：`cargo test --workspace` 全绿、`cargo clippy --workspace --all-targets -- -D warnings` 无告警、`tools/verify-architecture.ps1` 与 `tools/verify-docs.ps1` 均输出 OK |
| W0-3 | 已完成 | 运行态可从头构建 | `~/.axon` 只保留配置、Skill、人格、记忆与用量；`cargo build --release --locked --workspace` 成功 |

## W1 事件面（`axon-eventlog`）

| 编号 | 状态 | 判据 | 证据 |
| --- | --- | --- | --- |
| W1-1 | 已完成 | `cargo check -p axon-eventlog` 通过；依赖收敛 | 依赖仅 `zstd`/`fs4`/`sha2`/`serde`；`cargo check -p axon-eventlog` 通过 |
| W1-2 | 已完成 | 事件类型覆盖 turn/tool/assistant/checkpoint/recovered/metrics/job | `crates/axon-eventlog/src/record.rs` 的 `RuntimeEventKind`；`tests/contract.rs` 逐类型往返 |
| W1-3 | 已完成 | header 帧单行、版本与文件名一致、非法 header 拒绝 | `format.rs` 单元测试 + `tests/contract.rs` 的 header 用例 |
| W1-4 | 已完成 | 单批一次 fsync、失败回滚、帧可独立解压 | `append.rs` 与 `tests/contract.rs` 的批次追加用例 |
| W1-5 | 已完成 | 序号不连续时整批拒绝且不写入 | `append.rs` 的序号守卫 + 契约用例 |
| W1-6 | 已完成 | `read_from` / `read_tail` / 反向扫描三个入口有测试 | `read.rs` 单元测试 + `tests/contract.rs` 分页用例 |
| W1-7 | 已完成 | torn 尾只采纳完整记录；写句柄首次写入前截断 | `recover.rs` 单元测试 |
| W1-8 | 已完成 | 两进程并发追加不交错；锁冲突有明确错误 | `lock.rs` + `tests/contract.rs` 的并发用例 |
| W1-9 | 已完成 | append / fsync / 截断三处注入失败后日志可读且无半条记录 | `crates/axon-eventlog/tests/crash.rs` |
| W1-10 | 已完成 | 契约用例全绿并作为后续实现的回归基线 | `cargo test -p axon-eventlog`（含 `tests/contract.rs`） |

## W2 事件面接线与 rollout 退役

| 编号 | 状态 | 判据 | 证据 |
| --- | --- | --- | --- |
| W2-1 | 已完成 | 端口拆分后每个方法只有唯一归属 | `crates/axon-session/src/ports/event_log_store.rs` 与 `runtime_store.rs`；`cargo test -p axon-session` 全绿 |
| W2-2 | 已完成 | `EventLogStore` 实现在 `axon-eventlog` 之上 | `crates/axon-session/src/infrastructure/sqlite_runtime_store/event_log_repository.rs`，端口契约测试在 `cargo test -p axon-session` 内 |
| W2-3 | 已完成 | 写入调用点全部改走事件流 | `cargo test --workspace` 全绿（事件批次、提交、结算相关用例） |
| W2-4 | 已完成 | 读取调用点（replay / 历史 / 尾部窗口）有测试 | `gateway_replay_tests`、`sqlite_runtime_store_event_*` 用例 |
| W2-5 | 已完成 | 结算与事件分开提交、水位回退正确 | `commit_repository` 两段式 + `ledger_commit_rolls_back_*` 用例 |
| W2-6 | 已完成 | 一轮一条 `turn_metrics`，作业启停各一条 | `crates/axon-eventlog/src/record.rs` 的 `TurnMetrics`；事件批次用例断言计数 |
| W2-7 | 已完成 | 非 trace 级别仍产出 `turn_metrics` | `turn_performance` 采集路径 + `cargo test -p axon-session-runtime` |
| W2-8 | 已完成 | 作业启停与终态可从事件重建 | `job_runtime` 事件落盘 + 会话事件回放用例 |
| W2-9 | 已完成 | 新库不含 `session_events` 表，直连 SQL 消失 | `crates/axon-session/src/infrastructure/sqlite_runtime_store/schema.rs` 无该表；`cargo test --workspace` 全绿 |
| W2-10 | 已完成 | 分页、字节预算、缺失正文降级、cursor 续订全绿 | `crates/axon-daemon/src/infrastructure/gateway_replay_tests.rs` |
| W2-11 | 未开始 | 断网重连后视图与在线态一致（需 CDP 真机证据） | 待 W6-5 真机验收时取证 |
| W2-12 | 进行中 | 删库后多轮对话回归全绿；`--continue` 可接续；无 `RolloutSqliteStore` 残留 | 恢复指针已迁入 `control.resume_pointer`（`run_resume_pointer.rs` 4 个测试）；thread 历史窗口实现已验证（1193 个 CLI 用例全绿）；**rollout 库与 `run_rollout_store` 尚未删除**，整删待完成 |

## W3 控制面与工作项

| 编号 | 状态 | 判据 | 证据 |
| --- | --- | --- | --- |
| W3-1 | 已完成 | 两库可从零创建；`PRAGMA user_version` 生效且旧库被拒绝 | `sqlite_runtime_store_schema_tests.rs`：`legacy_database_is_rejected_instead_of_migrated`、并发首次建库用例 |
| W3-2 | 已完成 | `/new` 换 run，`/reset` 只换 generation | `sessions.run_generation` 列 + `reset_session` 递增；TUI `/new`、`/reset` 用例 |
| W3-3 | 已完成 | `inbox` 只有幂等、内容、受众、路由，无状态机 | `schema.rs` 的 `inbox` 定义；`work_items` 承担领取与终态 |
| W3-4 | 已完成 | 在途与终态分离；`kind` 覆盖 turn/goal/cron/subagent | `work_items` + `work_item_history` + `work_items_all` 视图；`archive_work_item` 在结算与终态路径调用 |
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
| W4-5 | 已完成 | `/new` 换 `session_id`，`/reset` 只换 `run_generation` | `state/session_commands.rs` 用例 `new_and_reset_commands_split_session_ownership` |
| W4-6 | 已完成 | `run --session` 单次指定，不存在则报错不新建 | `run_session_guard::validate_explicit_session` 与 `explicit_session_tests` |
| W4-7 | 已完成 | `debug session` 子命令全部可用，`axon sessions` 提层 | `crates/axon-cli/src/session_commands/mod.rs`；CLI 1189 用例全绿 |
| W4-8 | 已完成 | 全仓无 `lane_id` 残留 | 仅剩 Win32 `biPlanes` 字段名；`grep -r "lane_id" crates` 零命中 |
| W4-9 | 未开始 | X1-X7 跨端场景全部有 `OK:` 证据 | 待 W6-5 真机验收时取证 |

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
| W6-5 | 未开始 | 39 项验收矩阵逐项留证 | 待装机后执行；未取得真机证据的条目标注「未真机验证」 |
| W6-6 | 进行中 | 事件写入 p99、控制面事务、会话列表首屏、磁盘占用有实测输出 | `tools/verify_performance.py` 已加入四项硬上限（p99 ≤ 5 ms、事务 p95 ≤ 50 ms、首屏 ≤ 30 ms、data ≤ 50 MiB）并能自动统计运行态体积；读数待装机后 `--record` 登记 |

## 汇总

- 已完成：W0 三项、W1 十项、W2 十项、W3 八项（W3-7 撤销）、W4 八项、W5 六项、W6 一项。
- 进行中：W2-12（rollout 退役剩整删）。
- 进行中（续）：W6-6（门禁脚本已具备四项硬上限，待装机取数）。
- 未开始：W2-11、W4-9、W6-2 至 W6-5（全部依赖装机与发布阶段）。
