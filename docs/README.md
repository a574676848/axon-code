# Axon 文档中心

Axon 是基于 Rust 构建的专属个人智能体（Personal Agent）与战术协同平台，**知你所想，办你所托**（*Knowing Your Intent, Delivering Your Outcome*），融入你的工作区与工作节奏，在每一次交互中持续深化认知。系统严格遵循洋葱架构，在桌面端（Desktop Workbench）、终端交互（Terminal TUI）、无头命令行（Headless CLI）与企业协同（飞书远程接入）之间复用统一的 Agent 运行时、状态机模型、模型路由与上下文认知体系。

本页是全部文档的索引。每篇文档只讨论一个主题，互相之间保持链接，便于按需跳转。

## 快速导航

按当前目标选择入口：

| 我想… | 先读 |
|---|---|
| 了解 Axon 是什么、如何运转 | [系统概览](./overview.md) → [核心概念](./concepts.md) |
| 尽快装好并跑通第一个任务 | [快速开始](./quickstart.md) |
| 接入自己的模型供应商 | [配置参考指南](./configuration.md) → [Provider 与模型路由](./providers.md) |
| 用桌面端或终端界面工作 | [Axon Desktop 桌面工作台](./desktop.md) → [终端 TUI 交互](./tui.md) |
| 扩展工具、Skill 与 MCP 能力 | [内置工具契约](./tool-schema.md) → [Skill 技能系统](./skills.md) → [MCP 协议集成](./mcp.md) |
| 管理记忆与长周期认知 | [记忆管理系统](./memory-management.md) |
| 部署、排障与核算成本 | [Daemon 后台守护进程](./daemon.md) → [故障诊断与自愈](./troubleshooting.md) → [使用统计与用量看板](./usage-statistics.md) |
| 修改桌面端界面 | [桌面端 UI 约束规范](./ui-constraints.md) → [工作台设计系统](./workbench-design-system.md) |
| 参与源码开发 | [开源贡献指南](../CONTRIBUTING.md) |

## 1. 概念与架构

理解系统的核心思想、执行拓扑与状态机设计：

- [系统概览](./overview.md) - 系统全景架构、端到端执行流与核心运行机制
- [核心概念](./concepts.md) - Interaction、Goal、Execution、Session、上下文分层与工具权限
- [记忆管理系统](./memory-management.md) - 分层认知模型、五类持久化、检索与衰减、生命周期与管理接口

## 2. 界面与使用指南

针对不同工作场景的完整操作与配置指南：

- [快速开始](./quickstart.md) - 一键安装、源码构建、配置向导与首次任务执行
- [Axon Desktop 桌面工作台](./desktop.md) - 基于 Tauri 2 的图形化多工作区工作台完全手册
- [终端 TUI 交互](./tui.md) - Ratatui 全屏终端操作、仪表盘读数与按键绑定
- [CLI 命令参考](./cli.md) - 完整命令行接口、参数选项与无头自动化运行
- [配置参考指南](./configuration.md) - `config.toml` 配置项深度解析与环境约定

## 3. 模型与后台服务

大模型接入治理与后台守护进程：

- [Provider 与模型路由](./providers.md) - 多供应商接入、负载均衡、容灾 Failover 与 Prompt Cache
- [Daemon 后台守护进程](./daemon.md) - 网关通信、后台任务执行池、服务管理与原子升级
- [飞书远程协同](./feishu-remote.md) - 私有长连接远控、移动端交互卡片与群聊协作

## 4. 扩展生态与工具

扩展 Axon 的能力边界：

- [MCP 协议集成](./mcp.md) - Model Context Protocol 标准支持、服务器注册与 OAuth 授权
- [Skill 技能系统](./skills.md) - 动态工作流加载、自进化机制与 `SKILL.md` 编写规范
- [内置工具契约](./tool-schema.md) - 文件、Shell、编辑、记忆与交互问答等工具 Schema

## 5. 运维诊断与开发者中心

生产环境观测与源码开发：

- [使用统计与用量看板](./usage-statistics.md) - Token 消耗度量、审计日志与成本核算体系
- [故障诊断与自愈](./troubleshooting.md) - 健康检查 `axon doctor`、常见异常与修复手段
- [开源贡献指南](../CONTRIBUTING.md) - 代码结构、分层边界、门禁要求与贡献流程
- [工作台设计系统](./workbench-design-system.md) - 跨平台桌面端极简浅色设计规范与令牌 SSOT
- [桌面端 UI 约束规范](./ui-constraints.md) - UI 任务开工前置契约、令牌与组件约束、验收门禁与设计终检清单

## 6. 示例与配置样板

仓库根目录提供可直接参照的配置与接入样例：

- [`config.example.toml`](../config.example.toml) - 带逐字段注释的完整配置样板，对应用户级配置文件 `~/.axon/config.toml`
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

---

详细命令参数请随时运行 `axon --help` 或 `axon <COMMAND> --help` 查看。文档改动需通过 `pwsh -File tools/verify-docs.ps1` 的链接与术语门禁。
