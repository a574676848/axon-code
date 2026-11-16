# AXON 存储重构 WBS

> 本文是 [storage-architecture.md](./storage-architecture.md) 的执行分解，把 M0 到 M5 拆成可领取、可验收的任务。
>
> 工作量单位是"单人执行的名义人日"，只用于排序与判断粒度，不代表实际排期。每个任务都必须有可验证的交付物，否则不进入下一阶段。
>
> 位置说明：本文当前暂存 `docs/`，W5 阶段随技术文档一起迁入 `docs/dev/`。

## 使用方式

1. 按阶段顺序推进，阶段内任务可并行，但**阶段之间不跳步**：前一阶段的里程碑验收不通过，不进入下一阶段。
2. 每个任务完成时勾选，并在 PR 描述里贴上验收命令的输出。
3. 任何任务发现需要改计划，先改 `storage-architecture.md`，再改本文，最后改代码。

## 总览

| 阶段 | 主题 | 任务数 | 名义人日 | 里程碑 |
| --- | --- | --- | --- | --- |
| W0 | 准备与基线 | 3 | 0.5 | 基线可复现 |
| W1 | 基座 crate `axon-eventlog` | 10 | 3.5 | A：纯新增可独立验收 |
| W2 | 事件面切换 | 11 | 7.0 | B：事件不再进 SQLite |
| W3 | 控制面与工作项 | 9 | 6.0 | C：三库就位，队列合并 |
| W4 | 三端会话选择 | 9 | 5.0 | D：三端同会话 |
| W5 | 文档分层与产品化 | 6 | 3.0 | E：docs 成为产品 SSOT |
| W6 | 发布治理与真机验收 | 5 | 1.5 | F：v0.0.1 发布 |
|  |  | **53** | **26.5** |  |

---

## W0 准备（0.5 人日）

| # | 任务 | 交付物 | 依赖 | 验收 | 估时 |
| --- | --- | --- | --- | --- | --- |
| W0-1 | 建 `reconstruct` 分支并冻结 `master` 基线 | 分支存在，`master` 无新提交 | 无 | `git branch` 可见 | 0.1 |
| W0-2 | 跑通并记录全量门禁基线 | 门禁输出存档 | W0-1 | `cargo fmt/check/clippy/test` + `verify-*` 全绿 | 0.3 |
| W0-3 | 确认运行态已清空、可从头构建 | 构建产物与空 `data/` | W0-2 | `~/.axon` 只有保留集，`cargo build --release` 成功 | 0.1 |

---

## W1 基座 `crates/axon-eventlog`（3.5 人日）

纯新增 crate，不接入任何现有调用点。

| # | 任务 | 交付物 | 依赖 | 验收 | 估时 |
| --- | --- | --- | --- | --- | --- |
| W1-1 | crate 骨架与依赖 | `Cargo.toml`、`lib.rs` | W0 | `cargo check -p axon-eventlog` 通过；依赖仅 `zstd`/`fs4`/`sha2`/`serde` | 0.2 |
| W1-2 | 记录模型与 `RuntimeEventKind` 枚举 | `record.rs` | W1-1 | 覆盖 `turn_*`/`tool_*`/`assistant_message`/`checkpoint`/`session_recovered`/`turn_metrics`/`job_*` | 0.3 |
| W1-3 | header 帧编解码 | `format.rs` | W1-2 | header 帧只含一行；version 与文件名一致；非法 header 拒绝 | 0.3 |
| W1-4 | 批次追加：一帧一批 + checksum + fsync | `append.rs` | W1-3 | 单批一次 fsync；写失败回滚到原长度；帧可独立解压 | 0.5 |
| W1-5 | 序号连续性校验 | `append.rs` | W1-4 | 首条 seq 必须等于 next-seq，否则整批拒绝且不写入 | 0.2 |
| W1-6 | 读取 API：分页、尾部窗口、反向扫描 | `read.rs` | W1-4 | `read_from(seq)`、`read_tail(n)`、`scan_reverse` 三个入口有测试 | 0.5 |
| W1-7 | torn frame 检测与截断 | `recover.rs` | W1-6 | torn 尾只采纳完整记录；写句柄首次写入前截断并持久化 | 0.5 |
| W1-8 | 跨进程文件锁 | `lock.rs` | W1-1 | 两进程并发追加不交错；锁冲突返回明确错误 | 0.3 |
| W1-9 | 崩溃注入测试 | `tests/crash.rs` | W1-7 | 在 append / fsync / 截断三处注入失败，重启后日志可读且无半条记录 | 0.4 |
| W1-10 | 契约测试套件 | `tests/contract.rs` | W1-9 | 20 条契约用例全绿，后续实现改动必须保持 | 0.3 |

