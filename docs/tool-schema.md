# 内置工具参数契约

内置工具的 JSON Schema 与运行时解析必须保持同一份可执行契约。模型容易把“单个值”和“数组”混用，因此仅对明确列出的字段做受控归一化，不对所有字段做隐式类型转换。

## 单值与数组

以下字段同时接受单值和数组；运行时会统一转换为单元素数组：

- `builtin.search.paths`、`builtin.glob.paths`：路径字符串或路径字符串数组。
- `builtin.glob.include`：单个 `files`、`directories`、`all`，或最多两项的非空数组（如 `["files","directories"]`）；省略时默认为 `all`。
- `builtin.memory_write.supersedes_memory_ids`：单个正整数 Memory ID 或正整数数组。

例如，下面两种写法等价：

```json
{"query":"axon","paths":"src","intent":"定位 axon 的用法"}
{"query":"axon","paths":["src"],"intent":"定位 axon 的用法"}
```

兼容范围是白名单，不应据此推断其他数组字段也接受标量。

## Memory 写入

`builtin.memory_write` 的 `kind` 必须是 `profile`、`preference`、`experience`、`knowledge` 或 `procedure`；`summary` 与 `evidence` 必须是非空字符串。运行时当前不执行敏感关键词过滤，内容通过基础校验后进入既有的 Memory 准入、合并与持久化流程。Personal scope 只接受前两类，Workspace scope 只接受后三类。写入后的 CLI/导出记录使用 `memoryKind` 返回同一稳定代码值。

桌面端记忆管理 Gateway 使用 `/api/v1/memory`：列表请求必须提供 `memoryKind`，并支持 `scope`、`limit`、`cursorUpdatedAtMs`、`cursorMemoryId`；统计使用 `/stats`（同样接受 `scope`）；实际存在的 scope 列表使用 `/scopes`；详情使用 `/{memoryId}`；创建使用 `POST /memory`；修改使用 `POST /memory/{memoryId}`，通过 `supersedesMemoryIds` 形成替代关系；遗忘使用 `POST /memory/{memoryId}/forget` 写入 tombstone；物理删除使用 `DELETE /memory/{memoryId}`。

`scope` 取值为 `all`（默认）、`personal`、`workspace` 或 `workspace:<namespace>`；缺省时返回当前 owner 的 Personal 记录加全部 Workspace 记录，其他 owner 的 Personal 记录不出现在任何读取路径。Gateway 从当前 owner 派生 Personal scope，并拒绝客户端提交 `userRef`、`namespaceRef` 等身份字段。列表按 `updatedAtMs DESC, memoryId ASC` 排序，用 `(updatedAtMs, memoryId)` 游标翻页，单页上限 100 条。

`memoryId` 与游标 `memoryId` 一律为十进制字符串：id 是 u64，超出 JS 安全整数区间，按数字传输会在 WebView 内被舍入并打错目标。`supersedesMemoryIds` 同样以字符串为准，并兼容未丢精度的数字。时间轴行还返回 `confidenceBasisPoints`、`decayedScoreBasisPoints`（按 30 天半衰期、以 `lastActiveAtMs` 起算的指数衰减）、`halfLifeDays`、`lastActiveAtMs`；`namespaceRef`、`repoRef` 等内部派生字段不下发。

写请求体的 `memoryKind`、`summary`、`evidence` 必填。`experience`、`knowledge`、`procedure` 属于 Workspace 记忆，必须额外提供 `workspacePath`（文件夹绝对路径），Gateway 用该目录经 Workspace Registry 派生 namespace 与 `repoRef`；`profile`、`preference` 属于 Personal 记忆，提供 `workspacePath` 会被拒绝。请求体可以是单条对象、对象数组或 `{"records": [...]}` 包装体；批量写入先整批校验再落库，任一条非法都不产生半批记录。未知字段一律拒绝。

所有列表、统计和写操作回执都包含 `needsRepair`，表示文本索引、向量投影或 HNSW 清理是否需要修复；统计回执另含 `legacyRecordTables`，列出升级时归档的旧记录表。

## 计划更新

`builtin.update_plan` 只用于复杂、跨模块、高风险或多阶段 coding 任务。每次调用都提交完整计划快照，不做单步骤增量修改。该工具的场景说明位于主 turn 专用的稳定 developer 片段 `<tool_orchestration_protocol>`；子代理 payload 既不装配该片段，也从工具列表过滤 `builtin.update_plan`，计划权只属于主 turn。

- `plan` 必须包含 3 到 7 个可验证结果，同时最多一个 `in_progress`。
- `intent` 必填：一句话说明本次更新要达成什么目的，写法与长度同 `builtin.shell_command.intent`。
- `status` 只接受 `pending`、`in_progress`、`completed`。
- `completed` 步骤必须提供非空 `verification`，记录实际测试、检查或工具证据。
- 实现类任务的计划必须包含最终 check 步骤；任务开始、阶段完成、范围变化或验证失败时更新计划。

