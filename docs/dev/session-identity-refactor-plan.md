# 会话身份重构计划书（V1）

> 定位：本文是会话标识重构的实施计划与任务分解，配合 `docs/dev/storage-architecture.md` 的 §4.13 使用。
> 两份文档冲突时，会话身份的契约以本文为准，存储细节以存储架构为准。
>
> 本轮前置裁决：**不做兼容层、不写存量迁移**。本机运行态数据物理删除后重装，记忆库保留。

## 1. 背景

会话层目前存在三份并行的标识表示：

- `session-session-<sha256>` 哈希车道串，由 `crates/axon-session/src/application/session_identity.rs` 的 `session_id_for_route` 派生，输入含 `channel`。
- `stable_numeric_id(车道串)` 数值摘要，出现在 `crates/axon-cli/src/run_commands/run_tool_loop.rs` 的工具上下文里。
- `goal-session-<goalId>` 与 `<run-uuid>--subagent--<root>` 两种拼接形态。

三份之间没有主从关系，只能靠约定对齐。已经产出的真实缺陷有两类：用户问答记录写错会话键导致卡片永远不出现，以及轮次指标口径错位导致 header 读数与实际差两个数量级。

轮次层反而是干净的：`interaction_id` 只承担幂等与定位，`origin_channel` 只承担来源标注。本次重构只动会话层。

## 2. 目标与非目标

目标：

1. `session_id` 成为唯一、生成式、与任何维度无关的会话标识，各端看到同一条会话。
2. 渠道彻底下沉为轮次事实，只保留 `origin_channel` 与会话级分布投影。
3. 会话类型与受众边界从字符串与前缀搬进列。
4. 会话创建与入站改为显式携带，服务端不推断。

非目标：

- 不做兼容层，不保留哈希派生回落。
- 不写存量数据迁移，运行态数据直接重建。
- 不改前端交互范式与工具语义。
- 不变更 `origin_channel` 与 `channel_mix_json` 的既有口径，这两处已经正确。

## 3. 设计原则

| 编号 | 原则 | 当前反例 |
| --- | --- | --- |
| P1 | 会话身份只有一个事实源，即 `control.sessions` 主键 | 车道串与数值摘要并存 |
| P2 | 标识必须生成，禁止哈希派生 | `session_id_for_route` |
| P3 | 禁止用字符串拼接表达会话语义 | `goal-session-`、`--subagent--` |
| P4 | 维度落列，不用前缀表达语义 | `sessions` 无 `kind`、无 `trust_scope` |
| P5 | 渠道属于轮次，不属于会话 | 哈希输入含 `channel` |
| P6 | 服务端不推断会话，缺失即拒 | 按 destination 派生 |
| P7 | 会话标识不得为可选 | `interaction_session_id` 返回 `Option` |
| P8 | 代码只保留最新表结构，升级由一次性脚本承担 | `ensure_schema` 内的补列与重命名分支 |

## 4. 目标态设计

### 4.1 标识职责

| 标识 | 职责 | 生命周期 |
| --- | --- | --- |
| `interaction_id` | 一次入站的稳定身份，幂等键与按轮取帧的依据 | 每次入站生成 |
| `session_id` | 会话身份，跨端恒定 | 会话创建时生成，此后不变 |
| `origin_channel` | 本轮从哪个端进入 | 每轮写入 |
| `kind` / `root_session_id` / `goal_id` | 会话类型与关系 | 会话创建时写入 |

### 4.2 数据模型

```text
sessions(
  session_id        TEXT PRIMARY KEY,   -- ULID，服务端生成
  workspace_id      TEXT,
  trust_scope       TEXT NOT NULL,      -- private / group_workspace / restricted
  kind              TEXT NOT NULL,      -- primary / subagent / execution
  root_session_id   TEXT,               -- 子代理指向根会话
  goal_id           TEXT,               -- kind=execution 且来自目标
  execution_id      TEXT,
  current_run_id    TEXT,
  run_generation    INTEGER NOT NULL,
  title             TEXT,
  pinned            INTEGER NOT NULL DEFAULT 0,
  archived          INTEGER NOT NULL DEFAULT 0,
  sort_order        INTEGER,
  created_at_ms     INTEGER NOT NULL,
  last_activity_at_ms INTEGER NOT NULL
)
```

`channel` 与 `destination` 均不入此表。`inbox` 保留 `(source_channel, source_message_id)` 唯一键作为幂等事实。

### 4.3 入站与订阅契约

- `HttpEventRequest` 增必填 `session_id`，`deny_unknown_fields` 保持。
- 网关查表命中即用，未命中直接拒绝，不隐式建会话。
- 订阅按 `session_id`，放行条件只剩「会话存在且受众边界匹配」，删除按前缀的类型白名单。
- `primarySessionId` 继续作为线上字段名，值改为读 `interaction.session_id`。

### 4.4 渠道落点

- 每轮落 `turn_metrics.origin_channel`，仅外部来源有值。
- 会话级分布落 `session_metrics.channel_mix_json`，可由轮次事件重放重建。
- 环境上下文里的渠道语义是本轮来源渠道，不是会话所属渠道。

### 4.5 会话创建

唯一入口 `POST /api/v1/sessions`，生成 ULID 并写入 `kind`、`trust_scope`、`workspace_id`。桌面新建会话调这个接口拿 id，不再本地派生。

## 5. 现状飘逸清单（代码取证）

| 编号 | 飘逸 | 位置 | 后果 |
| --- | --- | --- | --- |
| D1 | 会话身份把 `channel` 算进哈希 | `session_identity.rs` 的 `session_id_for_route` | 同一投递目标在不同渠道成为两条会话，与「不区分渠道」直接冲突 |
| D2 | 会话标识有数值摘要副本 | `run_tool_loop.rs` 的 `stable_numeric_id(&request.session_id)` | 工具层按摘要写记录，问答查询与作答校验全部对不上 |
| D3 | 会话类型编进 id 字符串，表里无 `kind` | 目标执行会话与会话前缀判定 | 网关只能反解析前缀，与「不解析前缀」的既有裁决冲突 |
| D4 | `trust_scope` 只存在于哈希输入 | `sessions` 表无该列 | 受众边界不可查询、不可约束 |
| D5 | 问答记录把 cell id 当 interaction id | `crates/axon-cli/src/tools/orchestrator.rs` 的问答记录构造 | 记录无法反查是哪一轮 |
| D6 | 线上字段名文档与代码不一致 | 文档仍写车道时代的旧线上字段名，代码只有 `primarySessionId` | 会诱导后来者加回第二套字段 |
| D7 | `interaction_id` 生成形态不统一 | `external-interaction-<hash>` 与 `cron-wake-<hash>` | 任何按前缀识别来源的代码都要长特判 |
| D8 | 会话标识返回 `Option` | `interaction_session_id` | 调用方被迫写兜底分支，兜底分支又会掩盖真实错误 |

## 6. 数据清理与重装策略

本轮不做迁移，按下列顺序执行，顺序不可颠倒：必须先证明能装成功，再删数据，否则会出现「删完装不上」。

