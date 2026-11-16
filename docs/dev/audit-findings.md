# 交叉审计问题清单（第 16 轮）

> 两份独立只读审计的合并结果：性能与闭环（A/B/C 组）、稳定性与一致性（S 组）。交集项只保留一条并注明双编号。
>
> 修复顺序按严重度，不按编号。每条都给出复核命令；修完一组后在 `docs/dev/wbs-acceptance.md` 更新对应条目并补证据，**不得**在证据缺失时标已完成。

## P0 阻断项（不修不能装机）

| 编号 | 位置 | 问题 | 复核 | 修法 |
| --- | --- | --- | --- | --- |
| P0-1（S-02/A-1）**已修** `4eb48516c` | `work_item_archive.rs:31`、`interaction_repository.rs:418/460` | 归档按 `execution_id` 定位，交互路径传 `interaction_id`，两值永不相等 → 归档恒空操作，终态永不回收 | `git grep -n "archive_work_item" -- crates` | 先 `SELECT execution_id FROM work_items WHERE inbox_id=?` 再归档；或让归档函数按 `inbox_id` 分支。同时去掉 `release_work_item` 里的归档调用（那是回 queued） |
| P0-2（A-2）**已修** `f28653c98` | `commit_repository.rs:143-157`，以及 `failure_repository.rs:49`、`action_repository.rs:141`、`goal_repository.rs:156/209` | turn 结算主路径只 UPDATE 状态，**从不归档**；其余终态写点同样缺失 | `git grep -n "archive_work_item" -- crates` | 收敛成单一入口 `settle_work_item(tx, work_id, terminal_kind)`，所有终态写点在同一事务里调用。实际落地：新增 `archive_settled_work_item`（只搬 completed/failed/cancelled/dead_letter）作为唯一归档出口，补齐 9 处写点；补偿扫描改读 `work_items_all` |
| P0-3（S-01）**已修** `3df37831b` | `append.rs:172-215`（`open_at`） | 生产写入不扫 torn 尾、不截断；崩溃后新帧写在撕裂字节之后，读侧扫到 torn 即停，之后历史永久静默不可见 | `git grep -n "append_once\|\.writer()" -- crates` | `open_at` 复用 `read_snapshot` 的截断逻辑（先截断 + `sync_data`）；补「撕裂尾后再追加再读」契约用例。实际落地：尾部 1 MiB 自检 + 全量帧扫描兜底，两条契约用例 |
| P0-4（A-3）**已修** `f8c56a305` | `runtime_cleanup_families.rs:277-286` | 会话族删除前的活跃判定不过滤状态，任何跑过一轮的会话都命中 → 清理永远 `Err`，会话族一套都删不掉 | `git grep -n "work_items WHERE session_id IN" -- crates/axon-cli` | 判定加 `AND status IN ('queued','waiting','running')`，与积压口径一致。实际落地：复用 `EXECUTION_ACTIVE_STATUSES`，并在同一事务内回收残留终态行以满足 `work_items.session_id` 外键 |
| P0-5（S-09/A-4）**已修** `68dfea535` | `runtime_cleanup.rs:336-341`、`411-426` | 清理链在控制面库执行 `SELECT ... FROM session_events`，该表已删 → dry-run 每次必失败，且被 `?` 上抛 | `git grep -n "session_events" -- crates/axon-cli/src/cron_commands` | 删除 `count_runtime_session_event_overlap` 与 `session_event_expiry_predicate` |
| P0-6（A-5）**已修** `022772c48` | `workbench_http_session_readings.rs:157-170` | SQL 引用 `inbox.status`（该列已随状态机删除），prepare 即失败并被 `.ok()` 吞掉 → 桌面「累计耗时」恒 null | 对照 `schema.rs` 的 `inbox` DDL | 改读 `session_metrics.total_active_ms`；`.ok()` 读数补错误日志 |
| P0-7（S-07）**已修** `f28653c98` | `work_item_claim.rs:45-46` | turn 工作项 `session_id` 写死 NULL → 取消、清理、会话保护谓词全部漏判 | `git grep -n "UPDATE work_items" -- crates` | 建工作项时写入 `interaction_session_id()` 的结果。实际落地：INSERT 里用 `inbox` 与 `sessions` 的联合子查询取值（外键安全、未绑定 Workspace 折回 NULL） |

## P1 稳定性与一致性