示例：

```json
{
  "intent": "进入实现阶段",
  "explanation": "进入实现阶段",
  "plan": [
    {
      "step": "调用链证据闭环",
      "status": "completed",
      "verification": "图谱覆盖检查通过"
    },
    {
      "step": "普通 tool loop 可更新计划",
      "status": "in_progress"
    },
    {
      "step": "回放验证",
      "status": "pending"
    }
  ]
}
```

## 用户提问交互

`builtin.ask_user_question` 是主 turn 专属的工具：子代理不装配，模型可见工具面与执行注册表都不含它。它在需要用户决定、或缺少只有用户才知道的信息时向用户提问并等待作答；等待有硬上限，超限即把控制权交还用户，不会无限期占用线程。

- `summary`：必填，一句话说明这次在问什么（不超过 60 字符）。它是这条调用被收起时唯一可见的文本，不能留空，因此不展开也能看出模型问了什么。
- `questions`：向用户提出的问题列表（最多 8 项）；每项包含 `id`、`question`（不超过 512 字符）、可选 `header`、可选 `detail`（不超过 2 KiB）、可选 `options`（最多 20 项，每项含 `label` 与可选 `description`）以及 `multi_select`（布尔值）。
- `handoff`：可选文本，用户没有及时回答时展示给用户的话。
- 回执：等到答案返回 `status: answered` 与 `answers`；等待超限返回 `status: suspended` 且 `pending`、`suspended` 均为真；被取消返回 `aborted`。挂起不是同意，也不是跳过。
- 等待上限、退避与载荷上限是编译期常量（`crates/axon-agent-runtime/src/core/run/user_question_limits.rs`），不是配置键。
- 问答状态以 `WireEventKind::UserQuestion` 事件进入会话流（pending / extended / settled / suspended）；作答入口与未决查询见 [网关 HTTP 参考](skills/operate-axon/references/gateway-http.md)。

## Goal 工具

Goal 工具是主 turn 的持久工作能力。它们只装配到主 turn 的 `<tool_orchestration_protocol>` 和工具列表，后台 Goal Execution 与子代理不得调用。普通回答和澄清直接使用主 turn 的语义输出，不通过 Goal 工具伪装。

### `builtin.goal_create`

创建长周期或跨轮次的目标承诺，并在需要后台执行时同时创建首个待执行 Execution 与创建事件。

```json
{
  "scope": "current_workspace",
  "title": "非空标题",
  "objective": "可验证的目标",
  "steps": ["步骤一", "步骤二"],
  "acceptance": ["验收条件"],
  "createExecution": true
}
```

`scope` 只接受 `global`、`current_workspace`；省略时由当前场景确定。Goal 创建后 scope 固化，Workspace 切换不会迁移它。没有跨 turn 承诺时不要调用该工具。

### `builtin.goal_list`

按 scope、状态和分页游标返回 Goal 摘要。它是“检索所有 Goal”的受控入口，不能一次性返回无界历史。

```json
{
  "scope": "all_visible",
  "status": ["proposed", "active", "waiting", "paused"],
  "limit": 20,
  "cursor": null
}
```

`scope` 只接受 `current_context`、`global`、`current_workspace`、`all_visible`；`limit` 必须在运行时允许范围内；`cursor` 只可由上一页回执原样传回。结果受 audience、权限和当前 Workspace 过滤。

### `builtin.goal_get`

查看一个 Goal 的当前状态和进展，返回临时 `GoalActivityView`，包括 Goal、步骤进度、最新 Execution、最近相关 Event、最新 Result、待审批信息和结果游标。

```json
{
    "goalId": "goal-id",
  "includeExecutions": true,
  "includeEvents": true,
  "limit": 20
}
```

`goalId` 必须属于当前可见 scope；内部 `session_id`、lease、generation、provider checkpoint 等实现细节不得作为用户产品字段返回。

### `builtin.goal_start`

启动尚未执行的 Goal，或继续等待、暂停的 Goal。由你自主决定，无需额外询问用户；结果未知时先核实外部事实。

```json
{
  "goalId": "goal-id",
  "expectedRevision": 4
}
```

`goalId` 与 `expectedRevision` 均为必填；revision 过期、scope 不匹配或状态迁移非法时失败，失败不改变 Goal 与 revision。

### `builtin.goal_update`

以乐观并发方式更新 Goal。请求必须携带读取时拿到的 `expectedRevision`，并至少提供一个允许修改的字段。

```json
{
    "goalId": "goal-id",
  "expectedRevision": 4,
  "title": "新标题",
  "priority": 2,
  "status": "paused"
}
```