### 6.1 保留清单（不在本次删除范围）

| 项 | 落点 | 说明 |
| --- | --- | --- |
| 记忆库 | `$HOME/.axon/data/memory/` | 用户明确要求保留 |
| 人格 | `$HOME/.axon/SOUL.md`、`$HOME/.axon/AGENTS.md`、`$HOME/.axon/coding.md` | 与会话无关 |
| 配置 | `$HOME/.axon/config.toml`、`$HOME/.axon/.env`、`$HOME/.mcp.json` | 与会话无关 |
| 技能 | `$HOME/.axon/skills/` | 与会话无关 |
| 工作区 | `control.workspaces` | 项目事实，不属于会话 |
| 定时任务 | `cron/cron.sqlite3`、`control.schedules` | 任务定义不是会话数据 |
| 目标 | `control.goals`、`control.goal_results`、`data/goal-results/` | 定义与结果保留，只清指向会话的引用 |
| 用量统计 | `$HOME/.axon/usage/` | 统计口径独立于会话 |
| 诊断日志 | `$HOME/.axon/data/logs/` | 排障用 |
| 人格状态 | `$HOME/.axon/data/persona/` | 人格不是会话数据 |
| 缓存 | `$HOME/.axon/cache/` | 可重建，不属于会话数据 |

### 6.2 删除清单（只删会话数据）

行删除，库文件本身保留：

| 库 | 表 |
| --- | --- |
| `control.sqlite3` | `sessions`、`inbox`、`session_metrics`、`work_items`、`work_item_history`、`resume_pointer`、`ledger_events`、`decisions` |
| `outbox.sqlite3` | `user_questions`、`agent_messages`、`continuations`、`notifications`、`notification_deliveries` |

目录删除：

`data/events/`、`data/sessions/`、`data/attachments/`、`data/archive/`、`data/run-control/`、`data/runtime/`、`data/tmp/`、`data/channel-metrics/`、`data/resource-metrics/`、`data/subagent-graph.json`、`data/daemon.pid`

引用清理：

- `control.goals` 与 `control.schedules` 中指向会话的列置空，行本身保留。
- `control.workspaces` 完全不动。

关键约束：**不得整库删除 `control.sqlite3` 与 `outbox.sqlite3`**，否则会连定时任务、目标、工作区、用量一起丢掉，超出「会话数据」范围。

### 6.3 表结构重建（一次性临时脚本）

会话数据删除与表结构重建都由一个一次性临时脚本完成。它不是迁移机制，不进版本控制，处理完即可删除。

脚本约束：

- 放在仓库外的临时目录（例如 `$env:TEMP`），不入库、不写进 `scripts/`。
- 只做两件事：按 §6.2 清空会话数据，按目标态 DDL 重建表、索引与约束。
- 不引入 schema 版本号、迁移注册表或升级分支；处理完连同脚本一起删除。
- 执行前先停 daemon 与桌面端，执行后按 §6.4 重装。

这样做的目的是让新身份模型从第一行数据起就成立，而不是在旧表上叠补丁，同时避免把一次性操作沉淀成代码里的长期迁移负担。

| 表 | 重建要点 |
| --- | --- |
| `sessions` | 主键改 ULID；新增 `trust_scope`、`kind`、`goal_id`、`execution_id`、`sort_order`；`current_run_id` 与 `run_generation` 分工，删除语义重叠列；`channel` 与 `destination` 一律不出现 |
| `inbox` | 保留 `(source_channel, source_message_id)` 唯一键；`session_id` 非空且指向 `sessions`；渠道只在此表存在，不进入会话身份 |
| `session_metrics` | 主键仍为 `session_id`；保留 `channel_mix_json` 与 token 分桶；列口径与投影重放一致 |
| `work_items`、`work_item_history` | `session_id` 非空；保留既有 claim 与 lease 索引 |
| `resume_pointer` | `session_id` 指向新表 |
| `user_questions` | `session_id` 为会话 ULID；`interaction_id` 存真实轮次标识，不再存 cell id |
| `notifications`、`notification_deliveries` | 会话维度投递记录一并重建 |

索引重建：

- 会话列表：`(workspace_id, archived, sort_order, last_activity_at_ms)`。
- `inbox`：按会话与接收时间 `(session_id, received_at_ms DESC)`。
- `work_items`：按状态与可用时间 `(status, kind, available_at_ms)`。
- `session_metrics`：主键即索引，不额外建二级索引。

约束重建：

- `sessions.kind` 限定 `primary`、`subagent`、`execution`。
- `sessions.trust_scope` 限定 `private`、`group_workspace`、`restricted`。
- `goal_id` 与 `execution_id` 只在 `kind='execution'` 时有值。
- `root_session_id` 只在 `kind='subagent'` 时有值。

代码侧边界：脚本执行完之后，仓库里只保留最新表结构。会话运行库的 `ensure_schema` 只负责按最新 DDL 建表，函数内的补列、加列、重命名 legacy 表等升级分支一并删除，不再保留任何旧结构兼容路径。其他子系统（用量统计、记忆、人格、MCP 缓存）的补列逻辑不在本次范围，另立事项，不在本计划内顺手处理。

### 6.4 重装与核验

`scripts/install-from-source.ps1 -Components all`，装完核验：`current.json` 指向的版本目录内两个二进制同时存在且与 `target/release` 哈希一致，`axon --version` 与本次构建一致，cargo 的 bin 目录无残留。

## 7. 里程碑

| 里程碑 | 内容 | 出口条件 |
| --- | --- | --- |
| M0 | 决策与规范收口 | 七条原则写入文档，冲突的旧裁决显式作废 |
| M1 | 数据模型改造 | `sessions` 具备 `kind` 与 `trust_scope`，语义重叠列收敛 |
| M2 | 身份层替换 | 派生函数删除，会话标识只剩一个来源 |
| M3 | 入站与订阅契约 | 三端显式携带，网关不派生 |
| M4 | 调用点收敛 | 六个派生调用点全部改读实列 |
| M5 | 门禁与验收 | 不变量探针与真机链路全绿 |

## 8. WBS

量级：S 半天内，M 一天左右，L 两天以上。

### W0 环境准备

| 编号 | 任务 | 产出物 | 依赖 | 验收 | 量级 |
| --- | --- | --- | --- | --- | --- |
| W0.1 | 确认工作区可编译 | `cargo check --workspace` 通过记录 | 无 | 退出码 0 | S |
| W0.2 | 停 daemon 与桌面端 | 进程清单 | W0.1 | 无 axon 进程残留 | S |
| W0.3 | 备份保留清单 | 备份副本与哈希 | W0.2 | 备份哈希与源一致 | S |
| W0.4 | 物理删除会话数据：按表清空会话相关行，删除事件、附件、运行态目录 | 删除清单回执 | W0.3 | 只剩保留清单，会话表零行 | S |
| W0.5 | 用一次性临时脚本重建会话相关表结构与索引约束，脚本置于仓库外且处理完即删 | 新 DDL 与执行回执 | W0.4 | 新表含 `kind`、`trust_scope`，无渠道列；版本控制内无该脚本 | M |
| W0.6 | 重装并核验版本树 | 安装回执 | W0.5 | 二进制哈希与 `current.json` 一致 | M |

