# 贡献指南

感谢你关注并参与 Axon 的开源建设。Axon 是基于 Rust 构建的专属个人智能体（Personal Agent）与多端战术协同平台。

---

## 1. 项目架构与 Crate 布局

Axon 是基于 Rust 2024 Edition 构建的 Cargo Workspace，包含 36 个 package，全部以 `axon-*` 命名。系统严格遵循**洋葱架构（Onion Architecture）**设计范式；核心领域层 `axon-domain` 不依赖 CLI、TUI、Provider、数据库、异步 Runtime 或文件系统等基础设施实现，边界由 Cargo manifest 与架构守卫共同约束。

```text
crates/
├── axon-cli/                  # CLI 命令行入口、子命令解析与多端编排
├── axon-desktop/              # 基于 Tauri 2 的跨平台桌面应用工作台
├── axon-tui-runtime/          # 基于 Ratatui 的沉浸式全屏终端交互界面
├── axon-daemon/               # 后台常驻守护进程、网关通信与会话排他调度池
├── axon-launcher/             # 稳定 Launcher 入口与零中断平滑升级治理
├── axon-agent-runtime/        # Agent 核心循环、工具调用与子代理委派
├── axon-session-runtime/      # Session 运行时引擎与状态分发
├── axon-session/              # Interaction、Goal、Turn 持久化与领域服务
├── axon-domain/               # 核心领域模型、运行时契约与错误定义 (洋葱内层)
├── axon-config/               # 配置 Schema、加载、校验与 Setup 事务
├── axon-memory/               # Memory 权威 SQLite 存储、Schema 与索引基座
├── axon-memory-read/          # Memory 召回、检索投影与读取信号
├── axon-memory-write/         # Memory 准入、显式写入与 Pipeline 协调
├── axon-memory-ops/           # Memory 治理原语、维护任务与自愈流水线
├── axon-prompts/              # 系统提示词模板与装配
├── axon-context-fragments/    # 动态上下文 Fragment 投影
├── axon-persona/              # 人格内核解析、状态机、表达策略与人格库
├── axon-protocol/             # 跨 Crate 共享协议与 Wire DTO 契约
├── axon-patch-model/          # 补丁数据模型与纯计算（无 I/O，零依赖）
├── axon-apply/                # 文件编辑应用执行器与安全审计
├── axon-output-filter/        # 工具输出过滤与 Token 节省优化
├── axon-provider-transport/   # Provider HTTP / WebSocket Wire 通信与流式处理
├── axon-rustls-provider/      # 进程级 Rustls TLS 证书与 Provider
├── axon-mcp-runtime/          # 外部 MCP 客户端适配与运行时工具动态注册
├── axon-remote-feishu/        # 飞书远控适配器（OpenAPI、WebSocket、卡片交互）
├── axon-process/              # 非交互子进程的平台隔离与管道保护
├── axon-fs-walk/              # 高性能文件遍历基础设施
├── axon-crypto/               # 加密、哈希与签名设施
├── axon-display/              # 终端与前端共享的显示格式化单元
├── axon-time/                 # 时间戳与时钟抽象
├── axon-fixtures/             # 测试 Fixture 与确定性样例辅助
└── axon-test-support/         # 测试临时产物隔离设施（仅 dev-dependencies）
```

---

## 2. 核心架构设计原则

1. **洋葱架构 (Onion Architecture)**：领域模型位于洋葱最内层，严禁依赖外部框架、数据库或 UI。
2. **SOLID 原则**：单一职责、开闭原则、里氏替换、接口隔离与依赖倒置。
3. **二态权限裁决**：所有工具调用在执行前统一裁决，仅产生 `Allowed`（直接放行并审计）与 `Forbidden`（携带结构化错误拒绝）。
4. **单写边界**：后台守护进程独占主执行流与持久状态写入；桌面端与前端仅做增量状态投影与指令派发。
5. **路径无关性 (Path-Agnostic)**：**严禁**在源码、脚本、配置、文档与命令示例中写死任何本机绝对路径（用户目录、盘符、某台机器特有路径）。一律使用相对路径或环境变量（`$HOME` / `%USERPROFILE%`）表达。

---

## 3. 本地开发与质量门禁

提交代码或发起 Pull Request 之前，必须在本地完整跑通以下质量校验链：

| 校验项 | 命令 | 说明 |
|---|---|---|
| **代码格式** | `cargo fmt --all --check` | 验证 Rust 源码风格 |
| **编译检查** | `cargo check --workspace --all-targets` | 全平台源码及测试目标编译通过 |
| **静态分析** | `cargo clippy --workspace --all-targets -- -D warnings` | 全工作区零警告 |
| **单元与集成测试** | `cargo test --workspace` | 全模块自动化功能测试 |
| **架构合规性** | `pwsh -File tools/verify-architecture.ps1` | 校验洋葱分层、命名一致性、单文件 ≤ 700 行 |
| **文档与链接** | `pwsh -File tools/verify-docs.ps1` | 校验全仓 Markdown 零坏链、UTF-8 without BOM |
| **桌面 UI 契约** | `pwsh -File tools/verify-ui-contract.ps1` | 校验无破折号、无全局滚动监听、动效与令牌合规 |
| **编译期 lint 豁免** | `python tools/check_no_lint_allows.py` | 未登记的 `#[allow(...)]` 与 `#![allow(...)]` 一律失败，白名单见 `tools/lint-allow-allowlist.txt` |