可更新字段由领域状态决定；objective、steps、acceptance 在 active Execution 期间默认拒绝直接修改。终态 Goal、scope 不匹配、revision 过期、状态迁移非法或字段为空时失败，失败不改变 Goal 和 revision。成功只递增一次 revision 并写入对应 Event。

### `builtin.goal_cancel`

明确取消一个 Goal，不等价于普通 `goal_update(status=cancelled)`。

```json
{
    "goalId": "goal-id",
  "expectedRevision": 4,
  "reason": "用户要求停止"
}
```

取消必须校验 scope 和 revision；Goal 原子进入 `cancelled`，queued Execution 不得领取，running Execution 收到持久 cancellation request 并在安全检查点停止。迟到 Result 不得覆盖取消事实；若 Goal 已完成、失败或已取消，返回明确的终态结果而不重复产生副作用。

### 执行边界

Goal 工具不设审批等待状态：`GoalToolCommand` 只包含 create / list / get / start / update / cancel 六个命令，不存在授权类命令。

执行边界的唯一裁决者是工具自身的 `ToolExecutionPermission` 与远控能力门（`RemoteCapabilityPolicy`）：`Allowed` 直接执行并记录审计，`Forbidden` 返回结构化拒绝原因且不产生副作用。

所有 Goal 工具均返回结构化回执；写工具回执包含最新 `revision`，列表回执包含 `next_cursor`。回执失败不会伪造成功状态，也不会改变账本事实。

## 工具指引的分层约定

工具相关文本按「模型在什么时机需要它」分四层，同一规约只在一层表述：

| 层 | 载体 | 回答的问题 | 覆盖身份 |
|---|---|---|---|
| 单次调用参数 | 工具 `description` / `input_schema` | 本次调用的每个字段怎么填：格式、取值、必填、转义 | 全部 |
| 跨调用规约 | `<tool_execution_and_receipt_protocol>` 片段 | 何时用哪个工具、命令与编辑文本的形态、回执怎么解读 | 主 turn + 子代理 |
| 主 turn 策略 | `<tool_orchestration_protocol>` 片段 | 计划、委托、Goal 与 Memory 等只对主 turn 成立的策略 | **仅主 turn** |
| 即时后续动作 | 工具回执的 `nextActions` / `guidance` | 刚完成的这一次调用之后做什么 | 全部 |

判定标准：

- 违反会导致**本次调用必然失败**的约束（格式、参数来源、取值）留在工具 schema。
- 只影响**多轮协作质量**的（工具选型、命令形态）进入 `<tool_execution_and_receipt_protocol>`。
- 依赖主 turn 专属工具（`builtin.update_plan`、`builtin.delegate_to_fast`、`builtin.goal_*`、`builtin.memory_*`）的进入 `<tool_orchestration_protocol>`；该片段子代理身份不装配。
- **回执解读**（`exit_code` 语义、`timed_out`、截断标志、`nextOffset`、hashline 失配）对两类身份同样有效，必须放在 `<tool_execution_and_receipt_protocol>`，不得放进仅主 turn 的 `<tool_orchestration_protocol>`。
- 只对**这一次调用的结果**成立的进入回执 `nextActions`；它随该次回执下发，不占每轮前缀。

> `<tool_execution_and_receipt_protocol>`、`<tool_orchestration_protocol>` 与工具 schema **都处于稳定前缀**并同样参与 provider 前缀缓存，
> 因此层间搬移**不改变 token 总量**。分层的目的是让每层只回答自己该回答的问题、
> 消除同一规约在 schema 与片段中的重复表述，而不是压缩上下文成本。

## MCP 工具直调

MCP 工具由 Provider 直接暴露并按完整名称调用；不再注册 `builtin.use_tool` 代理工具。

- 模型使用搜索结果中的完整 `qualified_name` 直接发起工具调用。
- `input` 必须严格匹配该工具自身的 `input_schema`；Freeform 工具按其声明的文本字段传递。
- 目标工具继续执行自身的 schema 校验、二态执行权限和输出策略。
- 执行注册表刷新后，后续 Provider 请求使用最新 MCP 工具定义。
- 外部 MCP `input_schema` 中声明为 `array` 却省略 `items` 时，注册边界会递归补充等价的开放项 schema `{}`。这保持 JSON Schema 的原始校验语义，同时兼容要求数组必须携带 `items` 的 Gemini FunctionDeclaration。
- `builtin.list_mcp_resources` 无必填字段；可用 `server` 限定单个 MCP server，省略时列出全部 server；`cursor` 只能与同一 `server` 一起传递。
- `builtin.read_mcp_resource` 必须同时提供 `server` 与资源列表返回的完整 `uri`。