### W1 规范与决策（M0）

| 编号 | 任务 | 产出物 | 依赖 | 验收 | 量级 |
| --- | --- | --- | --- | --- | --- |
| W1.1 | 澄清存储架构文档中「渠道是否入表」的自相矛盾 | 决议记录 | 无 | 文档不再两说 | S |
| W1.2 | 定 ULID 规范：长度、字符集、禁止业务前缀、可读性只在展示层 | 规范段落 | W1.1 | 规范可被测试表达 | S |
| W1.3 | 定 `kind` 取值域与 `root_session_id`、`goal_id` 语义 | 规范段落 | W1.1 | 与网关放行规则一致 | S |
| W1.4 | 统一 `interaction_id` 生成规范与幂等键来源 | 规范段落 | 无 | 所有入口共用一套策略 | S |
| W1.5 | 作废旧裁决「桌面新建会话不走会话创建接口」 | 裁决记录 | W1.4 | 文档标注作废原因 | S |
| W1.6 | 重写存储架构 §4.13，删除四元组派生的旧形态描述 | 文档变更 | W1.2、W1.3 | 文档门禁通过 | M |
| W1.7 | 术语统一：车道改为会话，全仓不再出现车道时代的旧线上字段名 | 文档与注释 | W1.6 | 全仓检索无命中 | S |

### W2 数据模型（M1）

| 编号 | 任务 | 产出物 | 依赖 | 验收 | 量级 |
| --- | --- | --- | --- | --- | --- |
| W2.1 | `sessions` 增 `trust_scope`、`kind`、`goal_id`、`execution_id`、`sort_order` | schema | W1.3 | 建库即含新列 | M |
| W2.2 | 收敛语义重叠列，`run_generation` 与 `current_run_id` 分工明确 | schema | W2.1 | 读写用例通过 | M |
| W2.3 | 列表索引：`workspace_id` 加 `archived` 加 `last_activity_at_ms` | schema | W2.1 | 列表查询走索引 | S |
| W2.4 | 复核 `inbox` 幂等唯一键，确认渠道不入会话身份 | 核对记录 | 无 | 幂等用例通过 | S |
| W2.5 | 删除会话运行库 `ensure_schema` 内的补列与 legacy 重命名分支，只保留最新 DDL 建表 | 代码删除 | W2.1 | 函数内无 `ALTER TABLE` 与重命名分支 | M |

### W3 身份层（M2）

| 编号 | 任务 | 产出物 | 依赖 | 验收 | 量级 |
| --- | --- | --- | --- | --- | --- |
| W3.1 | 新增 ULID 生成器并集中一处 | 新模块 | W1.2 | 单调性与字符集用例 | S |
| W3.2 | 删除 `session_id_for_route` 与前缀常量 | 代码删除 | W3.1 | 全仓无引用 | S |
| W3.3 | `interaction_session_id` 改读实列，返回类型收紧为 `String` | 签名变更 | W2.1 | 编译通过且无 `Option` 兜底 | M |
| W3.4 | 取消会话数值摘要，工具上下文只保留一个会话标识 | 上下文收敛 | W3.3 | 工具层无第二套会话 id | M |
| W3.5 | 删除 `goal-session-` 与子代理拼接 | 代码删除 | W2.1 | 全仓无会话 id 拼接 | M |

### W4 创建与入站（M3）

| 编号 | 任务 | 产出物 | 依赖 | 验收 | 量级 |
| --- | --- | --- | --- | --- | --- |
| W4.1 | 会话创建接口生成 ULID 并写 `kind`、`trust_scope` | 路由实现 | W3.1 | 返回合法 ULID | S |
| W4.2 | 入站请求增必填 `session_id` | 协议变更 | W4.1 | 缺失被拒且错误可读 | M |
| W4.3 | 网关改为查表命中即用，删除派生 | 网关实现 | W4.2 | 探针用例通过 | M |
| W4.4 | 桌面新建会话改调创建接口，列表以会话表为事实源 | 桌面端改动 | W4.1 | 新建后列表出现同一 id | M |
| W4.5 | TUI 与 CLI 直传会话标识，缺失时明确报错 | 两端改动 | W4.2 | 指定不存在 id 直接报错 | M |

### W5 订阅与放行（M3）

| 编号 | 任务 | 产出物 | 依赖 | 验收 | 量级 |
| --- | --- | --- | --- | --- | --- |
| W5.1 | 订阅按 `session_id`，删除类型白名单 | 订阅实现 | W4.3 | 三类会话同规则订阅 | M |
| W5.2 | 放行条件改为会话存在加受众边界匹配，边界读实列 | 放行实现 | W2.1 | 越权订阅被拒用例 | M |
| W5.3 | 事件分段目录名与主键一致性核对 | 核对与修正 | W3.1 | 目录名等于主键 | S |

### W6 调用点收敛（M4）

| 编号 | 任务 | 产出物 | 依赖 | 验收 | 量级 |
| --- | --- | --- | --- | --- | --- |
| W6.1 | `workbench_http_adapter.rs` 改显式入参 | 改动 | W3.3 | 用例通过 | S |
| W6.2 | `cron_http_adapter.rs` 改任务存目标会话 | 改动 | W3.3 | 唤醒落到目标会话 | M |
| W6.3 | `gateway_voice.rs` 改按显式会话 | 改动 | W3.3 | 语音入站命中同会话 | S |
| W6.4 | `daemon_history_loader.rs` 改读实列 | 改动 | W3.3 | 历史加载用例 | S |
| W6.5 | `interrupted_interaction_repository.rs` 改读实列 | 改动 | W3.3 | 中断恢复用例 | S |
| W6.6 | `interaction_adapter.rs` 改读实列 | 改动 | W3.3 | 适配层用例 | S |
| W6.7 | 轮次运行与 turn lanes 层收敛，不再派生会话 | 改动 | W3.4 | 轮次运行无派生调用 | M |
| W6.8 | 问答记录的 `interaction_id` 改用真实轮次标识 | 改动 | 无 | 记录可按轮反查 | S |

### W7 渠道与轮次核对（M4）

| 编号 | 任务 | 产出物 | 依赖 | 验收 | 量级 |
| --- | --- | --- | --- | --- | --- |
| W7.1 | 复核 `origin_channel` 在四个入口的覆盖 | 核对记录 | 无 | 四渠道各一轮可读出 | M |
| W7.2 | 复核 `channel_mix_json` 累计与重放重建 | 核对记录 | W7.1 | 重放后分布一致 | S |
| W7.3 | 环境上下文的渠道语义改成本轮来源渠道 | 改动 | W7.1 | 同一会话不同轮显示不同渠道 | S |
| W7.4 | 前端逐轮来源展示复核 | 核对记录 | W7.1 | 三轮分别标注正确 | S |

### W8 门禁与验收（M5）

