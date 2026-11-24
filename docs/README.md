# Axon 文档中心

Axon 是基于 Rust 构建的专属助理（Personal Agent）与战术协同平台，**知你所想，办你所托**（*Knowing Your Intent, Delivering Your Outcome*）。它在桌面工作台、终端界面与命令行之间共享同一套会话、记忆与技能，在每一次交互中持续深化认知。

本页只索引**产品文档**：讲用户能看到、能操作、能配置的行为。开发者与维护者文档见文末的开发者入口。

## 快速导航

按当前目标选择入口：

| 我想… | 先读 |
| --- | --- |
| 了解 Axon 是什么、能做什么 | [系统概览](./overview.md) → [核心概念](./concepts.md) |
| 尽快装好并跑通第一个任务 | [快速开始](./quickstart.md) |
| 接入自己的模型供应商 | [配置参考指南](./configuration.md) → [Provider 与模型路由](./providers.md) |
| 用桌面端或终端界面工作 | [Axon Desktop 桌面工作台](./desktop.md) → [终端 TUI 交互](./tui.md) |
| 扩展工具、技能与 MCP 能力 | [内置工具契约](./tool-schema.md) → [Skill 技能系统](./skills.md) → [MCP 协议集成](./mcp.md) |
| 管理记忆与长周期认知 | [记忆管理系统](./memory-management.md) |
| 调整 Axon 的人格与表达 | [人格系统](./persona.md) |
| 后台常驻、排障与安全边界 | [Daemon 后台运行](./daemon.md) → [故障诊断与自愈](./troubleshooting.md) → [安全基线](./security-baseline.md) |
| 使用语音输入与朗读 | [语音能力](./voice.md) |
| 查看用量、成本与费率 | [使用统计与成本](./usage.md) |

## 1. 概念与使用

- [系统概览](./overview.md) - 产品定位、三种形态、一次请求的全过程与能力边界
- [核心概念](./concepts.md) - 会话、轮次、交互、后台工作与工作区
- [快速开始](./quickstart.md) - 安装、配置向导与首次任务
- [Axon Desktop 桌面工作台](./desktop.md) - 图形化多会话工作台操作手册
- [终端 TUI 交互](./tui.md) - 全屏终端操作、状态读数与按键绑定
- [CLI 命令参考](./cli.md) - 命令、参数与无头自动化运行

## 2. 配置与模型

- [配置参考指南](./configuration.md) - `config.toml` 里你可以改的键与环境约定
- [Provider 与模型路由](./providers.md) - 多供应商接入、路由与故障切换
- [人格系统](./persona.md) - 人设格式、模式推导与表达调整

## 3. 后台服务与安全

- [Daemon 后台运行](./daemon.md) - 服务管理、外部事件接入、健康检查、数据保留与升级
- [安全基线](./security-baseline.md) - 连接形态、传输信任、凭据与访问边界
- [故障诊断与自愈](./troubleshooting.md) - 常见症状、判定依据与可执行动作

## 4. 扩展生态

- [MCP 协议集成](./mcp.md) - MCP 服务器注册、工具发现与 OAuth 授权
- [Skill 技能系统](./skills.md) - 技能结构、装载规则与编写要点
- [内置工具契约](./tool-schema.md) - 内置工具的参数、回执与展示契约

## 5. 记忆与语音

- [记忆管理系统](./memory-management.md) - 记忆分类、准入、替代、归档与自愈
- [语音能力](./voice.md) - 语音输入与朗读的当前状态、状态提示与错误码

## 6. 示例与配置样板

仓库根目录提供可直接参照的配置与接入样例：

- [`config.example.toml`](../config.example.toml) - 带逐字段注释的完整配置样板，对应用户级配置文件 `$HOME/.axon/config.toml`
- [`.env.example`](../.env.example) - 环境变量与 API 密钥模板
- [`mcp_stdio.example.json`](../mcp_stdio.example.json) - MCP stdio 服务器注册样例
- [`mcp_streamable_http.example.json`](../mcp_streamable_http.example.json) - MCP streamable HTTP 服务器注册样例
- [`installer/windows/README.md`](../installer/windows/README.md) - Windows MSI 构建与安装说明
- [`homebrew/axon.rb`](../homebrew/axon.rb) - Homebrew formula

## 7. 变更记录与许可证

- [CHANGELOG](../CHANGELOG.md) - 版本变更与发行说明
- [Releases](https://github.com/a574676848/axon-code/releases) - 发行包与更新日志
- [MIT 许可证](../LICENSE) - 项目开源许可条款
- [返回项目首页](../README.md) - 项目定位、核心能力与快速开始

## 8. 社区与治理

- [安全策略](../SECURITY.md) - 漏洞报告的私密渠道、支持范围与处理流程
- [行为准则](../CODE_OF_CONDUCT.md) - 社区协作的期望行为与执行方式
- [贡献指南](../CONTRIBUTING.md) - 开发流程、门禁要求与提交规范
- [Issue 入口](https://github.com/a574676848/axon-code/issues/new/choose) - 缺陷报告与功能建议的结构化模板

## 9. 开发者入口

界面设计规范属开发者文档，不属产品说明，单独维护在 [界面设计规范](./design/README.md)。

---

详细命令参数请随时运行 `axon --help` 或 `axon <COMMAND> --help` 查看。文档改动需通过 `pwsh -File tools/verify-docs.ps1` 的链接、术语与产品文档清洁度门禁。
