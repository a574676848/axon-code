# Axon Workbench Desktop (`axon-desktop`)

Axon 桌面工作台宿主（基于 Tauri 2），为 Axon 提供跨平台图形化操作外壳与高精度交互界面。

## 前置构建要求 (重要)

任何编译 `axon-desktop` 的 `cargo` 命令之前，**必须**先在 `crates/axon-desktop/ui` 完成前端依赖安装与静态产物构建：

```bash
cd crates/axon-desktop/ui
npm ci
npm run build
```

覆盖安装 Desktop 和 daemon 后，可只读验证真实网关的历史回放标记：

```bash
cargo test -p axon-desktop --test history_replay_probe -- --ignored
```

该探针要求本机网关可达且存在历史会话，不创建消息或修改会话数据；环境不满足时明确失败。

原因：`tauri-build` 在编译期会强依赖并打包 `ui/dist` 中的静态文件（由 `tauri.conf.json` 的 `frontendDist` 指定）。若产物缺失，Rust 编译将中断。

## 本地开发与构建

1. **前端热重载开发**：
   ```bash
   cd crates/axon-desktop/ui
   npm run dev
   ```

2. **生产构建**：
   ```bash
   cd crates/axon-desktop/ui
   npm run build
   cd ../../..
   cargo build -p axon-desktop --release
   ```

3. **IPC 契约自动化往返自检**：
   ```bash
   target/release/axon-desktop --self-check
   ```

## 跨平台架构与安全边界

- 严格遵循洋葱架构分层，表现层不依赖表现层；禁止直接依赖 `axon-cli`、`axon-daemon` 与 `axon-remote-feishu`。
- 配置读写复用 `axon-config` 权威内核，修改前自动备份，直接覆盖原配置文件，兼容编辑器允许读写但禁止替换文件的占用方式。凭据文件保持原子写入。
- 密钥安全隔离：IPC 传输与界面视图仅暴露环境变量名与掩码，严禁明文外泄。

## 会话与工具展示

- 会话切换后定位最新消息，窗口标题同步会话名称；主动上翻后暂停跟随，回到最新时同步消息导航。
- 思考展开、正文和工具输出增长时，处于底部的视口持续跟随。
- 工具前缀使用中文，MCP 保持原前缀；文件、便签、记忆、任务、子代理、上下文和历史操作按具体工具语义展示，搜索操作统一使用“检索”，终端交互显示“终端执行”，后台作业输出显示“作业输出”。Desktop 与 TUI 共用 `axon-display` 的内置工具名称表，并通过真实注册清单检查覆盖范围；未知工具保留原名。
- 工具行收起时只给一项事实，规则表与 TUI、飞书工具卡共用 `axon-display/src/tool_row_rules.json`：模型自述的 `intent` 优先（`shell_command`、`write_stdin`、`update_plan`、`glob`、`search` 与便签写入必填）；没有它时按工具语义取主参数（记忆检索的 `query`、技能的技能名、MCP 的服务名与工具名、子代理的 `task`、`send_message` 的 `message`、`job_output` 的等待作业、新建上下文的 `reason`）；`get_context_remaining` 收起时不追加内容；其余工具回落到文件路径、命令投影与结果摘要。原始参数、变更与输出仍在展开态完整可见。
- 展开的 JSON 输入输出使用缩进和语法高亮；Shell 命令单独高亮，输出保留原始换行。非正常退出以红色标识。
- 文件读取工具中的路径，以及正文和行内代码中的明确文件路径，可在右侧文件栏预览。性能分位数、I/O 和外部 URL 不作为本地文件。
- 历史索引与事件回放共同归并用户消息，附件由历史索引补齐，同一事件不会产生重复气泡。
- 历史回放不发送系统通知或新结果标识；实时轮次结束只提示一次，通知正文按订阅隔离。窗口处于前台时仅更新界面标识，后台时按通知设置发送系统通知。旧网关未提供回放标记时保守地跳过通知，需同步更新 daemon。
- 当前会话所属工作区显示主题色的展开文件夹，其他工作区显示灰色的闭合文件夹；列表展开状态独立保留。

## 配置操作与验证

配置修改复用后端类型校验，备份后直接覆盖并同步原文件。直接写入不保证原子性，写入时若发生进程中断或断电，可能需要从备份恢复。数值手输在失焦或按 Enter 后提交；写入期间禁用配置控件，失败后恢复已保存值，并在弹窗中保留后端具体错误。输入坞的模型与推理等级切换遵循相同的保存与错误反馈规则。

主模型、快速模型和对话模型选项排除 `supports_embedding = true` 的模型；向量检索设置只提供此类模型。日志等级只显示 `warn`，旧配置中的 `warning` 自动归一为 `warn`。网关未连接时统一使用遮罩弹窗，并在主区域常驻结论、失败原因与重试入口（弹窗被关掉后不留空白），侧栏不重复显示连接错误。

前端验证命令在 `ui` 目录运行：

```bash
npm test
npm run lint
npm run build
```