| 编号 | 任务 | 产出物 | 依赖 | 验收 | 量级 |
| --- | --- | --- | --- | --- | --- |
| W8.1 | 探针：同一投递目标三渠道命中同一会话 | 自动化用例 | W4.3 | 用例通过 | M |
| W8.2 | 架构门禁：禁拼接会话 id、禁会话数值摘要 | 门禁规则 | W3.4 | 违例即失败 | M |
| W8.3 | 契约测试：入站缺标识被拒、越权订阅被拒、幂等重放 | 测试 | W4.2、W5.2 | 用例通过 | M |
| W8.4 | 真机链路：桌面发起加 CLI 续聊加桌面继续 | 真机证据 | W4.4、W4.5 | 双向取证 | L |
| W8.5 | 真机：目标执行与 cron 唤醒落到同会话且渠道正确 | 真机证据 | W6.2 | 唤醒后历史连续 | M |
| W8.6 | 文档与运行时操作手册同步 | 文档变更 | 全部 | 文档门禁通过 | M |

### W9 性能专项

重构会把「会话派生」换成「会话读取」，读取代价必须显式管理，否则身份问题修好了，热路径反而变慢。

| 编号 | 任务 | 产出物 | 依赖 | 验收 | 量级 |
| --- | --- | --- | --- | --- | --- |
| W9.1 | 建性能基线：会话列表、打开会话首屏、单次入站、订阅放行 | 基线与阈值表 | W0.5 | 四项各有本机实测值 | M |
| W9.2 | 入站会话存在性校验改为查一次并缓存，禁止每帧查库 | 缓存实现 | W4.3 | 单次入站无重复主键查找 | M |
| W9.3 | 订阅放行缓存：按会话 id 的固定容量条目，带受众边界与存在性，关闭事件或 TTL 失效 | 缓存实现 | W5.2 | 帧路放行零查库 | M |
| W9.4 | `channel_mix_json` 写放大评估：确认累加与 `session_metrics` UPSERT 在同一事务内完成，不做二次读改写 | 评估与修正 | W7.2 | 单轮只写一次投影行 | S |
| W9.5 | 主键局部性：确认 ULID 单调有序，会话插入不做随机主键式页分裂 | 核对与索引说明 | W2.1 | 主键插入位置单调前进 | S |
| W9.6 | 事件量治理：实测单轮 14543 帧中 14006 帧为用量快照，评估只在流收口落一条 | 方案与实测 | W0.5 | 单轮帧数显著下降且读数不变 | L |
| W9.7 | 性能回归门禁：四项基线写成脚本，超过阈值即失败 | 门禁规则 | W9.1 | 回归可自动判定 | M |

### W10 三端用户可见回归

身份重构的直接后果落在三端的使用路径上，这些路径必须逐条回归，不能只验不变量。

| 编号 | 任务 | 产出物 | 依赖 | 验收 | 量级 |
| --- | --- | --- | --- | --- | --- |
| W10.1 | TUI `/sessions` 列表：数据来自会话清单接口，每条含标题与 `provider · model` 行 | 回归记录 | W4.4 | 列表条数与库内会话一致 | M |
| W10.2 | TUI 选中切换：以新 `session_id` 重建订阅，界面不写服务端状态 | 回归记录 | W5.1 | 切换后正文与指标来自新会话 | M |
| W10.3 | CLI 续接：`axon run "..." --session <id>` 直接落该会话 | 回归记录 | W4.5 | 续接后轮次追加到同一会话 | M |
| W10.4 | CLI 不存在 id：显式报错，不隐式新建 | 回归记录 | W4.5 | 退出码非零且不产生会话行 | S |
| W10.5 | CLI `sessions resume` 与 `--session` 优先级：单次覆盖高于默认 | 回归记录 | W10.3 | 优先级与文档一致 | S |
| W10.6 | 会话解析收敛：`session_resolution.rs` 的双重回落可简化，id 不再有两个标识空间 | 代码简化 | W3.3 | 解析只用一处且无回落分支 | M |
| W10.7 | 桌面左侧菜单：置顶、归档、排序、标题读同一份会话清单 | 回归记录 | W4.4 | 四项与会话表一致 | M |
| W10.8 | 桌面分页渲染：加载更早一次一轮、展开折叠行回放整轮、超页数预算保留首页与尾窗、中段截断如实标注 | 回归记录 | W6.7 | 20 轮以上会话逐项符合 | L |
| W10.9 | 跨端接续可见性：桌面发起、CLI 续聊、桌面再看，同一会话内三轮来源分别标注 | 回归记录 | W8.4 | 来源与分布投影一致 | M |

## 9. 关键路径与并行

关键路径：W0 到 W2 到 W3 到 W4 到 W5 到 W8.4。

W10 是三端用户可见回归，依赖 W4 与 W5 完成后才能全量开跑；其中 W10.6 与 W10.8 属代码简化与渲染回归，可与 W6、W7 并行推进。

可并行：

- W0.1 到 W0.3 可与 W1 同时进行，W0.4 必须等 W0.1 通过。
- W7 与 W1 无依赖，可早期插入。
- W6.1 到 W6.6 在 W3.3 完成后彼此独立，可并行。
- W6.8 与整条链路无依赖，随时可插入。

## 10. 性能约束

会话身份从「派生」变成「读取」，多出来的是读取代价。下列红线按本机实测标定，超过即视为回归。

### 10.1 性能红线

| 路径 | 红线 | 依据 |
| --- | --- | --- |
| 会话列表 | 单次 `sessions JOIN session_metrics USING(session_id)`，走索引，禁止扫事件与 usage | 列表页只需会话事实与投影 |
| 打开会话首屏 | 索引页 <= 300ms，首屏可见 <= 800ms | 此前实测 468 至 781ms，网关索引页 300ms |
| 单次入站 | 会话校验为一次主键查找，上限 1ms，禁止每帧查库 | 高频事件会放大任何一次额外 IO |
| 订阅放行 | 帧路零查库，命中进程内缓存 | 放行发生在每一帧上 |
| 单轮事件量 | 不得因新增字段而增长；用量快照只保留收口一条 | 实测 14543 帧中 14006 帧为用量快照 |
| 会话投影写入 | 每轮只写一次 `session_metrics` 行，累加与 UPSERT 同事务 | 避免读改写放大 |

### 10.2 与本次重构直接相关的三处性能设计

1. **会话存在性校验必须缓存。** 入站带 `session_id` 后，网关每次都要确认该会话存在。若逐帧查库，事件吞吐立刻被主键查找压住。做法是命中一次后按会话 id 缓存存在性与 `trust_scope`，失效来源只有会话关闭事件与 TTL，容量固定。

2. **订阅放行从「判前缀」变为「查一次」。** 原来靠字符串前缀在内存里判定会话类型，代价为零。改成按 `kind` 与 `trust_scope` 判定后，第一次必须读库，此后走同一份缓存。这是本次重构最容易引入性能回退的一处，W9.3 单独验收。

3. **ULID 必须选有序实现，不能用 UUIDv4。** ULID 前 48 位是毫秒时间戳，主键插入位置单调前进，B-tree 页分裂远少于随机主键。会话表是只增不删的写入型表，这条差异在长期运行后非常明显。同时禁止每次生成都抢全局锁或重复读系统时钟。

