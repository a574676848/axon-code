# 使用统计

Axon 使用统计系统把智能体运行记录异步投影为独立的聚合数据库，供命令行 CLI 和监控统计看板读取。统计采集采用完全解耦的旁路设计，不侵入模型流式传输（Provider Streaming）、工具执行或会话写入的关键路径。

---

## 1. CLI 统计查询

通过 `axon usage` 命令查询运行聚合指标与最近请求明细：

```powershell
# 查询今天的聚合结果
axon usage

# 查询最近 7 天并立即增量补齐最新记录
axon usage --period 7d --refresh

# 按 Provider、模型或调用来源过滤
axon usage --period 30d --provider openai
axon usage --period all --model gpt-5
axon usage --source subagent

# 按特定会话 (Session) 或单次运行 (Run) 隔离统计
axon usage --session session-abc123
axon usage --run assistant-part-42

# 输出结构化 JSON，供外部脚本或自动化系统消费
axon usage --period all --format json
```

### 查询参数规范

| 参数 | 取值范围 | 说明 |
|---|---|---|
| `--period` | `today`、`7d`、`30d`、`all` | 统计时间窗口，默认 `today` |
| `--source` | `all`、`axon-cli`、`subagent` | 调用来源过滤 |
| `--session` | 字符串 (Session ID) | 按会话 Thread ID 精确过滤 |
| `--run` | 字符串 (Assistant Part ID) | 按单次运行 Assistant Part ID 精确过滤 |
| `--provider` | 字符串 (Provider 名称) | 按服务商名称过滤 |
| `--model` | 字符串 (模型标识) | 按模型名称过滤 |
| `--limit` | 1 ~ 500 (默认 10) | 返回最近请求明细的最大条数 |
| `--refresh` | 标志开关 | 查询前立即触发一次增量投影 |
| `--format` | `text`、`json` | 输出格式，默认终端可读文本 |

### 查询聚合策略

- **常规查询**：直接读取预聚合表（`usage_rollup_hour` / `usage_rollup_day`）和有限条最近事实，避免全表扫描。
- **会话/单次穿透查询**：指定 `--session` 或 `--run` 时，为确保会话间严格隔离，汇总数据、时序趋势与明细直接从 `usage_facts` 实时聚合，不复用跨会话的聚合桶。
- **按需增量刷新**：使用 `--refresh` 会在执行查询前完成一次增量投影，确保读取到最新的运行事实。

---

## 2. 可观测性 HTTP 接口

桌面工作台的用量统计经网关的 `/api/v1/observability/*` 路由族读取，全部只读，复用网关同一套 Token 鉴权与统一错误模型：

| 方法与路径 | 查询参数 | 返回 |
|---|---|---|
| `GET /api/v1/observability/overview` | `period`、`provider`、`model`、`source` | 汇总读数、迷你走势、活动日历分块与健康时间线 |
| `GET /api/v1/observability/realtime` | `window`（`15m`、`30m`、`60m`） | 进程内滑动窗口的每分钟吞吐、请求结果与 Token 构成 |
| `GET /api/v1/observability/distribution` | `dimension`（`model`、`provider`）、`period` | 分项占比与时间序列 |
| `GET /api/v1/observability/diagnostics` | `period`（仅 `today`、`7d`） | 延迟分位数、散点样本与模型单价效益 |
| `GET /api/v1/observability/audit/logs` | `model`、`source`、`status`、`from_ms`、`to_ms`、`cursor`、`limit`（上限 500） | 请求事件明细与游标 |
| `GET /api/v1/observability/audit/export` | `format`（`csv`、`json`）与上述过滤条件 | 事件导出 |
| `GET /api/v1/observability/pricing/rules` | 无 | 价格规则清单 |
| `POST /api/v1/observability/pricing/rules` | 规则体 | 新建价格规则（规则 ID 由服务端生成） |
| `DELETE /api/v1/observability/pricing/rules/{id}` | 路径参数 | 删除价格规则 |
| `POST /api/v1/observability/pricing/sync` | 无 | 从 models.dev 同步费率 |
| `POST /api/v1/observability/pricing/recalculate` | `from_ms`（可选） | 历史成本重算 |
| `GET /api/v1/observability/pricing/auto-sync` | 无 | 每日自动同步任务的状态 |
| `POST /api/v1/observability/pricing/auto-sync` | `enabled` | 创建或启用 / 暂停每日自动同步任务 |