## Skill 加载（`builtin.invoke_skill`）

`builtin.invoke_skill` 只接收一个必填字段 `skill_name`（非空字符串，不接受别名或额外字段），用于把该 Skill 的操作说明加载进当前上下文；加载后仍要调用说明里列出的真实工具才能完成任务。

```json
{
  "skill_name": "operate-axon"
}
```

Skill 可以在 frontmatter 中声明禁止模型调用；此类 Skill 由该工具加载时返回附带原因的失败，不会静默返回空说明。Skill 不存在或渲染失败同样返回结构化失败。

## 条件字段与枚举

- `builtin.shell_command` 的前台回执**始终**返回 `session_id`，超出首次等待窗口时进程继续在后台运行，后续用该 ID 调用 `builtin.write_stdin`；`run_in_background: true` 走的是另一条通道，回执返回 `job_id`（见「后台作业」一节）。会话生命周期与 `session_id` 的可达性：

| 会话状态 | 回执 `session_id` | 可读输出 / 终态 | 可写 stdin |
|---|---|---|---|
| `Running` | 返回 | ✅ | ✅ |
| 终态（结束 5 分钟内） | 返回 | ✅（幂等，重复查询结果一致） | ❌ 忽略写入与 `ctrl_c` |
| 终态（超过 5 分钟，已回收） | 返回 | ❌ 提示「已结束并超出保留窗口，请重新执行命令」 | ❌ |

  会话被回收后仍保留 30 分钟墓碑记录，因此凭已回收的 ID 调用会得到**可区分**的提示，而不是含糊的「不存在」——后者只用于从未产生过的 ID。
- `builtin.write_stdin` 在 `Running` 会话上：中断时设置 `ctrl_c: true`；写入字符时使用 `chars`；仅等待新输出时只传 `session_id`，可用 `yield_time_ms` 延长本次纯等待窗口。纯等待由新输出、进程终态或等待窗口结束唤醒；等待窗口结束不会终止进程。内部无输出检查采用有上限的指数退避，最大检查间隔为 5 秒，收到输出后恢复初始检查间隔，不影响有输出时的实时回执。
- 前台会话有**空闲回收窗口**：距进程创建或最后一次 `write_stdin` 触碰满 10 分钟即判定模型不再跟进，进程被终止（退出码 `130`、`cancelled=true`），并向本轮注入一次 `shell-session-reclaimed` 运行时通知 —— 载荷含 `session_id`、命令、归档路径与后续动作，因此模型不必轮询也能知道它已被回收。轮询（含纯等待）会刷新该窗口，持续跟进的长命令不受影响；预计超过窗口的命令请改用 `run_in_background: true`，或显式提高 `timeout_ms` 并保持轮询。回合结束时该 run 的全部会话由 `terminate_sessions_for_owner` 统一清理，因此该窗口只在回合内生效。
- MCP 列表工具使用分页 `cursor` 时，必须同时传递同一 `server`。

## 记忆窗口工具（notes / history）

`builtin.notes_*` 五件套（`notes_write_file` / `notes_append_to_file` / `notes_read_file` / `notes_list_files_by_prefix` / `notes_search_contents`）与 `builtin.history_*` 四件套（`history_list_windows` / `history_list_items` / `history_read_item` / `history_search_contents`）的 JSON Schema 逐字段对齐 Codex `history-notes` 扩展。正整数约束字段（`limit`、`max_results`、`max_chars_per_item`、`limit_chars`、`max_matches_per_file`、`max_files`）统一声明 `minimum: 1`；`offset_chars` 为 `minimum: 0`；`read_file` 的 `start_line` / `stop_line` 为 1-based 且负值从文件末尾回数；`role` 只接受 `user`、`assistant`、`tool`、`system`、`developer`。九件套均拒绝未知字段。

便签写入路径是会话便签目录内的相对虚拟路径：空组件、`.`、`..`、绝对路径与反斜杠一律拒绝，不做 shell 展开（`~` 按字面量处理）；`write_file` 整文件替换、`append_to_file` 原样追加，单文件不得超过 1,000,000 UTF-8 字节，接近上限时新建文件。读取、列表与检索都立即反映已成功的写入。`history_read_item` 必须同时提供 `item_id` 与 `window_id`，且 ID 必须原样使用历史工具返回值；未知窗口的列表与检索返回空结果。非 assistant 的 provider 可见条目正文尾部带 `[id: <window_id>:<ordinal>]` 标记，可原样记入便签作为后续回读引用；该标记不进入 system / developer 稳定前缀。

## 文件编辑格式

`builtin.edit` 统一支持 JSON Function-Call 与纯文本 Freeform 两种通道，具备输入参数别名映射（支持 `input`/`code_edit`/`patch`/`content`/`diff`）、语法混杂自动清洗与后置结构化自愈诊断。文件编辑核心格式严格二选一：