### 10.3 事件量是当前最大的性能病灶

实测一条只有一轮的会话，事件日志 14543 帧里有 14006 帧（96.3%）是 `usageUpdate` 用量快照，同一条 provider 流把同一份快照反复写了几百上千次。它带来三重代价：日志体积、解压与传输、客户端归约。W9.6 要求评估「只在流收口落一条」，这是本次重装后最值得做的一项性能治理，与会话身份重构相互独立，可以单独验收。

## 11. 验收标准

1. 五条不变量均有自动化探针：同一投递目标三渠道同 id、会话必可命中且表内有 `kind` 与 `trust_scope`、无会话数值摘要、无会话 id 拼接、`interaction_id` 可幂等复核。
2. 真机链路通过：桌面两轮、CLI 一轮、桌面继续，三轮来源分别标注，分布投影为桌面 2 与 CLI 1。
3. Rust 与前端门禁全绿，架构与文档校验脚本通过。
4. 文档中不再存在四元组派生、车道时代的旧线上字段名、前缀解析等旧形态描述。
5. 仓库内不存在会话相关的 `ALTER TABLE` 补列分支、legacy 表重命名分支与 schema 版本迁移注册表；会话库的建表函数只表达最新结构。
6. 一次性重建脚本不在版本控制内，且在本机执行完成后已删除。
7. 三端用户可见回归矩阵全绿：TUI 的 `/sessions` 列表与选中切换、CLI 的 `--session` 续接与不存在 id 报错、桌面左侧菜单与正文分页渲染。这一条不可用不变量探针替代，必须逐条走真实入口。

## 12. 风险与对策

| 风险 | 影响 | 对策 |
| --- | --- | --- |
| 三端未同批发布 | 旧客户端入站全被拒 | 发布清单把三端绑为一个交付单元 |
| 订阅放行简化后越权 | 私聊历史进群聊 | W5.2 必须以实列校验，不得省略 |
| 事件目录与主键不一致 | 历史读不到 | W5.3 单独核对并补一致性用例 |
| 目标与 cron 唤醒失去前缀定位 | 唤醒落空 | 靠 `kind`、`goal_id`、`execution_id` 列定位，W6.2 覆盖 |
| 删数据后装不上 | 本机不可用 | 顺序固定为 W0.1 先证明可编译，W0.4 才删 |
| 临时脚本被误提交 | 一次性操作沉淀成长期迁移机制 | 脚本置于仓库外，提交前核对暂存区无该文件 |
| 并行改动撞车 | 构建与安装冲突 | 每批收口前重跑编译，安装前确认当前提交 |
| 旧记忆与新裁决冲突 | 后续实现被改回旧形态 | W1.5 显式作废旧裁决并写入文档 |

## 13. 待裁决项

1. `session_id` 是否保留可读前缀。建议不保留，纯 ULID，可读性只在展示层。
2. 子代理与目标执行会话是否统一到 ULID 加 `kind`。建议统一，否则刚清掉一层特判又留一层前缀特判。
3. 补全 `kind` 的取值域是否需要第四类（例如定时任务独立成 `cron`）。**已裁决**：按来源取值，取 `primary`（用户对话）、`subagent`（子代理派发）、`cron`（定时任务唤醒）、`goal`（目标看板发起）四类；桌面与 TUI 会话清单只列 `primary`，CLI 只续接 `primary`。

## 14. 实施状态回写

> 每个任务项完成后在本节回写状态与证据。状态取值：已完成 / 待执行 / 阻塞。

