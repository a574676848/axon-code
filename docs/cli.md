# CLI 命令参考

命令行帮助是参数与可用子命令的权威来源：

```bash
axon --help
axon <COMMAND> --help
```

## 1. 任务交互与执行

```bash
# 启动终端全屏交互界面 (TUI)
axon
axon "解释当前工作区"

# 启动跨平台桌面工作台
axon desktop

# 非交互执行单次任务 (适合脚本与自动化流程)
axon run "为 API 添加错误处理"
axon run --format json "检查测试失败原因"

# 沿用当前工作区车道继续任务
axon run --continue "继续处理上一轮任务"

# 请求停止运行中的会话
axon stop
axon stop --session <SESSION_ID>
```

无子命令时，位置参数直接作为本轮 Personal Agent 请求执行，进入交互式 TUI。使用 `axon run` 可显式控制模型、会话、文件附件、工具白名单与结构化输出格式（Text 或 Json）。

## 2. 配置与环境初始化

```bash
axon setup                             # 交互式初始化向导
axon config --help                     # 查看配置项（get / set / validate / edit 等）
axon config validate                   # 校验当前 config.toml 合法性
axon providers                         # 列出已配置的 Provider 状态
axon models                            # 查询可用模型列表
axon auth --help                       # 管理认证状态与登录
axon doctor                            # 运行系统全链路健康检查
axon doctor --check runtime --fix      # 检测并自动修复运行态环境
```

`setup` 在本地验证并原子写入配置；手工修改 `config.toml` 或相邻 `.env` 后，后续命令执行时会自动重新加载。

## 3. 扩展与工具生态

```bash
axon mcp --help                        # 管理 MCP 服务器（list / get / add / remove / login / logout）
axon skills                            # 查看当前加载的技能列表
axon skills --summary                  # 查看已加载技能摘要
axon lsp --help                        # LSP 语言服务与代码诊断入口
```

工具调用受工具自身的 `Allowed` / `Forbidden` 二态执行权限、运行时安全边界和显式 `--tool` 白名单约束。图像生成与处理等高级能力由对应 Skill 或连接器按需提供，不占用 CLI 顶层子命令。

## 4. 后台常驻与守护服务 (Daemon)

```bash
# 系统服务管理（安装器已自动注册，服务损坏时可显式修复）
axon daemon install
axon daemon uninstall
axon daemon start
axon daemon stop
axon daemon status                     # 快速检查安装与运行状态
axon daemon restart

# 后台运维面
axon daemon health                     # 内部健康快照（租约、并发积压、订阅连接）
axon daemon drain                      # 排空待处理队列，安全停止服务
axon daemon run --root <AXON_ROOT> --config-path <CONFIG>
axon daemon attach <SESSION_ID>        # 重新挂接到指定会话事件流

# 独立前台 HTTP 网关
axon serve --host 127.0.0.1 --port 3000
```

Daemon 是 Axon 的常驻后台宿主，负责接收持久化 `Interaction`、排他调度车道执行、托管 Agent Cron 定时任务、维护记忆治理以及飞书长连接网关。客户端断开后，长周期 Goal 与 Execution 继续在后台执行。

## 5. 自动化、统计与度量

```bash
axon cron --help                       # 管理定时任务（list / create / update / pause / resume / run / remove）
axon usage --period 7d                 # 查询最近 7 天 Token 消耗与成本看板
axon usage audit                       # 审计日志流水查询
axon pricing --help                    # 模型计费规则与成本核算
axon remote --help                     # 飞书等远程协同通道治理与状态探测
```

## 6. 系统维护与运维诊断

```bash
axon upgrade                           # 检查并平滑升级至最新版本
axon package                           # 生成便携式 Release 发行包
axon export --output <PATH>            # 导出运行数据
axon completion bash                   # 生成 Shell 自动补全脚本 (支持 bash/zsh/fish/powershell)
axon exec <COMMAND...>                 # 执行本机受控命令

# 只读运维诊断面 (内部排障使用，用户无需手工管理 session_id)
axon debug session list
axon debug session show <SESSION_ID>
axon debug session read <THREAD_ID>
axon debug session doctor

# 人格系统：校验人设与查看持久化状态
axon persona validate [--file <PATH>]  # 校验 SOUL.md（或指定文件）是否符合 axon-soul/v1
axon persona status                    # 展示人格库、当前状态与已发布习得风格
axon persona status [--json]          # 展示人格库、当前状态、最近转移与已发布习得风格
axon persona style list [--all]        # 列出习得风格
axon persona style retire <STYLE_ID>   # 退役一条习得风格
axon persona reset --yes               # 把人格状态重置为基线，不动人设与习得风格
```

Axon 自行判断是否继续既有工作并按工作区车道自动管理上下文。`debug session` 仅作为系统诊断与数据恢复的底层接口。

人设与人格状态的字段、模式与体积预算见 [人格系统](./persona.md)；`axon doctor --check persona` 给出同一条判定。