**里程碑 A**：`cargo test -p axon-eventlog` 全绿，`cargo check --workspace` 不受影响。

---

## W2 事件面切换（7.0 人日）

| # | 任务 | 交付物 | 依赖 | 验收 | 估时 |
| --- | --- | --- | --- | --- | --- |
| W2-1 | 拆 `RuntimeStore` 为 `EventLogStore` + `ControlStore` | `ports/event_log_store.rs` | 里程碑 A | 32 个方法各有唯一归属，编译通过 | 0.8 |
| W2-2 | `EventLogStore` 实现接 `axon-eventlog` | `infrastructure/event_log.rs` | W2-1 | 端口契约测试全绿 | 0.7 |
| W2-3 | 改 8 个写入调用点 | 见架构文档 §5.3 | W2-2 | 全量单测通过 | 1.0 |
| W2-4 | 改 9 个读取调用点 | 见架构文档 §5.4 | W2-2 | gateway replay / 历史加载 / 尾部窗口均有测试 | 0.8 |
| W2-5 | `commit_action` 两段式 | `commit_repository.rs` | W2-3 | 结算与事件分开提交；崩溃注入后水位回退正确 | 0.8 |
| W2-6 | `turn_metrics` / `job_started` / `job_completed` 事件 | `metrics.rs` | W2-3 | 一轮一条 `turn_metrics`；作业启停各一条 | 0.7 |
| W2-7 | 采集开关改造：`TurnPerformance` 无条件采集 | `turn_performance.rs` 调用点 | W2-6 | 非 trace 级别下仍产出 `turn_metrics` | 0.4 |
| W2-8 | `JobRuntime` 生命周期落事件 | `job_runtime` 与 sink | W2-6 | 启停与终态可查，进程重启后可从事件重建 | 0.6 |
| W2-9 | 删除 `session_events` 表与三个索引 | `schema.rs` | W2-5 | 新库不再含该表；直连 SQL 全部消失 | 0.4 |
| W2-10 | gateway replay 契约测试 | `gateway_replay_tests.rs` | W2-4 | 分页、字节预算、缺失正文降级、cursor 续订全绿 | 0.5 |
| W2-11 | 桌面端断线重连确认 | CDP 证据 | W2-10 | 断网重连后视图与在线态一致 | 0.5 |

**里程碑 B**：`session_events` 从代码里消失；重复运行 24 小时，事件目录体量与预期一致，控制面库不再随轮次增长。

---

## W3 控制面与工作项（6.0 人日）