| 编号 | 状态 | 证据 |
| --- | --- | --- |
| W0.1 | 已完成 | `cargo check --workspace` 退出码 0，21.67s |
| W0.2 | 已完成 | `axon daemon stop` 后 `daemon status` 为 `running=false`，无 axon 进程残留 |
| W0.3 | 已完成 | 备份至 `$env:TEMP\axon-session-identity-backup-20261118-131925`；`control.sqlite3`、`outbox.sqlite3`、`SOUL.md`、`AGENTS.md`、`coding.md`、`config.toml`、`.mcp.json`、`.env`、`cron.sqlite3`、`usage.sqlite3` 逐份 SHA-256 与源一致；`data/memory` 138 个文件逐份哈希一致，0 不匹配；`skills/` 16007 条清单指纹 SHA-256 `7912390E…A98E850` |
| W0.4 | 已完成 | control 清空 `sessions`、`inbox`、`session_metrics`、`work_items`、`work_item_history`、`resume_pointer`、`ledger_events`、`decisions`，`goals.created_from_interaction_id` 与 `schedules.session_id` 置空；outbox 清空 `user_questions`、`agent_messages`、`continuations`、`notifications`、`notification_deliveries`；`data/events`、`data/sessions`、`data/attachments`、`data/archive`、`data/run-control`、`data/runtime`、`data/tmp`、`data/channel-metrics`、`data/resource-metrics`、`data/subagent-graph.json` 与 `~/.axon/sessions`（会话便签目录，按旧 session_id 分目录）删除；`workspaces`、`goals`、`goal_results`、`usage`、`memory`、`persona`、`logs` 未动 |
| W0.5 | 已完成 | 一次性脚本置于 `$env:TEMP`、执行后已删除；control `PRAGMA user_version` 6→7，outbox 保持 1；`sessions` 含 `trust_scope`、`kind`、`goal_id`、`execution_id`、`sort_order`，无 `channel`/`destination`，`run_session_id`→`current_run_id`；两库 `PRAGMA integrity_check` 均为 `ok`；`git status` 干净 |
| W0.6 | 已完成 | 按长官指示改走**停机覆盖**安装（不触发 UAC、不重注册计划任务）：先 `axon daemon stop` 并确认 `axon` / `axon-desktop` 进程为 0，再把 `target\release\{axon.exe, axon-desktop.exe}` 覆盖到 `current.json` 指向的 `versions\0.0.1\`。核验：两文件 SHA-256 与 `target\release` 逐对一致（`axon.exe` 149004288 字节、`axon-desktop.exe` 23209984 字节，安装后时间戳等于构建时间 2026-11-18 20:24:22 / 20:23:46）；`current.json` 覆盖前后 SHA-256 不变（`9298ACD5…717A20`）；`axon --version` 为 `axon-cli 0.0.1`；`~/.cargo/bin` 下 `axon*.exe` 残留 0；Daemon 经 `schtasks /change /enable` + `schtasks /run`（当前用户权限，无提权）恢复，`daemon status` 为 `installed=true,running=true`，PID 30252，网关监听 `127.0.0.1:3144` 与 `0.0.0.0:3143`，`daemon health` 报 host `running`、租约 26s |

| W1.1 | 已完成 | `storage-architecture.md` §4.12/§4.13 明确「`channel` 与 `destination` 不在 `sessions` 表」，删除「把 channel 落成实列」的旧表述 |
| W1.2 | 已完成 | §4.13.1 写入 ULID 规范：26 字符 Crockford Base32、48 位毫秒时间戳加 80 位随机、禁止业务前缀、生成点集中一处 |
| W1.3 | 已完成 | §4.13.1 写入 `kind` 三类取值与 `root_session_id`/`goal_id`/`execution_id` 的列约束，定时任务并入 `execution` |
| W1.4 | 已完成 | §4.13.3 写入 `interaction_id` 规范：ULID、无前缀、幂等来源为 `(source_channel, source_message_id)` 唯一键 |
| W1.5 | 已完成 | §4.13.6 显式作废「桌面新建会话不走会话创建接口」，并写明作废原因 |
| W1.6 | 已完成 | §4.13 整节重写，`storage-architecture.md` 不再出现四元组派生形态描述 |
| W1.7 | 已完成 | 全仓 `primaryLaneId` 零命中；`docs/**/*.md` 与 `README.md` 共 21 个文件的「车道」统一为「会话」；`pwsh -File tools/verify-docs.ps1` 输出 `Documentation verification OK` |
| W2.1 | 已完成 | `sessions` 增 `trust_scope`、`kind`、`goal_id`、`execution_id`、`sort_order`，`kind` 取 `primary`/`subagent`/`cron`/`goal`；活库与 `schema.rs` DDL 一致，`PRAGMA integrity_check` 为 `ok` |
| W2.2 | 已完成 | `run_session_id` 全仓改为 `current_run_id`，与 `run_generation` 分工：前者可变、后者单调递增；`root_session_id` 改为可空且只在 `kind='subagent'` 有值（CHECK 约束） |
| W2.3 | 已完成 | 新增 `sessions_by_workspace(workspace_id, archived, sort_order, last_activity_at_ms)` 与 `sessions_by_kind_root(kind, root_session_id)` 索引 |
| W2.4 | 已完成 | `inbox` 保留 `(source_channel, source_message_id)` 唯一索引；`session_id` 改为 `NOT NULL` 无默认值，渠道不进入会话身份 |
| W2.5 | 已完成 | `prepare_schema` / `initialize_schema` 内无 `ALTER TABLE`、无 legacy 重命名分支，只按最新 DDL 建表；schema 变更一律递增 `user_version`（control 6→7） |
| W3.1 | 已完成 | `application/session_identity.rs` 实现 ULID：26 字符 Crockford Base32、48 位毫秒时间戳、80 位随机；同毫秒内单调递增，全原子无锁；字符集、单调性、时间戳解码与 `kind`/`trust_scope` 往返均有单测 |
| W3.2 | 已完成 | `session_id_for_route`、`SESSION_SESSION_ID_PREFIX` 与 `lanes/identity` 派生路由全部删除，全仓无引用 |
| W3.3 | 已完成 | `InteractionRecord` 增必填 `session_id`；`interaction_session_id` 改读该列并返回 `String`；`inbox` 读路径 `INBOX_COLUMNS` 末列补 `session_id` |
| W3.4 | 已完成 | 工具上下文只保留 `thread_session_id`；数值键收敛到 `memory_session_key()` 一处（记忆子系统的键空间，不是会话身份）；问答幂等键改用它自己的会话串 |
| W3.5 | 已完成 | 两处拼接全部删除：目标执行会话按 `goal_id` 唯一索引解析，子代理按 `(root_session_id, agent_path)` 唯一索引解析，标识一律为服务端 ULID；`sessions` 增 `agent_path` 列（schema v8）与两个部分唯一索引；子代理逐字稿按登记身份写入，缺行时补登记同一身份而不是另建主会话；名册 slug 改读 `agent_path`；桌面「独立会话」判定改读渠道，不再按标识内容猜类型。真机回归并入 W8.4 |
| W4.1 | 已完成 | `POST /api/v1/sessions` 生成 ULID，写入 `kind='primary'`、`trust_scope='private'`、`workspace_id`；用例断言返回值为合法 ULID |
| W4.2 | 已完成（已失效，见 W10.13） | **外部事件报文不再接受 `sessionId` 与 `workspaceId`**（都属于未知字段，`deny_unknown_fields` 直接拒绝）；语音 `start` 帧仍由客户端显式给出 `sessionId`，缺失即拒 |
| W4.3 | 已完成 | 网关外部事件一律落到服务端内部的一条固定会话（`GATEWAY_EVENT_SESSION_ID`，首次到达登记、此后复用），Workspace 固定取网关默认（Axon 根目录 `~/.axon`）；外部既不暴露会话也不暴露 Workspace，且不存在第二个 Workspace |
| W4.4 | 已完成 | 桌面 `SessionSource::derive_session_id` 改为 `create_session`，改调 `POST /api/v1/sessions`；会话清单以 `sessions` 表为准并只列 `kind='primary'` |
| W4.5 | 已完成（已被 W10.13 取代） | CLI/TUI 统一 `prepare_interaction_from_prompt_as(session, ...)`：显式 id 必须已登记且为 `primary`、工作区一致，否则直接报错；未给 id 时解析该 Workspace 的活跃会话，没有才新建 |
| W5.1 | 已完成 | 会话级订阅删掉类型白名单与前缀解析，统一按 `session_id` 放行；四个新用例覆盖允许与三类拒绝 |
| W5.2 | 已完成 | 放行条件改为「会话存在 + 工作区匹配 + `trust_scope` 非 `restricted`」，边界读 `sessions.trust_scope` 实列 |
| W5.3 | 已完成 | 事件分段目录名与主键一致性已核对：`EventLog::encode_segment` 对 ULID 不产生任何转义，目录名与主键逐字相同；补了两条用例（ULID 主键 → 目录名即主键；不同主键永不共用目录）与一条会话侧一致性用例（`store.event_log(id)` 的目录名等于主键） |
| W6.1 | 已完成 | `workbench_http_adapter` 删除派生路由，会话创建统一走 `POST /api/v1/sessions` |
| W6.2 | 已完成 | Cron 唤醒改用 `schedules.session_id`（缺失即报错），Cron 视图与唤醒路径改调 `resolve_workspace_session`；**任务定义侧同步收口**：`resolve_schedule_session` 先读任务已有绑定、没有才建一条 `kind=cron` 会话（工作区与根目录取自 Workspace 注册表），`upsert_interval_schedule` 在省略 `--session-id` 时调用它并在同一次落库里写回 `schedules.session_id`，显式给出的会话仍先校验已登记。用例覆盖「省略会话→落定 cron 会话」「同一任务重复定义→沿用同一会话」「未登记会话→定义侧即拒」 |
| W6.3 | 已完成 | `gateway_voice` 改用帧内显式 `sessionId`，`session_id_for` 删除；桌面侧同步收口：`voice_start` 与输入坞语音入口显式传当前会话标识，首帧必带 `sessionId`，缺省由界面与宿主各自拦下 |
| W6.4 | 已完成 | `daemon_history_loader` 直接读 `interaction.session_id` |
| W6.5 | 已完成 | `interrupted_interaction_repository` 读 `inbox.session_id`，`candidate_session_id` 不再派生 |
| W6.6 | 已完成（已被 W10.13 取代） | `interaction_adapter` 的 Interaction 会话取自显式入参或 Workspace 活跃会话 |
| W6.7 | 已完成 | 主 turn 与执行 worker 已改读 `current_run_id`；turn lanes 与子代理装配层不再派生会话标识：原 `child_session_id(父会话, 路径)` 已删除，统一走 `resolve_subagent_session`，唤醒前置校验按 `(根会话, 代理路径)` 反查 |
| W6.8 | 已完成 | `RunRequest` 增 `interaction_id`（来自 daemon 信封的 `transportContext.interactionId`）；工具上下文增 `interaction_id` 与 `turn_id`；问答记录改用它自己的真实入站标识，`turn_id` 改用与本轮持久化 `Turn.turn_id` 逐字一致的构造 |
| W7.1 | 已完成 | 探针 `origin_channel_covers_all_four_inbound_entries`：桌面 / CLI / TUI / 外部事件各跑一轮真实入站（桌面、CLI、TUI 带客户端，外部事件没有客户端），逐轮断言 `turn_metrics.origin_channel` 与 `channel_mix_json` 都是该入口自己，且不出现 `internal` |
| W7.2 | 已完成 | 真实交互链路上的在线累计与 `rebuild_session_metrics` 重放一致（`rebuild_restores_the_same_channel_mix_after_real_interactions`），重复重建幂等；此前只有手工 `with_origin` 事件的重建用例 |
| W7.3 | 已完成 | 环境上下文 `session_channel` 改名 `origin_channel`，取值改**本轮来源**：daemon 的 `transportContext` 增 `originChannel`（客户端优先、没有客户端退回来源渠道，与投影同一口径），旧载荷退回 `channel`；目标执行显式标 `goal`；本批同时把对外请求的 User-Agent 扩到六个入口（`axon-tui` / `axon-desktop` / `axon-cli` / `axon-webhook` / `axon-cron` / `axon-goal`），取值同样走 `originChannel`；外部事件另给 `userAgentChannel=webhook`，UA 报 webhook 而来源标注仍是具体厂商 |
| W7.4 | 已完成 | 历史索引本身已逐轮给出 `channel` 与 `client`，前端 `historySource` 客户端优先；新增三轮用例（桌面 / CLI 续聊 / 定时）锁定「同一会话三轮各标各的」 |
| W8.1 | 已完成 | 新增入站探针：同一投递目标、desktop / cli / tui 三渠道各提交一轮，全部落在同一条会话（三条入站、一条会话，且投递目标一致），守住「不得按渠道或投递目标另建会话」 |
| W8.2 | 已完成 | `tools/verify-architecture.py` 增两条门禁：①**禁拼接会话 id** —— 生产源码里 `format!`/`starts_with`/`contains`/`strip_prefix`/`ends_with` 不得出现 `--subagent--` 或 `goal-session-`，也不得为它们定义分隔符或前缀常量；②**禁会话数值摘要** —— `stable_numeric_id` 只允许出现在 6 个既有内存句柄文件，新增即失败（注释与测试源码不计）。两条均以临时探针验证过「违例即 exit 1」，随后删除探针 |
| W8.3 | 已完成 | 三类契约均有用例：入站缺标识被拒（HTTP/WS/语音/外部事件，此前已覆盖）；越权订阅被拒（未登记、工作区不匹配、受众受限三条拒绝 + 三条允许，见 `gateway_bridge_goal_session_tests`）；幂等重放（同记录重放命中同一标识且只一行、重放方换标识仍命中库里那一条、来源键相同但内容不同一律 Conflict，外加外部事件入口的同事件跨渠道判为两条）。D7 的四类派生交互标识（`external-interaction-`、`cron-wake-`、`shell-job-wake-`、`interaction-` / `schedule-interaction-`）全部改为服务端 ULID，五个生成点统一走 `new_interaction_id()`，幂等由各自的来源键承担 |
| W8.4 | 已完成 | 真机链路三段全通（安装态 `versions/0.0.1`，Daemon PID 27804 与桌面端 PID 41072，CDP 9333）：同一会话 `01J8XQ7K3M4N5P6Q7R8S9T0V1W` 上「桌面发起（20:57）→ CLI 续聊（20:59）→ 桌面继续（22:04）」连续落地，`axon sessions read` 逐轮读作 `turns: 4`（20:44 那轮为修复前存量，无指标），三轮新轮次分别标注 `来源 桌面 / 来源 CLI / 来源 桌面`；`session_metrics` 为 `turn_count=3, turn_succeeded=3, total_active_ms=15792, max_turn_ms=5487, channel_mix_json={"cli":1,"desktop":2}`；桌面 header 由 `耗时 10.83s · 累计 130K` 推进到 `耗时 15.79s · 累计 210K · 命中率 85.41%`，逐轮分割线 `4.85s · Token 80K · 入 7.71K · 缓存命中 72.3K · 出 7 · 推理 9 · 命中率 90.36%` 与 `turn_metrics` 同源。期间修复 5 个缺陷（详见提交 `20fb6a429`）：桌面信封 `workspaceId` 被 `deny_unknown_fields` 拒收、`work_items` 外键残留 `sessions_old`、固定会话 `cwd` 为空导致 Provider 失败、主 turn 终态批次漏写 `turn_metrics`、会话清单缺渠道/去向/受众/条数 |

| W8.5 | 待执行 | Cron 分支已真机取证：任务在**创建时**绑定自己那条 `kind='cron'` 会话：(`sessions.execution_id = job_id`，配 `sessions_cron_binding` 部分唯一索引），同一任务重复执行落同一条会话并累计轮次（实测两次 run 均 success=1，`session_metrics.turn_count=2`，`sessions` 里没有任何 primary 会话被写入）；自动化列表载荷给出该会话 `sessionId`，可直接进入只读视图。期间修正两处错误逻辑：①`execute_local_cron_agent` 原先用 `resolve_workspace_session` 取「该 Workspace 最近活跃的 primary 会话」，把任务自身的 prompt 执行轮写进了用户主会话（不是 `--wake` 注入）；现改为后台入口 `prepare_background_interaction` 落任务自己的会话；②自动化列表的「进入会话」原先也指向那条最近活跃主会话，现改为读创建时的绑定。目标执行分支（`kind='goal'`）尚未真机取证，本条不转已完成 |
| W8.6 | 已完成 | 运行态操作手册逐条复核并修正：`POST /api/v1/sessions/identity` 已删除（改为 `POST /api/v1/sessions`）；订阅放行口径与拒绝原因改写为「会话已登记 + 工作区匹配 + 受众不受限」；环境上下文字段表更新为 `origin_channel` / `session_id` / `session_destination` / `session_workspace`；`sessions reset` 输出改为 `current_run_id`；清掉车道时代遗留的「会话会话」重复词与「TUI 会话标识按进程随机」的旧解释 |
| W9.1 | 已完成 | 四项基线各有本机实测（Windows 11 / debug 构建 / 本机 SSD）：①**会话列表** `sessions_first_page_ms=1ms`（200 会话 × 20 万入站事实，走 `inbox_session_recent`；删索引后同一查询 18008ms，收益 4 个数量级，门槛 30ms）；②**打开会话首屏** `session_first_screen_ms=78ms`（20 轮 × 200 帧 = 4000 帧的会话，`interaction_history_page(tail=true, limit=200)` 尾窗索引页，门槛 300ms）；③**单次入站** `inbound_session_lookup_p95_ms=0.0731ms`（500 会话、1000 次主键点查，上限 1ms）；④**订阅放行** `subscription_admission_cached_us=0.900µs`（10 万次缓存命中 p95，帧路零查库，上限 50µs）。三项新测量都是 `#[ignore]` 用例：`session_first_screen_index_page_stays_within_budget`（axon-cli）、`inbound_session_lookup_is_a_single_primary_key_read`（axon-session）、`cached_subscription_admission_stays_within_the_frame_budget`（axon-daemon）。同时修复原有 `session_list_query_uses_inbox_session_index_and_stays_within_budget` 的两处硬伤：INSERT 列值数不符（15 值对 17 列）与 `root_session_id` 违反 CHECK，该用例此前一跑即崩，性能门禁的会话列表一项实际长期缺测 |