声明分层：**场景选型与格式约束只随工具 `description` 下发一次**，`input` 字段描述只讲该参数怎么填并指向工具描述。这段文本会随每次 provider 请求原样重复，因此禁止在参数描述中复述场景选型；字符预算由 `builtin_edit_tool.rs` 的 `edit_tool_definition_stays_within_budget` 守卫。

- 单文件小改且已读取并持有 `[path#TAG]` 回执时使用 Hashline：定位基于读取时快照，延迟最低、变更范围最直观；第一行必须是该回执，下一行开始写操作。
- 新建文件与整文件重写使用 `*** Add File:`：目标已存在时按整文件重写处理（内部规划为 Update，复用备份→硬链→回滚链路并继承原文件行尾与 BOM），无需再写 `*** Delete File:`；正文逐行以 `+` 开头，漏写 `+` 会被自动补全。
- 删除、移动/重命名与必须原子的跨文件批处理使用 PathDiff：必须由 `*** Begin Patch` 与 `*** End Patch` 包裹；没有可用快照、读取回执已过期、目标可能被外部修改或文件超过快照上限时同样使用它。必要时先重新读取以恢复 Hashline 快照。

不要用 `builtin.shell_command` 绕过文件编辑事务。成功回执包含 `files[].diff`、写后 `files[].hashline.header`、LSP 诊断摘要和 `nextActions`；后续同一文件小改应复用最新 header。

PathDiff 的 `*** Update File:` 块中，上下文行必须以空格开头，删除行以 `-` 开头，新增行以 `+` 开头。独立的 `// ... existing code ...` 或 `# ... existing code ...` 是兼容的 Lazy Edit 边界，运行时按 `@@` 处理；带 `+`、`-` 或空格前缀的同名文本仍按真实代码处理。不要把 Python、Rust 或 shell 命令原文放进更新块。执行命令统一调用 `builtin.shell_command`。

`*** Add File:` 的目标已存在时按**整文件重写**处理：内部规划为 Update，复用「备份源文件 → 硬链提升 → 失败回滚」的既有事务语义，并继承原文件的行尾与 BOM；缺失时仍为新建。回执的 `operation` 如实标注 `update`（不是 `add`），`files[].diff` 给出旧行 `-` / 新行 `+` 的真实前后对比。因此「整文件重写」无需再写 `Delete File` + `Add File` 两段（同批次对同一路径重复操作仍会被 `DUPLICATE_PATH` 拒绝）。

Hashline 头的 apply_patch 噪声剥离：方括号头里混入 `***`、`Update File:` / `Add File:` / `Delete File:` / `Move to:` 等关键字时（例如 `[*** Update File:src/a.ts#1A2B3C]`），输入规范化阶段会剥离噪声并重写为 `[src/a.ts#1A2B3C]`。该修复是保守的：只有在方括号内确实剥离出噪声关键字块且剥离后仍有内容时才改写，其余（例如 `[Update total: 3]`）原样保留交给解析器硬失败。

流式只读预览：编辑载荷在 provider 流式生成期间即被投影为只读 diff 快照。PathDiff 走补信封解析；Hashline 正文只携带新内容，因此预览会按**当前工作区文件内容**渲染真实前后对比（与 omp 的 `applyPartialTo(text, …)` 同思路）。预览全程只读且容错：目标文件缺失、section 标签陈旧、操作尚未写完时对应文件被跳过，不发出任何事件；读取被限制在工作区内（拒绝绝对路径与父目录跳转，只接受一般名称组件）。

read_file 的多格式投影：除纯 UTF-8 文本外，已支持 Notebook、PDF、Word、Excel、PowerPoint、压缩包安全清单，以及 **SQLite**（`.db`/`.sqlite`/`.sqlite3`）。SQLite 以 `SQLITE_OPEN_READ_ONLY` 只读打开，投影「对象清单 + schema + 每表有界数据行」（对象数、行数、列数与单元格长度均有界，输出再经统一上限收敛），**绝不创建、修改或写入目标文件**。所有派生文档都不产生 Hashline 快照与 seenLines，因此不能直接参与 Hashline 编辑。

编辑解析回归观测：Hashline 写入后，运行时用既有 tree-sitter 语法判断该文件是否由「可解析」变为「解析失败」，**只有本次编辑引入回归时**才在回执 `warnings` 追加 `[解析回归] <path> 由本次编辑从「可解析」变为「解析失败」（约行 X-Y）`。肇事区取所有 ERROR / MISSING 节点行范围的并集，向两侧扩 6 行上下文；跨度超过 150 行时退回错误节点原始范围。该观测**不阻断写入、不自动改写文件**，修复决策留给上层与模型；未知扩展名（无可用语法）时跳过判定而不报错。配套的修复接受判据（供上层使用）：候选内容必须重新解析成功，且**不得**是整段回退到编辑前文本 —— 只满足其一都会被拒绝（前者防"删干净蒙混"，后者防"接受仍然坏的候选"）。