时间序列与健康时间线一律**按时间桶合并后返回**：`usage_rollup_hour` 的主键含 provider、model、source 与 status，同一小时存在多行，直接展开会把一个时刻画成多个点。健康度按终态判定（`completed` 计成功，其余终态计失败），与汇总读数同源，不使用「未定价」口径。

### 桌面看板

入口在桌面端「设置 › 用量统计」，按页签分屏：概览（读数、用量趋势、用量排行）、实时（滑窗吞吐与 Token 构成）、活动（活动日历与请求健康时间线）、诊断（延迟分位阶梯与散点）、请求日志（单次请求明细）。周期可选 `today`、`7d`、`30d`、`all`；诊断固定在上限 7 天的窗口内；实时面按 30 秒自刷新，其余面板由手动刷新按钮触发。

### 模型定价与自动同步

入口在桌面端「设置 › 模型定价」，管理运行态用量库里的价格规则：

- **规则清单**按服务端的四级全序排列（优先级降序、匹配式字面字符数降序、更新时间降序、ID 升序），单价的读写单位是美元 / 百万 Token，落库为微美元。
- **新增与删除**直接作用于规则表；界面上的「编辑」是先建后删两步，因此中途失败时原规则保持不变。改动规则不会自动改写已落库的历史成本，需要触发一次历史重算。
- **手动同步**调用 `POST /pricing/sync`，受 60 秒防抖保护；回执给出结论（`success`、`debounced`、`rate_limited`、`fallback`）与原因，上游不可达时保留本地规则快照。
- **同步的收录范围**：models.dev 的目录是「供应商 → 模型」的分层结构，同步**只收录与本机配置同名供应商的费率条目**，并按「匹配式 + 单价」去重、跳过全零条目。理由有三：同一个模型 id 在不同供应商下价差可达数倍，全量导入等于按任意一条覆盖事实；目录里的 0 价是订阅或免费额度这类计划信息，不是可比单价；规则只按模型匹配，不收同名供应商之外的条目才不会张冠李戴。同步写入的规则优先级为 `-1`，**手工规则（默认优先级 0）始终优先**。
- **别名与上游 id 不同**时同步覆盖不到：本机配置里的 `name`（例如 `deepseek-v4.1-flash`）与上游 id（`deepseek-flash`）不是同一个字符串，规则按用量事实记录的字符串匹配，因此这类别名需要显式建规则。
- **自动同步**打开时创建或启用系统 Cron 任务 `axon.system.pricing-sync`（脚本 `axon.internal.pricing-sync`，周期一天），关闭时转为暂停并保留历史运行记录。任务状态、上次运行结论与下次运行时间都以任务本身为准，不由界面另记一份开关。

---

## 3. 架构与数据流

```text
Coding Runtime
    │
    ▼ 写入运行记录与完成事件
sessions/rollout.sqlite3（唯一事实来源，只读接入）
    │
    ▼ Daemon + Axon Cron（默认每 60 秒调度）
usage projector（基于 rowid 水位线增量、幂等事务处理）
    │
    ▼
usage/usage.sqlite3
    ├── usage_facts             (单次请求明细事实表)
    ├── usage_rollup_hour       (小时级预聚合桶)
    ├── usage_rollup_day        (天级预聚合桶)
    └── usage_projection_state  (增量投影水位线与状态)
    │
    ▼
axon usage / 监控看板
```

### 调度与执行机制

- **任务载体**：由后台守护进程（DaemonHost）驱动，注册为 Axon Cron 持久化任务：
  - **Job ID**：`axon.system.usage-rollup`
  - **内部任务**：`axon.internal.usage-rollup`
  - **默认周期**：60 秒
- **状态领取机制**：Daemon 维护线程依据 `status + next_run_at_ms + claim_until_ms` 复合索引领取到期任务，不随历史任务总量线性扫描。
- **错期补偿机制**：若 Daemon 暂停或错失调度周期，无需逐次补跑。下一次运行时会自动基于持久化的 `rowid` 水位线增量处理期间产生的所有数据。