| # | 任务 | 交付物 | 依赖 | 验收 | 估时 |
| --- | --- | --- | --- | --- | --- |
| W3-1 | 两库 schema 重写（audit 取消并入 control，理由见 docs/storage-architecture.md §4.4.3） | `control`/`outbox`/`audit` 各自 schema | 里程碑 B | 三库可从零创建；`PRAGMA user_version` 生效 | 0.8 |
| W3-2 | `sessions` 表（含 `current_run_id` / `run_generation`） | schema + repository | W3-1 | `/new` 换 run，`/reset` 只换 generation | 0.6 |
| W3-3 | `inbox` 表（去状态机） | schema + repository | W3-1 | 只有幂等、内容、受众、路由；无 `status`/`lease_token` | 0.6 |
| W3-4 | `work_items` + `work_item_history` | schema + repository | W3-3 | 在途与终态分离；`kind` 覆盖 turn/goal/cron/subagent | 0.9 |
| W3-5 | 统一 claim 逻辑 | `claim.rs` | W3-4 | 三套旧 claim 合并为一套；并发 claim 不重复 | 1.0 |
| W3-6 | `session_metrics` 投影 | schema + 更新管道 | W3-4 | 每轮更新一次；可从事件重放重建 | 0.8 |
| W3-7 | ~~`channel_bindings`~~ **撤销** | 不实施 | W3-3 | 渠道面收敛为桌面 / TUI / CLI 三端，三端都显式携带 `session_id`，不存在无界面入站渠道；绑定表与路由不实施，入站身份仍由 `session_id` 唯一决定 | 0 |
| W3-8 | 去掉事件路径上的 fence 写 | `helpers.rs` 调用点 | W3-3 | 事件写入不再 `UPDATE daemon_instances` | 0.3 |
| W3-9 | PRAGMA 调整（`synchronous`/`busy_timeout`/`journal_size_limit`） | 连接配置 | W3-1 | 控制面事务毫秒级；写风暴下续租不 `Fenced` | 0.6 |

**里程碑 C**：三库就位，`control.sqlite3` 在 24 小时运行后小于 50 MB。

---

## W4 三端会话选择（5.0 人日）

| # | 任务 | 交付物 | 依赖 | 验收 | 估时 |
| --- | --- | --- | --- | --- | --- |
| W4-1 | `GET /api/v1/sessions` 接口 | HTTP adapter | 里程碑 C | 返回 `sessionId`/`title`/`turnCount`/`totalActiveMs`/`lastTtftMs`/`openJobCount` | 0.6 |
| W4-2 | 协议字段 `laneId` → `sessionId` | `axon-protocol` + `frames.ts` | W4-1 | 前后端同批改；网关契约测试绿 | 1.0（**延后**：纯命名重构，Rust 与 TS 合计约 840 处引用，零行为收益；验收矩阵不检查字段名，先做 D/T/C 三组能力项） |
| W4-3 | 桌面左侧菜单接 `sessions` | desktop workspace 模块 | W4-2 | 点击切换会话，置顶/归档/排序生效 | 0.8 |
| W4-4 | TUI `/sessions` 弹窗 | `command_palette` + handler | W4-1 | 上下键选择、Enter 进入、`--all` 含归档 | 0.7 |
| W4-5 | `/new` 与 `/reset` 拆分 | `commands.rs` + handler | W4-2 | 前者换 `session_id`，后者只换 `run_generation` | 0.4 |
| W4-6 | CLI `run --session` | `run_commands` 参数 | W4-2 | 单次指定；不存在则报错不新建 | 0.5 |
| W4-7 | CLI `axon sessions` 提层 | `session_commands/mod.rs` | W4-6 | `debug session` 子命令全部可用 | 0.4 |
| W4-8 | 删除 `session_lane.rs` | 模块移除 | W4-2 | 全仓无 `lane_id` 残留 | 0.3（**延后**：与 W4-2 同一批命名重构；单独做会让 `lane_id` 与 `session_id` 并存更混乱） |
| W4-9 | 跨端同步验证（X1-X7） | CDP + TUI 快照 | W4-3 到 W4-7 | 七项跨端场景全部有 `OK:` 证据 | 0.3 |

**里程碑 D**：桌面、TUI、CLI 三端能进入同一会话，一端在聊另一端能看到。

---

## W5 文档分层与产品化（3.0 人日）

| # | 任务 | 交付物 | 依赖 | 验收 | 估时 |
| --- | --- | --- | --- | --- | --- |
| W5-1 | 建立 `docs/dev/` 与索引 | `docs/dev/README.md` | 里程碑 D | 目录存在，索引可用 | 0.2 |
| W5-2 | 技术文档迁移 | 10 篇移入 `docs/dev/` | W5-1 | 坏链为零 | 0.4 |
| W5-3 | 产品文档重写 12 篇 | `docs/*.md` | W5-2 | 每篇只讲用户可见行为 | 1.2 |
| W5-4 | `AGENTS.md` 路径同步 | 4 处引用改 `docs/dev/` | W5-2 | 引用路径可达 | 0.2 |
| W5-5 | `verify-docs.ps1` 覆盖 `docs/dev/` | 门禁脚本 | W5-1 | 新增目录参与坏链检查 | 0.3 |
| W5-6 | 产品文档技术词扫描 | `tools/verify-docs.ps1` 内的扫描段 | W5-3 | `jarvis_`/`.sqlite3`/`crates/`/`WAL`/`PRAGMA` 零命中 | 0.7 |

