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