陈旧快照恢复（Hashline）：读取后文件被外部改动时，运行时会尝试把编辑重放到当前内容上，但要求所有锚点的位移**完全一致**（等价于 line-map 上的一次整体平移），并对决定位移的锚点做邻域校验：锚点行值唯一时至少一侧邻居逐行一致，行值重复时两侧都必须一致；后续锚点改为按已确定位移直接逐行核对。任一条不满足即 fail-closed，回执要求重新读取文件，绝不猜测位置。

Lazy 块内的漏写前缀容错：当某个 `*** Update File:` 块内出现独立 Lazy 边界时，该块中漏写行首前缀的正文行在输入规范化阶段被补一个空格并按上下文行处理，`+`、`-` 与空格前缀的既有语义不变。Lazy 占位符本身、`@@` 变更上下文、Markdown 代码围栏、Hashline 头（`[path#TAG]`）与 shell 命令原文一律不参与补正，仍由解析器硬失败，以保证定向诊断不被吞掉；未出现 Lazy 边界的严格块保持原样硬失败。该容错由 `crates/axon-agent-runtime/src/agent/agent_run_tool_args_edit_normalize.rs` 的 `pad_lazy_update_block_lines` 实现。

```text
*** Begin Patch
*** Update File: src/lib.rs
@@ fn target()
-old()
+new()
// ... existing code ...
-older()
+newer()
*** End Patch
```

### 结构化错误诊断与自愈闭环
当编辑执行失败时，工具回执输出结构化诊断卡片与针对性自愈指引：
- `HUNK_CONTEXT_MISMATCH` / `HUNK_OLD_LINES_MISMATCH`：提供预期上下文 (Expected) vs 目标文件当前内容 (Actual) 的局部 Diff 对比，并自动附带失配点附近带行号的只读切片（Micro-read Snippet），无需额外调用 `read_file` 即可直接采用真实代码更新补丁。
- `HASHLINE_SNAPSHOT_MISSING`：提示会话未读取该文件，指引先调用 `read_file` 或改用 PathDiff 范式。
- `CONCURRENT_MODIFICATION` / `TRANSACTION_ABORTED`：得益于两阶段原子落盘，所有前序文件修改均已安全回滚，提示工作区保持干净一致。
- `NOOP_CHANGE`：提示新内容与原内容完全一致，引导反思是否写反符号或重复提交。

## Shell 命令格式

`builtin.shell_command.command` 是当前平台默认 shell 的裸脚本，不要再包一层 `pwsh -Command`、`bash -lc` 或 `cmd /c`。JSON 中脚本内的双引号必须转义；简单 Python 检查优先使用单行 `python -c`，避免把未缩进的多行 `with`/`for` 拼进 `-c` 字符串。命令失败时先根据 stderr 修正脚本，再重试，不要原样重复调用。

`builtin.shell_command` 与 `builtin.write_stdin` 的 `max_output_tokens` 必须是正整数，默认 2000。

`builtin.shell_command.intent` 是**必填**参数：一句话说明这次命令要达成什么目的，例如「校验代码格式」「确认依赖版本」。它是这次执行面向用户的说明，工具行与工具卡都以它作为标题，命令原文是详情。写目标、不复述命令原文、不加句号、40 字以内。

`intent` 是跨工具的展示契约，字段名与写法在所有声明它的工具上完全一致，当前包含 `builtin.shell_command`、`builtin.write_stdin`、`builtin.update_plan`、`builtin.glob`、`builtin.search`、`builtin.notes_write_file`、`builtin.notes_append_to_file` 与 `builtin.send_message`。它只影响展示，不参与执行语义：模型自述目的在参数被截断之前取出，因此再长的脚本、正文或计划都不会把它挤掉；未声明该字段的工具（含 MCP 工具与第三方 schema）不受影响。

两处内容分工固定，改任一处都要同步检查另一处：**写法与长度**（写目标、标点、字数、示例）属单次调用语义，写在该字段的 schema 描述里；**产品态承诺**（每次调用必填、它是这次执行面向用户的说明、不得复述命令）属跨调用约束，写在 `crates/axon-prompts/templates/agent/tool_usage_guidance.md` 的 `<tool_execution_and_receipt_protocol>` 片段里。界面机制（哪一端折叠、哪一端展开）两处都不写。

输出预算按每次回执的增量批次计算：读取回执会消费当前缓冲，前一批大输出被截断并不意味着最后一批短输出也会截断。`full_output_path` 指向跨批次保留的完整输出归档。截断与归档回归使用真实输出缓冲和自动清理的临时目录，不启动系统 shell。

