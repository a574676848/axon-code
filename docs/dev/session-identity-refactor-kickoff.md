# 会话身份重构 开工提示词

> 用途：新开一轮实施会话时，把下面整段作为首条指令贴入。它自包含任务、事实底座、工具纪律与验收口径。

---

## 一、任务

在当前 Axon 工作区内实施「会话身份重构」，把会话标识改为唯一、生成式、与渠道无关的 ULID，渠道下沉为轮次事实。严格按 `docs/dev/session-identity-refactor-plan.md` 执行，该文件是本次唯一计划源，不要另起方案。

前置裁决已定，不要再讨论：
- 不做兼容层，不写存量迁移。
- 本机会话数据物理删除后重建，记忆库与配置保留。
- 表结构重建用一次性临时脚本，脚本放在仓库外，处理完删除；代码内只保留最新表结构，不写任何升级 schema 逻辑。

## 二、强制工具纪律（不可协商）

本节优先级高于任何便利性考虑。违反即视为任务未完成。

### 2.1 结构检索必须走 codebase-memory-mcp

- 动手前先 `list_projects`。本项目可能同时存在 `axon-code` 与 `E-zbg-axon-code` 两个条目指向同一仓库，此时必须对两者各跑一次 `index_status`，用最后一次索引时间与节点、边数量判断哪份更新，只对更新的那份下结论；判定不清就 `index_repository` 重建一份再用，不能对着旧图谱下结论。未索引或明显过期时同样用 `index_repository` 补。
- 找定义、调用者、被调用者、影响面，一律先走图工具，禁止首选手写 grep：
  - `search_graph`：按名称或自然语言定位符号、路由、变量。
  - `trace_path`：追踪 inbound 与 outbound 调用拓扑，改共享符号前必跑一次。
  - `get_code_snippet`：读精确源码，替代整文件通读。
  - `query_graph`：多跳与聚合依赖，用于找「谁在用会话标识」这类横切问题。
  - `get_architecture`：先建立分层与入口的整体方位。
- 下否定性或穷尽性结论前必须 `check_index_coverage`，覆盖不到的路径回到 read 逐行核对，不得把「无缺口记录」当成完整性证明。
- 只有检索字面量、错误串、非代码文件，或图谱明确未覆盖时，才允许降级 grep；降级必须在汇报里写明触发证据。

### 2.2 代码修改必须走 morph-mcp

- 语义定位优先 `codebase_search`，不要靠猜文件名。
- 核心逻辑修改强制用 `edit_file`：`crates/axon-session`、`crates/axon-cli`、`crates/axon-daemon`、`crates/axon-agent-runtime` 下的所有改动都属于核心逻辑。
- `edit_file` 必须用 `// ... existing code ...` 占位符保留未改段落，同一文件多处改动在同一次调用里提交，压缩往返。
- 禁止用 shell 重定向、`Set-Content`、`sed` 之类手段改写代码文件；`write` 仅用于新建文件或整体重写极小文件。
- 遇到第三方库行为疑问时用 `github_codebase_search` 查上游，不要凭记忆断言。

### 2.3 只读事实不要猜

会话标识、表结构、字段语义一律以源码与真实库为准。本提示词给出的事实底座已核实，可直接当起点；与源码冲突时以源码为准，并当场记录差异。

## 三、必读顺序

1. `docs/dev/session-identity-refactor-plan.md`（计划书与 WBS，含性能红线与三端回归）
2. `docs/dev/storage-architecture.md` 的 §4.13 与相关裁决段（会话身份契约）
3. `AGENTS.md` 与 `~/.dsh/coding.md`（工程与交付规范）
4. 涉及桌面端界面时，另读 `docs/dev/ui-constraints.md` 与 `docs/dev/workbench-design-system.md`

## 四、事实底座（已核实，可直接引用）

### 4.1 会话身份的当前实现

- `crates/axon-session/src/application/session_identity.rs`
  - `SESSION_SESSION_ID_PREFIX = "session-session-"`，为会话标识前缀。
  - `session_id_for_route(workspace_id, audience, channel, destination)` 把四个值序列化后取 SHA-256，`channel` 在哈希输入里。
  - `interaction_session_id(interaction)` 现算派生，返回 `Option<String>`。
- 六个派生调用点：`workbench_http_adapter.rs`、`cron_http_adapter.rs`、`gateway_voice.rs`、`daemon_history_loader.rs`、`interrupted_interaction_repository.rs`、`interaction_adapter.rs`。
- 对外输出：`crates/axon-daemon/src/application/interaction_turn_payloads.rs` 里 `primarySessionId` 的值来自 `interaction_session_id`。

### 4.2 已经正确、不要动的两处

- 轮次来源标注：`crates/axon-eventlog/src/record.rs` 的 `origin_channel` 字段与写入路径。
- 会话级渠道分布：`session_metrics.channel_mix_json`，累计在 `session_metrics_repository.rs`。