| 编号 | 位置 | 问题 | 修法 |
| --- | --- | --- | --- |
| P1-1（S-03/B-5） | `event_log_repository.rs`、`runtime_repository.rs:422` | 无「水位 vs 日志尾」对账，`session_recovered` 无生产者 | 启动时对账：日志尾大于水位则回退水位并写 `session_recovered` |
| P1-2（S-04） | `commit_repository.rs:203-212` | 两段式只有正向，文件写失败时控制面已提交且无补偿 | 文件写失败走补偿事务回滚水位与状态 |
| P1-3（S-05/B-6） | `event_log_repository.rs:107-149`、`read.rs:40-60` | 读侧不过滤 generation；`read_snapshot` 只在帧内校验序号，跨帧不连续无人拒绝 | 读侧按当前 generation 过滤；跨帧维护单调 `expected` |
| P1-4（S-08）**已修** `256f732c0` | `runtime_event_archive.rs:19-25` | 高水位算成 16 GiB（上限 4 GiB 的 4 倍），且淘汰不看会话是否活跃 | 改为 `MAX*8/10`；淘汰前排除活跃会话 |
| P1-5（S-06/C-8） | `event_log_repository.rs:288` | `let _ =` 吞 `session_metrics` 投影错误，且无重建入口 | 失败上报；补 `rebuild_session_metrics(session)` 与测试 |
| P1-6（S-10）**已修** `4e871981e` | `execution_worker_settlement.rs:168`、`execution_worker.rs:430/435` | 账本事件写入失败被 `let _ =` 吞 | 失败上报并写补偿事件 |
| P1-7（S-11）**已修** `e9883a436` | `workspace_files_tests.rs:130-151` | 单测直接写真实用户主目录 | 注入假 home（`test_temp_root`）或只测纯函数 |
| P1-8（B-1）**已修** `b61804187` | `schema.rs`（`inbox`） | 缺 `(session_id, received_at_ms DESC, inbox_id DESC)` 索引；实测会话列表 71.3 ms / 历史页 31.1 ms，门槛 30 ms | 加索引并递增 control 版本；滤镜下推 SQL。实测：200 会话×20 万入站事实，有索引 1 ms / 无索引 10452 ms |
| P1-9（B-2） | `health_repository.rs:12-21` | `/system/status` 每请求做三表全扫 + `work_items_all` 全量 GROUP BY（实测 35.4 ms） | 改有界索引查询或投影列 |
| P1-10（B-3） | `read.rs:23-27`、`event_log_repository.rs:127-149` | 每次读都把整个日志读入内存并全量解压，分页 replay 每页重做 | 加 `.idx` 或惰性解压；分页复用 reader |
| P1-11（B-4） | `event_log_repository.rs:184-188/289` | `origin_channel`/`origin_kind` 硬编码，`channel_mix_json` 恒为 internal | 从 `inbox.source_channel` 解析后传入 |
| P1-12（B-10）**已修** `b1f6cfe6d` | `docs/skills/operate-axon/references/*.md` | 安装态 Skill 仍写 `/api/v1/lanes/<laneId>/...`，实际路由是 `/api/v1/sessions/<id>/...` | 同步路由与 `sessionId` 命名，核验安装态副本 SHA-256 |
| P1-13（B-8） | `tools/verify_performance.py` | 已加四项硬上限，但读数仍是人工登记，未接自动测量 | 事件写 p99 用 `cargo test --ignored` 产出；首屏用列表 SQL 计时；体积已自动 |

## P2 写放大与原则残留

| 编号 | 位置 | 问题 |
| --- | --- | --- |
| P2-1（S-12/C-1） | `commit_repository.rs:174-190` | 每轮写 `session_checkpoints` 但全仓无读取；应删除该表与 INSERT |
| P2-2（C-2） | `interaction_turn_event_sink.rs:283-285` | `turn_metrics` 单独一批写入，多一次事务与 fsync |
| P2-3（C-3） | `event_log_repository.rs:255-271` | `write_records` 每条一次 `append_once`，N 条即 N 次 fsync |
| P2-4（C-4） | `schema.rs:303-376` | `goal_result_reads` 未并入 `goal_results`；`work_items` 主键仍叫 `execution_id` |
| P2-5（C-5） | `schema.rs:122-127` | 版本相等时仍执行 DDL 补表 = 补表迁移通道，与「不迁移」冲突；已有测试把它固化（`legacy_database_without_continuation_table_gains_it_on_open`） |
| P2-6（C-6） | `sessions_http_adapter.rs:41` | 每请求新建连接池，未复用 `RUNTIME_STORES`；滤镜在 LIMIT 之后生效 |
| P2-7（C-7） | `lock.rs:19-31` | 写锁阻塞无超时，卡死会让该会话追加无限阻塞 |
| P2-8（C-9） | `workbench_http_session_readings.rs:1-10` | 注释仍把 rollout 当用量账本；`rollout_current_session` 与 `resume_pointer` 双存储 |
| P2-9（S-13） | `runtime_cleanup_families.rs:287-308` | 会话族清理不删 `session_metrics`/`inbox`/`resume_pointer`，留孤儿 |
| P2-10（S-15） | `schema.rs:380-383` | `COALESCE(session_id, goal_id)` 对 turn 项为 NULL，去重索引失效 |
| P2-11（S-16） | `read.rs:24/43-57` | 两套序号权威并存；`read_snapshot` 全量入内存 |
| P2-12（A-2/B-7） | rollout 读取点 30+ 处 | W2-12 未完成，用量/模型/子会话/治理仍读 rollout；读点需逐点登记迁移目标 |

## 台账改判（已完成部分）

W1-7（`recover.rs` 不存在）、W1-9（`tests/crash.rs` 不存在）→ **未完成**；W1-1（依赖不符）、W1-6（无 `scan_reverse`）、W1-8（无并发用例）、W1-10（7 条非 20 条）→ **待补证据**；W3-4（归档恒空）→ **未完成**。

仍需改判：W2-5（无文件写失败补偿）、W2-8（`job_started` 无生产者）、W3-5（三套 claim 只合两套、无并发用例）、W3-6（无重建实现）、W2-12（指针未被 `--continue` 消费）。W2-9（`session_events` 直连 SQL 仍在清理链）已随 P0-5 合上。

## 结论

结构方向成立（事件面一批一次 fsync、控制面两段式、版本不符拒绝打开），但**闭环没闭合**：终态搬不进历史表、清理被残留行挡住、清理 SQL 打在已删表上。在这些修复前装机只能验「界面能开」，验不了「存储重构成立」。