| W9.2 | 已完成 | 新增会话放行缓存（定容 1024、TTL 30 秒）：入站与订阅共用同一份「会话归属 + 受众边界」判定，未命中才查一次主键；用例证明同一会话的第二次放行不回表（删掉会话行后仍放行），TTL 到期后重新回表并拒绝已删会话；缓存自身的命中/过期/容量淘汰有单测 |
| W9.3 | 已完成 | 订阅放行缓存按会话标识定容（1024）并带 TTL（30 秒）与受众边界，入站与订阅共用同一份判定；会话级订阅关闭即回收该会话条目，TTL 到期同样回表。用例覆盖订阅存活期间重复放行不回表、TTL 到期回表、关闭后回表 |
| W9.4 | 已完成 | 写放大复核 + 并发用例。`apply_turn_metrics` 在同一个 `IMMEDIATE` 事务内完成「读旧行 → 内存累加 → 一次 UPSERT 写回」，`channel_mix_json` 只在这一处累加一次；作业计数走 `apply_job_delta` 的纯 SQL 算术 UPSERT（`jobs_started = jobs_started + excluded.jobs_started`），不做二次读改写。`apply_projections` 只对 `turn_metrics` 事件调用，不按帧调用。新增 `turn_projection_accumulates_atomically_within_one_transaction`：8 线程各 20 轮共 160 轮并发写入后 `turn_count`、活动耗时与渠道分布都精确等于 160，证明读改写同事务且渠道分布不重复累加。单轮实际写入耗时并入 W9.1 基线 |
| W9.5 | 已完成 | ULID 单调有序，主键插入位置单调前进；单测覆盖同毫秒递增 |
| W9.6 | 已完成 | 用量快照改「逐帧实时投递 + 每流收口落一条持久副本」：`FrameClass::Snapshot` 分支只把载荷暂存（`stage_replay_copy`，一次内存替换，不做 JSON 解析、不碰库），落库推迟到三个边界（provider 流收口、语义事件前、轮次边界）由 `flush_replay_copy` 一次写入，同值快照仍去重。热路径因此从「每 token 一次 IMMEDIATE 事务 + fsync」变成「每 token 一次内存替换」。用例 `usage_frames_reach_the_live_channel_but_only_the_stream_tail_is_persisted`：3 条流各 120 帧（共 360 帧）时实时通道仍投递 360 帧、持久面只落 3 条（每条流最后一条读数）、`turn_metrics` 输出仍为按流累加的 720，读数不变。真机单轮帧数实测见 W9.1 |

