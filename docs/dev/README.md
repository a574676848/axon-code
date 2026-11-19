# 开发者与维护者文档

本目录是 `docs/dev/`，存放开发者与维护者文档。它**不属于产品说明**，也不出现在产品文档索引里。

两条规矩：

1. 产品文档（`docs/*.md`）只讲用户可见行为，不写表名、库名、代码路径、内部模块名、调优常量与实现演进史。
2. 技术设计、内部契约、测绘记录与重构计划放在这里；两份文档冲突时，产品行为以 `docs/` 为准，实现细节以本目录为准。

改动本目录后同样要跑 `pwsh -File tools/verify-docs.ps1`：坏链检查覆盖本目录。

## 存储与重构

| 文档 | 内容 |
| --- | --- |
| [存储架构重构方案](./storage-architecture.md) | 事件面、控制面与投影面的目标架构、影响面、已裁决事项与安装后验收矩阵 |
| [存储层现状测绘](./storage-map.md) | 全量存储单元、表、写入路径与依赖方向的取证记录（重构附件） |
| [存储重构 WBS](./reconstruction-wbs.md) | 阶段与任务拆解、依赖关系、里程碑与延后项 |
| [会话身份重构计划书](./session-identity-refactor-plan.md) | 会话标识唯一化方案、现状飘逸清单、会话数据删除与表结构重建步骤、WBS 与性能红线 |
| [会话身份重构开工提示词](./session-identity-refactor-kickoff.md) | 实施会话的首条指令：任务、工具纪律、事实底座、执行顺序、门禁与验收口径 |
| [单渲染路径后端改造提示词](./single-render-path-backend-prompt.md) | 让三条会话入口共用一条渲染路径所需的后端契约：能力位、派发轮终态、逐轮指标归属、轮次头订阅与入口事实 |
| [WBS 验收台账](./wbs-acceptance.md) | 逐条登记 WBS 各项目的状态、判据与可复核证据 |
| [混合审计发现清单](./audit-findings.md) | 交叉审计的性能、闭环与稳定性发现及处置结论 |
| [v0.0.1 真机验收记录](./device-verification.md) | 安装态二进制的 39 项验收矩阵逐项结果、证据与环境副作用登记（未取证项已标注） |

## 运行时内部实现

| 文档 | 内容 |
| --- | --- |
| [Daemon 内部实现](./daemon-internals.md) | 领取与并发模型、事件入口细节、投影时序、启动快照、容量基线与资源遥测 |
| [桌面客户端内部实现](./desktop-client-internals.md) | 通信时序、连接档案解析、能力位、帧页与回放机制 |
| [TUI 内部实现](./tui-internals.md) | 指标口径、订阅恢复判据、投影与背压、工具行规则 |
| [上下文消息契约](./context-message-contract.md) | `source`、`kind`、`role` 三维标记、构造器矩阵、装配顺序与换窗保留边界 |
| [上下文装配事实核对](./context-assembly-facts.md) | 上下文装配的基线核对记录 |
| [记忆与反思子系统重构设计](./memory-reflection-redesign.md) | 反思链路的现状缺陷、三条线架构、索引卡与攒批、迁移分期 |
| [Turn 生命周期加固](./turn-lifecycle-hardening.md) | 轮次卡死与执行权回收的收敛目标与实施计划 |
| [使用统计](./usage-statistics.md) | 统计投影的数据流、存储模型、调度机制与成本定价边界 |
| [语音协议与接入](./voice-protocol.md) | 语音组件边界、WebSocket 帧协议、二进制布局、超时常量与提供方接入落点 |
| [子进程资源围栏](./process-fences.md) | 资源围栏的取值原则、平台落点与已知限制 |
| [桌面客户端 TLS 信任](./tls-trust.md) | 平台根证书在两条链路上的接线落点 |

## 命令行与诊断

| 文档 | 内容 |
| --- | --- |
| [CLI 诊断命令](./cli-debug.md) | `axon debug session` 系列只读诊断命令与标识约定 |

## 界面规范

| 文档 | 内容 |
| --- | --- |
| [Axon Desktop UI 约束规范](./ui-constraints.md) | UI 任务的开工前置契约、令牌与组件约束、验收门禁与设计终检清单 |
| [Axon 工作台设计系统](./workbench-design-system.md) | 跨平台桌面端的视觉与交互设计事实源：调色板、字号、圆角、组件形态与动效 |

## 相关入口

- [产品文档索引](../README.md)
- [开源贡献指南](../../CONTRIBUTING.md)
