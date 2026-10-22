# Skills

Skill 是 Axon 的可复用工作流，将复杂任务封装为简单的调用入口。

## 核心特性

- **统一入口**：通过 `builtin.invoke_skill` 调用，不扩展模型工具 schema。
- **官方合同**：每个 Skill 使用独立目录；SKILL.md frontmatter 至少包含字符串 name、description，可携带额外 YAML 字段。
- **确定性来源**：只扫描 user、workspace 两个受管根目录。
- **不可变引用**：加载正文时计算 SHA-256 digest，运行时按精确官方名称解析。
- **相关性路由**：当前请求从全部可调用 Skill 中生成有界推荐，推荐 fragment 不进入 prompt cache。

## Skill 与 MCP 的区别

| 对比项 | Skill | MCP |
|--------|-------|-----|
| 定位 | 工作流模板 | 外部工具集成 |
| 格式 | Markdown 文件 | JSON-RPC 协议 |
| 复杂度 | 简单 | 中等 |
| 适用场景 | 多步骤任务自动化 | 访问外部系统 |

## 快速开始

### 查看可用 Skills

Skills 按以下优先级加载，同名 Skill 由更高优先级覆盖：

1. `~/.axon/skills/` — 用户级 Axon Skills
2. `<workspace>/.axon/skills/` — Workspace 级 Axon Skills

同名 Skill 只加载 User 版本，不重复加载 Workspace 版本。`docs/skills/` 用于随发行包提供文档与示例，不是运行时搜索路径。

仓库随发行包内置的官方 Skill 统一维护在 `docs/skills/`。

### 调用 Skill

TUI 中通过 `$skill` mention 引用：

```
$review 检查 src/main.rs
```

在 Composer 输入区输入 `$` 会搜索可用 Skill，选中后带回输入区，后面可以继续补充用户要求再按 Enter 发送。`/skills` 只用于查看列表、启用或禁用 Skill，不用于按名称筛选并发起对话。

`skill://` 是运行时资源引用，主要用于记录和内部协议；TUI 用户交互应使用 `$` 选择 Skill，`@` 保留给当前项目文件引用。

### 模型注入语义

Skill 选中后不会替换或包裹用户原始请求。Axon 把 available skills 摘要作为 session-stable 后缀固化到 prompt，把 selected skill 正文作为 dynamic / ephemeral user fragment 按当前请求追加。模型可以为互不重叠的阶段分别调用 Skill，例如先用架构分析 Skill 再用文档 Skill；同一阶段只选择一个最匹配 Skill，不组合会重复执行相同工具链的 Skill。`builtin.invoke_skill` 仍是唯一执行入口。

### 调用参数契约

`builtin.invoke_skill` 只接收 `skill_name`。它只把该 Skill 的操作说明加载进当前上下文，不会代替模型执行任务；模型读完说明后，必须按说明调用真正的工具完成用户请求。任务参数属于后续工具调用，不得塞进 `invoke_skill`。

```json
{
    "skill_name": "review"
}
```

## Skill 文件格式

```text
~/.axon/skills/
└── review/
    ├── SKILL.md          # Skill 主文件
    └── scripts/
        └── check.sh      # 辅助脚本
```

### Frontmatter 格式

```markdown
---
name: review
description: Review code changes before merge
---

# 代码审查

执行步骤...
```

## 编写 Skill 最佳实践

- 描述前置：在 frontmatter 的 `description` 中清晰说明 Skill 的目标与触发场景。
- 步骤化正文：使用编号列表或 markdown 标题表达阶段。
- 名称稳定：`name` 使用小写 ASCII 字母、数字和单连字符，调用时精确匹配。
- 能力显式：正文说明所需工具和执行步骤；额外 frontmatter 仅作为兼容元数据，不产生 Axon 权限或执行语义。

## 调试 Skill

```bash
# 查看 Skill 列表
axon skills

# 查看 Skill 摘要
axon skills --summary

# 以 JSON 格式输出
axon skills --format json
```

## 相关文档

- [核心概念](./concepts.md)
- [MCP 集成](./mcp.md)
- [CLI 参考](./cli.md#扩展与工具)