### 4.3 已知飘逸

| 编号 | 飘逸 | 位置 |
| --- | --- | --- |
| D1 | 渠道进入会话身份哈希 | `session_id_for_route` |
| D2 | 会话标识有数值摘要副本 | `crates/axon-cli/src/run_commands/run_tool_loop.rs` 的 `stable_numeric_id(&request.session_id)` |
| D3 | 会话类型编进 id 字符串，表内无 `kind` | `goal-session-<goalId>`、子代理拼接 |
| D4 | `trust_scope` 只存在于哈希输入 | `sessions` 表无该列 |
| D5 | 问答记录把 cell id 当 interaction id | `crates/axon-cli/src/tools/orchestrator.rs` 的问答记录构造 |
| D6 | 文档仍写 `primaryLaneId`，代码只有 `primarySessionId` | 文档与代码不一致 |
| D7 | `interaction_id` 生成形态不统一 | `external-interaction-<hash>` 与 `cron-wake-<hash>` |
| D8 | 会话标识返回 `Option` | `interaction_session_id` |

### 4.4 代码内已存在的 schema 升级逻辑（本次要删）

- `crates/axon-session/src/infrastructure/sqlite_runtime_store.rs` 的 `ensure_schema`，control 与 outbox 两侧都会调用（`runtime_store_impl.rs` 里两行）。
- 只保留按最新 DDL 建表；补列、加列、legacy 重命名分支全部删除。
- `usage_statistics`、`memory`、`persona`、`mcp_schema_cache` 里的补列逻辑不在本次范围，不要顺手改。

## 五、执行顺序

严格按 WBS 的 W0 到 W10 推进，不得跳批：

1. W0 环境准备。**必须先 `cargo check --workspace` 通过，才允许删数据**，否则会出现删完装不上。删数据只清会话相关表与目录，不得整库删 `control.sqlite3` 与 `outbox.sqlite3`。
2. W1 规范与决策收口，同步文档，作废旧裁决。
3. W2 数据模型，含删除会话库 `ensure_schema` 内的升级分支。
4. W3 身份层，删除派生函数与数值摘要，收紧返回类型。
5. W4 创建与入站，含强制携带会话标识。
6. W5 订阅与放行，放行校验必须以 `trust_scope` 实列为准，不得省略。
7. W6 六个调用点收敛。
8. W7 渠道与轮次核对，这一层已正确，只做复核。
9. W8 门禁与验收。
10. W9 性能专项，四项基线必须实测。
11. W10 三端用户可见回归，逐条走真实入口。

## 六、破坏性操作边界

- 允许：删除本机会话数据，按 §6.2 清单逐表清空与逐目录删除。
- 允许：重建会话相关表结构与索引约束，用仓库外的一次性脚本。
- 禁止：整库删除 `control.sqlite3` 与 `outbox.sqlite3`。
- 禁止：删除记忆库、人格、配置、技能、工作区、定时任务、目标、用量统计。
- 禁止：把临时脚本提交进版本控制。
- 安装覆盖前必须确认版本目录取自 `current.json`，且 `axon.exe` 与 `axon-desktop.exe` 同时存在。

## 七、门禁（缺一不得收口）

- `cargo fmt --all --check`
- `cargo check --workspace`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `pwsh -File tools/verify-architecture.ps1`
- `pwsh -File tools/verify-docs.ps1`
- 改动界面时另加：`npm ci`、`npm run build`、`npm run lint`、`npm test`、`pwsh -File tools/verify-ui-contract.ps1`
- `git diff --check`

## 八、真机验收

- 安装态二进制必须是 `$HOME/.axon/bin/versions/<versionId>/` 下的产物，并与 `target/release` 哈希一致。
- 桌面端经稳定入口启动，CDP 端口用 `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` 打开，验证结束清掉。
- 判定看 `tools/cdp-*.ps1` 的输出前缀：`OK:` 才算通过，`ERR:` 一律视为未验证。
- 三端回归矩阵（W10）必须逐条走真实入口，不能用不变量探针替代。

## 九、熔断与裁决

- 同一问题连续三次尝试未过即停，带上已排除项与两到三个备选方案上呈。
- 遇到计划书第 13 节的三个待裁决项（可读前缀、子代理是否统一 ULID、`kind` 是否需要第四类）时停下问，不要自行决定。
- 遇到与他人在同仓库并行改动冲突时，不覆盖对方改动，先核对 `cargo check` 再继续。

## 十、交付格式

1. 本批完成了 WBS 哪些条目，逐条给状态。
2. 变更摘要：改了哪些文件，核心逻辑与设计意图。
3. 验证证据：门禁输出、性能实测值、三端回归结果。
4. 与计划书的偏差，以及偏差原因。
5. 未完成项与阻塞点。
6. 记忆治理回显，按 `AGENTS.md` 记忆层 M4 格式。
