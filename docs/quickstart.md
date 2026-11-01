# 快速开始

本指南将帮助你在几分钟内完成 Axon 的安装、配置与首次任务执行。

## 1. 安装

### 官方一键安装 (推荐)

**Windows (PowerShell)**:
```powershell
irm https://raw.githubusercontent.com/a574676848/axon-code/master/scripts/install.ps1 | iex
```

**macOS / Linux**:
```bash
curl -fsSL https://raw.githubusercontent.com/a574676848/axon-code/master/scripts/install.sh | sh
```

安装程序会自动完成：
1. 部署不可变二进制版本树至 `~/.axon/bin`；
2. 配置稳定 Launcher 并加入系统 PATH；
3. 安装并自动启动当前用户级 Daemon 守护进程服务（Windows 注册任务计划 `AxonDaemon`，支持开机常驻与崩溃自愈）；
4. 初始化默认配置文件与 Soul 人格文件（已存在时不覆盖用户配置）。

### 从源码构建安装

适用于本地二次开发或定制构建：

**Windows (PowerShell)**:
```powershell
.\scripts\install-from-source.ps1
axon --version
```

**macOS / Linux**:
```bash
./scripts/install-from-source.sh
axon --version
```

源码安装会严格编译 release 产物，构建前端并替换当前版本目录，自动重启守护进程完成生效。

#### 按组件安装

`scripts/install-from-source.ps1` 用 `-Components` 选择装哪一侧，取值为 `backend`、`desktop`、`all`（可逗号分隔），缺省等价 `all`：

```powershell
# 只装后端：注册并启动 Daemon，不铺桌面二进制与快捷方式
.\scripts\install-from-source.ps1 -Components backend

# 只装桌面端：不注册 Daemon、不写 config.toml、不启动后台服务
.\scripts\install-from-source.ps1 -Components desktop
```

`-BinariesOnly` 已移除，请改用 `-Components backend`（它原本的语义是「跳过提权与服务注册，但仍然启动 Daemon 并断言运行」，与只装一侧并不等价）。

稳定 launcher 与 `current.json` 在任何组件组合下都会安装：它们是桌面入口与版本切换的硬前置。

#### 只装桌面端之后

没有后端时，首次启动会进入四步引导：选模式（本机安装 / 连接云电脑）、本机探活或云端试连接、写入连接档案并切换。引导可跳过，跳过后面板处于未连接态，设置 › 连接里随时可以补。引导期间整个窗口只显示引导本身，背后的侧栏与主界面不渲染，也不响应快捷键。

## 2. 初始化配置

安装完成后，运行交互式配置向导：

```bash
axon setup
```

向导将引导你完成：
- 选择默认大模型 Provider（OpenAI、Anthropic、DeepSeek、Ollama 等）；
- 配置 API 密钥（安全隔离保存在 `~/.axon/.env` 中）；
- 自动生成网关通信 Token，用于桌面端与远程协同鉴权。

详细配置选项请参阅 [配置指南](./configuration.md)。

## 3. 多种形态体验

### 方式 A：启动图形化桌面工作台

体验可视化多工作区、任务车道与交互式问答：

```bash
axon desktop
```

首次打开时，桌面端按三种情形决定落点：后端配置文件存在且网关可达，直接建出本机档案进主界面；配置存在但网关不可达，仍建本机档案并进主界面，连接态显示未连接（可「启动后台服务」或「重试连接」）；配置缺失则进入四步引导。

详见 [Axon Desktop 指南](./desktop.md)。

### 方式 B：终端全屏交互 (TUI)

在沉浸式终端界面中与 AI 协同编程：

```bash
axon "分析当前仓库的核心架构与主要依赖"
```

详见 [终端 TUI 说明](./tui.md)。

### 方式 C：无头命令行 (Headless Run)

适合快速单次执行、脚本自动化与 CI/CD 流水线：

```bash
axon run "检查 Cargo.toml 依赖并报告潜在安全风险"
```

## 4. 任务接续与系统自检

```bash
# 沿用上一轮会话继续执行
axon run --continue "继续完成剩余测试用例"

# 运行系统全链路健康检查
axon doctor
```

完整命令及参数请参阅 [CLI 参考手册](./cli.md)。