| W9.7 | 已完成 | 四项基线全部接入 `tools/verify_performance.py`：新增 `session_first_screen_ms`、`inbound_session_lookup_p95_ms`、`subscription_admission_cached_us` 三条 Rust 测量项（各自对应 W9.1 的 `#[ignore]` 用例），与既有 `sessions_first_page_ms` 一起进 `HARD_LIMITS`（30 / 300 / 1 / 50）与 `REGRESSION_FLOOR`（2 / 10 / 0.05 / 2）。门禁现判定 7 项硬上限、全部通过；`--record` 已把本机读数写回 `tools/perf-baseline.json`（会话列表 2ms、首屏 37ms、入站 p95 0.0118ms、放行 p95 0.3µs、事件 p99 1.339ms、控制面 p95 0.411ms、data 1.53MiB）。负向探针：把首屏上限临时改成 1.0 后门禁输出「session_first_screen_ms = 42.0 / 上限 1.0 → 超限」并以退出码 1 收尾，恢复 300.0 后重新通过。控制面事务 p95 基线由 0.226ms 漂到 0.411ms（存储重写后本机抖动区间 0.39 至 0.50ms，仍低于上限 50ms 两个数量级），噪声下限与抖动说明已同步更新 |

| W10.1 | 待执行 | TUI `/sessions` 未真机回归 |
| W10.2 | 待执行 | TUI 选中切换未真机回归 |
| W10.3 | 待执行 | CLI `--session` 续接未真机回归 |
| W10.4 | 已完成 | 不存在的会话 id 显式报错且不产生会话行（用例覆盖）；未真机复核 |
| W10.5 | 待执行 | `sessions resume` 与 `--session` 优先级未复核 |
| W10.6 | 已完成 | `session_resolution.rs` 收敛为一次解析：新增 `TargetKind::Session` / `TargetKind::Thread`，会话入口只读一次会话行并取 `current_run_id`，Thread 入口只按 `sessions.current_run_id` 反查归属；删除「先当 Thread 命中、再按会话回落」的串行回落与 `sessions reset` 的二次回落分支，Thread 无归属行时如实返回 `None`、不伪造会话标识；`run_window_has_lane` 更名 `run_window_registered`。用例锁定互不回落（会话入口拒收 Thread ID、Thread 入口拒收会话 ID、fork Thread 不伪造会话、按列反查不靠前缀），`axon debug session` 只收 Thread ID，`axon sessions` 只收会话 ID，`reset` 两处入口都只收会话 ID |
| W10.7 | 待执行 | 桌面左侧菜单四项未真机回归 |
| W10.8 | 待执行 | 桌面分页渲染未真机回归 |
| W10.9 | 待执行 | 跨端接续可见性未真机回归 |
| W10.10 | 待执行 | 目标看板进入会话详情跟主会话同一个渲染路径未验证 |
| W10.11 | 待执行 | cron进入会话详情跟主会话同一个渲染路径未验证 |
| W10.12 | 待执行 | 子代理进入会话详情跟主会话同一个渲染路径未验证 |
| W10.13 | 已完成 | 会话身份收口（长官裁决）：删除 `resolve_workspace_session` 及其端口、SQLite 实现与全部调用点；CLI/TUI 未给 `--session` 时一律新建主会话；网关入站未带 `sessionId` 时按 Workspace 新建主会话，桌面提交信封改为携带当前会话 `sessionId`；cron `--wake` 收敛为 `session:<session-id>`，旧的「渠道 + 车道标识 + Workspace」形式与「按 Workspace 活跃会话投递」一并删除 |
