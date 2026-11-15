# Turn 生命周期加固：目标与实施计划

面向后续执行窗口的交接文档。目标是把「轮次卡住后系统无法自愈」这一类问题从根上收敛，而不是逐次打补丁。

## 一、两次事故的事实

| 时间 | 会话 | 现象 | 根因 |
|---|---|---|---|
| 10:46 | `588b559c` | turn 停在 Active 67 分钟，assistant 零字符，派出 3 个审计子代理全部空回执 | provider 配额耗尽（429 RESOURCE_EXHAUSTED），进程随后退出，无人改判 |
| 18:39 | `bb9c03ff` | 「正在连接」持续数分钟，重启 daemon 无效 | provider 单次交换 170 秒；用户 ESC 中断后 turn 恒 Active；空闲压缩因判据要求「最后 turn 非 Active」而死锁 |

两次的共同点：**没有任何机制在执行者失去能力后把 turn 改判为终态**。

## 二、目标：四条不变量

| 编号 | 不变量 |
|---|---|
| INV-1 | 任何 `Active` 的 turn 必须有一个当前有效的执行者 |
| INV-2 | 执行者一旦失去执行权，turn 必须在有限时间内被**第三方**改判终态，绝不依赖执行者自己善终 |
| INV-3 | 客户端只信服务端快照，不存在「前端自认为还在跑」的第二套真相 |
| INV-4 | 以上机制在热路径（每个 chunk、每次工具调用）上的开销为零 |

## 三、已完成

- 空闲压缩整体下线（理由见第五节第 1 条）：配置键、通知账本与投影类别一并移除，压缩只保留 turn 内联一条路径。
- 现存且已核实的收口原语：`RuntimeStore::reap_dead_hosts` 回收过期宿主、`recover_expired_sessions` 接管失效会话、`ThreadService::interrupt_turn` 主动中断、`repair_open_turn` 配 `TurnRepairOutcome` 按历史是否平衡决定改判还是零写。`build_daemon_host` 只在启动时调一次 `reap_dead_hosts`，**没有任何孤儿轮次对账**。

> 早前版本本节曾声称已有 `RuntimeStore::list_orphaned_sessions`、`crates/axon-cli/src/run_commands/run_turn_reconciliation.rs` 与启动对账接线（`TURN_RECONCILE_BATCH_LIMIT`）。经全仓核验，这三项都不存在，本节已按实际状态改写。

## 四、待实施

### 4.1 巡视对账（运行期自愈）

孤儿轮次目前没有任何对账，启动与运行期都不会被改判。补法分两步：先写一个巡检函数，参考 `recover_expired_sessions` 的失效判据（宿主记录缺失、代际对不上、宿主租约过期）但只读不接管，对最后一轮仍是 `Active` 的调 `ThreadService::interrupt_turn` 写终态并打 WARN；再在 `daemon_host_runner_workers.rs` 的 `run_supervisor_loop` 里照既有 `*_if_due` 模式每 30 秒跑一次。

### 4.2 turn 级心跳（覆盖 provider 挂住）

当前判据只覆盖「执行者没了」（宿主缺失、代际落后、租约过期），覆盖不了「宿主活着但 provider 挂住」：那种情况宿主租约仍然新鲜，对账不会命中。

补法：provider chunk 与工具事件更新内存 `last_progress: Instant`，每 60 秒续约时顺带落库；巡视判据加上「轮次开始已久且 `last_progress` 停滞超过 10 分钟」。禁止每个 chunk 写库。

常量：停滞阈值 10 分钟、巡视间隔 30 秒，均按常量实现，不做配置键。

### 4.3 客户端收敛

改判后的终态由既有 replay 与 WS 通道送达；需确认「桌面端正盯着这一轮时，改判事件能即时到达」，而不是只靠下次打开会话时的回放补帧。

### 4.4 已确认的接口（供续做）

- `ThreadService::new(store)`（`pub const fn`）、`interrupt_turn(&self, root: &Path, InterruptTurnCommand)`。
- `InterruptTurnCommand { thread_id: ThreadId, turn_id: String, recorded_at_ms: u64 }`。
- `Turn { turn_id: TurnId, sequence: u64, status: TurnStatus, .. }`，`TurnStatus::Active`。
- 读取线程事实用 `ThreadService::new(JsonlThreadEventStore).read_thread(&root, thread_id)`；原先的 `load_thread_detail` 已随空闲压缩下线一并移除。
- `session_runtimes.session_id` 与 thread id 同值，可直接作为 `read_thread` 的入参。

## 五、已核实的关键事实

1. **压缩只剩 turn 内联一条路径**（`run_provider_loop/inline_compaction.rs`），由 provider 调用前的预算判定与模型 `new_context` 请求触发。原先 turn 收口后按 debounce 排期的空闲压缩已整体下线：它的调度入口本来就在收口路径上，turn 卡住时永不排期；而结果又必须等下一次 turn 才能补投，会话结束后通知只能永久滞留。内联路径没有这两个缺口，故障事实直接落在当轮的投影通道里。
2. **turn 状态存在 JSONL 线程存储**（`ThreadDetail` / `TurnStatus`），不在 `session_runtimes` 里。改判只能追加终态事件，不能用 SQL UPDATE。
3. `session_runtimes` 有 `owner_host_id`、`owner_generation`、`last_activity_at_ms`，**没有** `lease_expires_at_ms`；租约字段在 `jarvis_interactions`。
4. 现成可复用：`ThreadService::interrupt_turn`、`project_interrupted_interactions`、`daemon_host_tick_report.interrupted_interactions`、`supervisor_tick`。
5. `run_supervisor_loop` 已具备 `cleanup_releases_if_due` / `checkpoint_wal_if_due` / `sample_and_broadcast_lsp_if_due` 的间隔执行骨架。

## 六、验收

1. 重启 daemon 后，现有孤儿轮次在启动对账里一次改判为 `interrupted`，桌面端不再转圈；
2. 正常长轮次不被误判（跑一个超过两分钟的请求，观察不被中断）；
3. 长对话 `provider_ms` 与改动前持平，证明心跳没有带来写放大；
4. `cargo fmt`、`cargo check`、`cargo clippy`、`cargo test`、架构门禁、文档门禁全绿；
5. 安装态真机验证。

## 七、明确不做的

| 不做 | 理由 |
|---|---|
| 引入外置工作流引擎 | 单机 daemon 用 SQLite 与租约足够，复杂度高出一个量级 |
| 每个 chunk 落库心跳 | 会把写放大乘到 token 流上 |
| 新增数据表或常驻线程 | 复用 `session_runtimes` 与 `supervisor_tick` |
| 客户端自建超时判定 | 会造出第二套真相，与 INV-3 冲突 |
| 让 provider 传输层兜底 | 它不该知道执行权语义 |
| 把压缩调度迁到巡视 | 压缩已收敛为 turn 内联单一路径，不再有独立调度入口需要迁移 |