### 桌面端开发与构建（`crates/axon-desktop/ui`）

桌面端构建必须遵守前端先于后端的构建时序：

```bash
# 1. 切换至前端工程目录
cd crates/axon-desktop/ui

# 2. 依赖还原
npm ci

# 3. 运行代码检查与测试
npm run lint
npm test

# 4. 构建前端生产产物 (输出至 dist)
npm run build

# 5. 回到仓库根目录编译桌面二进制
cd ../../..
cargo build -p axon-desktop --release --locked
```

---

## 4. 测试隔离规范

`cargo test` 执行后工作区必须保持 100% 干净，测试不得在仓库或系统中遗留未回收的数据：

- **临时路径一律走 `axon-test-support`**：使用 `axon_test_support::test_temp_root()` 获取当前测试线程独占的隔离根目录，用完自动安全回收。
- **配置与运行态隔离**：测试中的运行态数据根必须指向临时目录，严禁指向仓库内路径。
- **连接池显式关闭**：持有 SQLite 连接池句柄的测试必须在删除临时目录前显式关闭连接，防止 Windows 平台文件锁定导致删除失败。

---

## 5. 提交与 PR 规范

1. **Commit 信息**：使用清晰的中文提交说明，直击核心改动要点。
2. **敏感信息自检**：严禁提交包含真实 API 密钥、Token、私钥证书或未脱敏配置的任何文件。
3. **相关文档同步**：核心逻辑、命令或 API 变更时，必须同步更新相关文档及 `docs/skills/operate-axon/`。

---

## 6. 社区协作与评审

- **缺陷与建议**：通过仓库 Issue 模板提交。先搜索既有 Issue，按模板补齐版本、受影响入口与复现步骤。
- **安全漏洞**：不要开公开 Issue，按 [安全策略](./SECURITY.md) 的私密渠道报告。
- **Pull Request**：按 PR 模板填写变更说明、影响面与验证证据；门禁未全绿的 PR 不进入评审。
- **评审期望**：围绕正确性、边界条件、跨平台影响与文档一致性给结论；作者负责在合并前消除未决项。
- **协作约定**：所有项目空间适用 [行为准则](./CODE_OF_CONDUCT.md)。

---

## 7. 分支模型与合并（trunk-based）

`master` 是唯一长期分支，任何时刻都应处于可发布状态。

| 分支 | 用途 | 生命周期 |
|---|---|---|
| `master` | 唯一集成分支，受保护 | 永久 |
| `feat/<slug>` | 新功能 | 短命，建议不超过 5 天 |
| `fix/<slug>` | 缺陷修复 | 短命 |
| `chore/<slug>`、`docs/<slug>`、`deps/<slug>` | 工程杂务、文档、依赖升级 | 短命 |

- 所有分支从 `master` 切出、合回 `master`，合并后立即删除，仓库长期只保留 `master`。
- `master` 受分支保护：必须走 PR，且三项必需检查（`Check (ubuntu-latest)`、`Check (windows-latest)`、`Check (macos-latest)`）全绿。
- 管理员账号当前保留绕过权限，只用于构建口径类紧急修复；常规改动一律走 PR。
- 禁止强制推送 `master`；推送被拒时先 `pull --rebase` 再重试。

## 8. 提交信息规范

squash 合并时 PR 标题即最终提交信息：

```text
<type>(<scope>): <中文摘要>

<正文：为什么改、影响面、验证方式>
```

- `type` 取值：`feat`、`fix`、`refactor`、`perf`、`docs`、`test`、`build`、`ci`、`chore`、`revert`。
- `scope` 取值：crate 名或 `ci`、`docs`、`deps`，例如 `feat(desktop)`、`fix(agent-runtime)`。
- 摘要不超过 50 字；全文不使用 en/em 破折号。
- 破坏性变更在正文首行写 `BREAKING CHANGE:` 并说明迁移方式。

## 9. 发布流程

1. 版本号遵循语义化版本；0.x 阶段 MINOR 承载功能、PATCH 承载修复。
2. 在 `master` 上打 annotated tag `vX.Y.Z`，由 `.github/workflows/release.yml` 出包。
3. tag 只允许打在 `master` 上；指向其它分支的 tag 不进入发布。
4. `CHANGELOG.md` 按 Keep a Changelog 维护，发布前补齐当次版本段落。

## 10. 依赖升级

- Dependabot 每周提一次，patch 与 minor 分组，单生态待处理上限 3。
- 大版本不自动提，需维护者显式发起，并在 PR 中说明影响面与三平台检查结果。
- 只接受稳定版；`alpha`、`beta`、`rc`、`canary`、`next` 一律不引入。
- 改动 `Cargo.lock` 的 PR 必须核对三平台检查结果。
- 功能 PR 不得夹带依赖升级。