归档存在容量上限：达到上限后停止落盘，回执 `reasons` 携带 `shell_command:artifact_truncated`，同时 `output.artifactTruncated` 为真且 `output.fullOutputAvailable` 转为假。此时 `full_output_path` 只是部分输出，不可当作完整日志读取；模型可见的有界预览不受影响，仍按 head + tail 正常返回。

Shell 输出有**两层独立的上限**，失败表现完全不同，不要把二者混为一谈：

| 层 | 手段 | 覆盖范围 | 触顶后的表现 |
| --- | --- | --- | --- |
| 应用层（artifact 上限，64 MiB） | Axon 停止写归档 | **仅** shell 输出的归档文件 | 归档停止增长，命令**继续执行**，回执标记 `shell_command:artifact_truncated` |
| 内核层（资源围栏） | `RLIMIT_FSIZE` / Job Object | 子进程的**所有**文件写入 | 进程被内核终止（Unix 为 `SIGXFSZ`，退出码 `153`） |

因此 `yes > /tmp/x` 这类命令会先撞上 artifact 上限（预览被截断但命令仍在跑），而 `yes > user-file` 这种不经归档的重定向只能由内核层拦住。正常的大输出命令（如 `cargo build`）由应用层处理，**不会**被杀进程：`RLIMIT_FSIZE` 默认 4 GiB，远高于单次构建产物量级。