---

## 4. 存储模型

使用统计与主运行会话解耦，采用独立的 SQLite 数据库文件（`usage/usage.sqlite3`）：

1. **`usage_facts`（事实明细表）**
   - 以 `(session_id, assistant_part_id)` 为唯一主键，保证幂等。
   - 记录字段：Provider、模型、调用来源、终态状态、各类型 Token 数（输入/输出/推理/缓存命中）、Provider 调用次数与总耗时。
   - 仅用于有限数量的最近请求查询和聚合桶局部重建。
2. **`usage_rollup_hour` 与 `usage_rollup_day`（时序预聚合表）**
   - 分别按小时与天的时间桶粒度，结合 Provider、模型、来源和终态进行多维预聚合。
   - 承载常规看板与 CLI 概览的毫秒级快速检索。
3. **`usage_projection_state`（投影水位线表）**
   - 记录当前已投影的 rollout `rowid` 水位线、最后成功同步时间戳及累计事实总数。
   - 基于 SQLite 隐式索引进行游标推进，增量开销仅取决于新增数据量。

---

## 5. 事实收敛与保留策略

### 状态收敛与终态推进

- **事实源驱动**：投影器以 `rollout.sqlite3` 的 turn 快照为单一事实源。
- **状态流转与复核**：对于在同一 `(session_id, assistant_part_id)` 原地更新的 turn，投影器除基于 `rowid` 水位线增量扫描外，每轮处理还会针对未完结的 `active` 记录执行有界主键复核（每轮最多 256 条），确保进行中状态（active）在完成后能精准收敛为终态（`completed` / `failed` / `interrupted`）。
- **真实未完成判定**：无外部调用且未产生终态事件的尝试保留为 `active`，系统不进行猜测性标记。

### 生命周期与数据清理

- **保留周期**：Usage facts 事实明细默认保留 **30 天**。
- **清理策略**：清理作业仅清除超过保留期且已处于终态的非 `active` 记录，每批次最多处理 256 条。
- **原子事务保障**：单次清理在同一事务内完成事实删除、关联小时/日聚合桶重建、过期桶清理与计数更新，由 `axon.system.brain-cleanup` 任务统一维护调度。

---

## 6. 一致性与恢复机制

- **幂等写入**：相同 turn 重复投影时，来源指纹（Source Fingerprint）保持恒定，避免重复计数。
- **原子提交**：`usage_facts` 增量、预聚合时序桶更新以及 `rowid` 水位线均在单个数据库事务内提交。
- **失败回滚**：任一步骤失败则事务整体回滚，水位线不前移，待下一调度周期自动重试。
- **无锁读隔离**：投影器通过只读（`NO_MUTEX`）连接读取 `rollout.sqlite3`，并设置短 Busy Timeout，绝不阻塞前端交互与模型推理执行链路。
- **投影可重建性**：`usage.sqlite3` 为完全派生的投影数据，若损坏或丢失，可通过全量回放 `rollout.sqlite3` 无损重建。
- **陈旧度提示**：CLI 查询输出中的 `projection.stale` 字段明确指示当前投影数据相较于底座事实源是否存在未同步更新。

---

## 7. 指标与成本定价边界

- **统计指标集**：
  - 请求总数与成功率；
  - Token 分类用量（输入 Token、输出 Token、推理 Token、缓存命中 Token）；
  - 缓存命中比率；
  - Provider 实际调用次数与平均响应耗时；
  - 单次请求的生成速率：与工作台头部同口径，分子含推理输出 Token，分母是该轮各条 provider 流的实测流式耗时之和减去首字（首字只从第一条流的时间里扣）；
  - Provider 与模型维度的用量分布占比；
  - 小时/天维度的趋势曲线与最近请求历史。
- **定价边界**：
  - 鉴于各模型服务商的价格体系及费率协议变动频繁且不属于运行时核心契约，Axon 原生不对模型价格进行预设或推断。
  - 统计输出中的费用字段统一返回 `unpriced`（终端显示为“未定价”），确保统计结果严格基于事实数据，不引入非确定性测算。