**里程碑 E**：`docs/` 下不再出现任何技术性变更内容，`verify-docs.ps1` 全绿。

---

## W6 发布治理与真机验收（1.5 人日）

| # | 任务 | 交付物 | 依赖 | 验收 | 估时 |
| --- | --- | --- | --- | --- | --- |
| W6-1 | 版本号归零 0.0.1 | `Cargo.toml` + installer / scoop / homebrew | 里程碑 E | 构建产物版本为 0.0.1 | 0.2 |
| W6-2 | 删 14 个旧 tag 与全部 GitHub Releases | 远端清理完成 | W6-1 | `git tag` 与 `gh release list` 均为空 | 0.3 |
| W6-3 | orphan 重建提交历史并 force push | 单个初始提交 | W6-2 | `git log --oneline` 只有一条 | 0.3 |
| W6-4 | 打 `v0.0.1` 触发 release workflow | 四平台产物 | W6-3 | tar.gz / zip / msi / pkg / deb 齐全 | 0.3 |
| W6-5 | 39 项真机验收矩阵 | 验收记录 | W6-4 | 基础 7 + 桌面 12 + TUI 7 + CLI 6 + 跨端 7 全部留证据；失败项如实登记 | 0.4 |

**里程碑 F**：`axon --version` 输出 0.0.1，安装态真机验收矩阵通过。

---

## 延后项

本轮不复做、但已登记理由与补做前置的项，见 `docs/storage-architecture.md` §12.5：`rollout.sqlite3` 退役、W3-4 在途/终态拆表、W3-5 统一 claim、W4-2/W4-8 命名重构。它们不阻塞 v0.0.1 的验收矩阵。

## 依赖关系

```text
W0 ──► W1 ──► W2 ──► W3 ──► W4 ──► W5 ──► W6
                       │        │
                       └─ W4-2 可与 W3 并行
                                └─ W5 可与 W4 并行（文档不依赖代码细节定稿）
```

硬约束：

- W2 依赖 W1 的契约测试套件。W1 的测试没定稿就进 W2，后续每次改格式都要重跑全部下游。
- W3 依赖 W2 完成，因为 `work_items` 的 claim 与事件水位在同一批事务里。
- W6 强依赖 W5 与全部真机验收，不能提前。
- 跨端验收（W4-9）必须在安装态二进制上做，不能用 `cargo run` 替代。

## 缓冲与风险

| 阶段 | 风险 | 缓冲 |
| --- | --- | --- |
| W2 | 8 个写入点与 9 个读取点分散在 daemon / session-runtime / cli 三处，改漏一处就是运行时错误 | +2 人日 |
| W3 | `work_items` 合并三套 claim 是本次最容易出正确性问题的地方 | +1.5 人日 |
| W4 | 协议改名会牵动前端，可能反复 | +1 人日 |
| W6 | 历史重置不可逆，任何验证缺口都要在 force push 之前发现 | +0.5 人日 |

含缓冲后总工作量约 **31.5 人日**。

## 交付物清单

| 类型 | 文件 |
| --- | --- |
| 方案 | `docs/dev/storage-architecture.md` |
| 测绘 | `docs/dev/storage-map.md` |
| 执行分解 | `docs/dev/reconstruction-wbs.md`（本文） |
| 新 crate | `crates/axon-eventlog/**` |
| 新库 | `control.sqlite3`、`outbox.sqlite3`、`audit.sqlite3` |
| 新目录 | `data/events/**` |
| 文档 | `docs/**`（产品）与 `docs/dev/**`（技术） |
| 发布 | tag `v0.0.1` 与四平台产物 |