围栏的默认值、平台差异与临时关闭方式见[配置指南的资源围栏一节](./configuration.md#子进程资源围栏)；命令被内核终止时的排查步骤见[故障排查](./troubleshooting.md#命令被资源围栏终止)。

### 工具结果的事件流内联预算

工具结果在进入会话事件流之前有一道固定预算：单条 **12,500 tokens（约 50 KB）**。超预算时按「头 + 尾 + 省略提示」裁剪，完整正文原子写入统一内容归档（与外部事件正文、子代理结果同一套归档），事件里给出绝对路径；`metadata` 中体积型的字段（例如搜索结果的文件清单）换成 `{"omitted": true, "bytes": ..., "count": ...}`，整体归档路径放在 `archivePath`。

模型可见的工具结果与界面收到的事件读的是**同一份**裁剪结果：一次大搜索结果因此只产生有界事件，不会把订阅流撑爆，也不需要传输层另设限制。预算是固定值，不随上下文窗口放大。

Axon 会为每次执行注入模型友好的环境变量：非 TTY 场景注入 `PAGER=cat`、`GIT_PAGER=cat`、`NO_COLOR=1`、`TERM=dumb`；TTY 场景保留 `TERM` 以维持交互式终端能力，但仍注入两个分页器变量。分页器在缺少交互输入时会一直等待，是 `git log`、`man` 等命令挂起的直接成因；该注入覆盖「工具自行按 TTY 决定是否分页」这一类。非交互保障由这组结构化手段共同提供 —— 注入的分页器与色彩变量、无 profile 的 `-NonInteractive` shell 形态、会话的 `timeout_ms` 截止时间与空闲回收、以及 `tty: true` 的独立 PTY 通道；运行时**不对命令文本作词表式判定**：`less`/`more`/`vim`/`ssh`、`docker -it` 之类的写法按普通命令原样执行，不再被预检拒绝。cwd 位于 WSL UNC 路径时，命令由 `wsl.exe` 承载，变量同时经 `WSLENV` 声明后才对 Linux 侧可见。

```json
{"command":"git status --short"}
{"command":"python -c \"from pathlib import Path; print(Path('docs').exists())\""}
```

修改这些字段时，必须同时更新 schema、解析器和回归测试。

## 临时 Job（后台作业与后台子代理）

`builtin.shell_command` 有三种执行形态。它们共用同一条命令校验链（命令规范、cwd、环境、超时预算），差别只在**终点与句柄**：

| 形态 | 触发条件 | 回执句柄 | 并发策略 | 后续工具 |
|---|---|---|---|---|
| 前台短命令 | 默认 | 无（直接返回终态） | `WorkspaceCoordinated` | 无 |
| PTY 交互式会话 | `tty: true` | `session_id` | `Exclusive` | `builtin.write_stdin` |
| 后台长任务 | `run_in_background: true` | `job_id` | `ParallelSafe` | `builtin.job_output` / `builtin.job_kill` |

**后台子代理是同一类临时 Job**：`builtin.delegate_to_fast` 缺省后台执行（`run_in_background` 默认 true），回执带 `jobId`，其生命周期与 Shell 后台作业完全同构 —— 同一个注册表、同一套 owner 校验、同一保留与墓碑窗口，用 `builtin.job_output` 读取结果与终态、`builtin.job_kill` 完整中断（中止当前 turn → 关闭会话 → 投影统一终态 → 触发完成通知 → 回收）；显式传 `run_in_background: false` 时才走同步路径，直接返回回执且**不产生** `job_id`。`builtin.spawn_agent` 保留在执行注册表但**不向模型暴露**（不出现在模型工具清单里），模型侧没有调用它的路径。

寻址规则写死：**生命周期操作走 `job_id`，协作操作走 `AgentPath`**。`builtin.send_message` / `builtin.list_agents` 只接受 AgentPath（root 与当前代理自身都会被稳定拒绝）；`job_id` 传给这两个工具、或把 AgentPath 传给作业工具，都会被稳定拒绝。

`run_in_background` 与 `tty` 是**硬互斥**：同时为 true 时拒绝执行并返回明确原因，而不是静默忽略其中一个。理由是两种句柄来自不同的注册表，静默分流会让模型拿着一个并不存在的 ID 继续操作；同理，`run_in_background: true` 只能经后台入口启动，`false` 只能经前台入口启动，两者不可互换。

两个 ID 属于**不同通道，不可互传**：

- `job_id` 是进程内单调递增的十进制数，只被 `builtin.job_output` 与 `builtin.job_kill` 接受；送入 `builtin.write_stdin` 会得到「会话不存在」。
- `session_id` 是会话 UUID，只被 `builtin.write_stdin` 接受；被 `builtin.job_output` / `builtin.job_kill` 拒绝为非法 `job_id`。

作业注册表与会话注册表相互独立，这是**构造性保证**而非约定：不存在把作业注册进会话管理器、或把会话 ID 当作业 ID 使用的路径。作业按 `owner_id` 隔离，owner 与 shell 工具同源（当前 Agent run 标识），其他 run 读取或终止都会失败。

回执契约：

- 启动回执只承诺「作业已启动、当前仍在运行」：立即返回，不在 `shell_command` 的 `yield_time_ms` 上等待；返回 `job_id` 而**不返回** `session_id`，也不返回退出码。等待增量输出改由 `builtin.job_output` 的 `yield_time_ms` 控制。
- `builtin.job_output` 的 `status` 为 `running` 只表示作业仍在运行；退出码只在终态出现。六类退出**互不合并**：`completed`、`refusal`（执行前的准入裁决拒绝，任务未被接受）、`failed`、`killed`（显式 `job_kill`）、`timeout`（墙钟超时）、`unresponsive`（空闲超时）；`refusal` 的 `error_type` 为 `refused`（不是 `execution_failed`），后三者的 `error_type` 为 `killed`。预算耗尽（模型输出 token 上限）落在失败类，但退出原因固定为「预算耗尽（模型输出 token 上限）」，不与普通失败混同。
- 输出是**增量**的：每次读取消费并返回自上次读取以来新增的片段，单次回执不是全部输出。预览受 `max_output_tokens` 约束，超出时保留头尾并标记截断，完整内容按 `artifactPath` 读取；`fullOutputAvailable` 为假时归档本身已被容量上限截断。
- 作业在终态后按保留窗口回收（终态保留 5 分钟，墓碑保留 30 分钟）；超窗读取提示「已结束并超出保留窗口，请重新执行命令」，与从未产生过的 ID 可区分。
- 作业进入终态时产生**统一完成通知**，经运行时注入回流到下一轮 provider 请求，模型无需轮询；通知载荷自证归属（生产者、作业引用、终态、退出原因、有界摘要或归档引用、待读 mailbox 引用）。daemon 侧 Goal Execution 完成沿用既有的持久投递收件箱。
- `builtin.job_kill` 终止作业及其进程树；对子代理则执行上述完整中断语义。作业已进入终态时返回提示而非重复终止。终止后仍可用 `builtin.job_output` 读取已产生的输出与终态。

并发收益是这一层独立出来的直接原因：`builtin.write_stdin` 声明为 `Exclusive`，需要独占执行窗口；作业工具是 `ParallelSafe`，因此含作业的批次仍可与其他只读工具并行。

执行批次按并发策略切成**连续子段**：`Exclusive` 请求自成一段，其余请求合并成可并发的一段，段内并发、段间严格保持模型给出的调用顺序。独占请求因此不会被并发带进执行窗口，也不会再把同批其他工具一并拖成串行。权限与并发是两个独立事实：`Allowed` 直接进入对应执行窗口，`Forbidden` 在执行前携带结构化原因拒绝；不存在第三种中间状态、Guardian 或人工等待分支。修改工具的并发策略或权限判定时，必须同时更新 `provider_tool_schedule` 的调用级判定分支，否则 `CallEvaluated` 会落到兜底分支被整体禁止。
