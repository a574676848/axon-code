# v0.0.1 真机验收记录（进行中）

> 验证对象：安装态版本树 `$HOME/.axon/bin/versions/0.0.1/`，与 `target/release/` 逐字节核对 SHA-256 一致。
> 隔离方式：运行态根指向临时目录 `$env:TEMP/axon-verify-home/.axon`，配置与 `.env` 是副本，真实 `$HOME/.axon/config.toml` 前后 SHA-256 不变（`2208FC66…6646`）。
> UAC 在本环境不可确认，Daemon 用电装脚本的 `desktop` 组件安装后手工拉起（`daemon run --root <临时根> --config-path <副本>`），IPC 端点 `axon-daemon`。
> 判据：只看输出文本，不看退出码；未取得证据的条目一律标注「未真机验证」。

## 已完成取证的条目

| # | 检查 | 结果 | 证据 |
| --- | --- | --- | --- |
| B1 | `axon --version` | 通过 | 输出 `axon-cli 0.0.1` |
| B2 | `Get-Command axon` | 通过 | 解析到 `C:\Users\<用户>\.axon\bin\axon.cmd`，不是 cargo bin |
| B3 | `axon doctor` | 通过 | `Axon doctor: ok`，config/cli/preflight/provider/tool/mcp/memory/persona/lsp/smoke 全项 ok |
| B4 | `data/` 布局 | 通过 | 临时根 `data/` 下有 `control.sqlite3`、`outbox.sqlite3`；无 `runtime.sqlite3`、无 `data/rollout.sqlite3`；`data/sessions/rollout.sqlite3` 仍在，属 §12.5 已登记延后项 |
| B5 | 单回合写入 | 通过 | 生成 `data/events/<session>/session.v1.jsonl.zstd`（5145 字节）；`daemon attach` 回放出 10 条事件，序号 1..10 连续，`runtime.lastEventSequence=10` 与末条一致；帧可解压，含 `turn_metrics`（ttft 2785ms、active 5036ms）与 `turn_completed` |
| C1 | `axon sessions list` | 通过 | 输出 `session-session-<hash>  <首条内容>  last=…  turns=N` |
| C3 | `--session` 指向不存在的 id | 通过 | `error: 会话不存在：session-does-not-exist。先用 axon sessions list 查看可用会话；--session 不会隐式新建会话` |
| C6 | `--session` 与 resume 指针冲突 | 未真机验证 | 依赖 C5 的 resume 语义先成立，当前被 C5 阻塞 |

## 未通过 / 阻塞的条目

| # | 检查 | 结果 | 证据与原因 |
| --- | --- | --- | --- |
| C2 | `axon run "…" --session <id>` 追加到指定会话 | **不通过** | 用 `sessions list` 里的 `session-session-<hash>` 与 `sessions list` 里的 run/thread id（如 `04397d85-…`）各试一次，两次都**新建了另一个会话**（`session-session-533e9d…`、`session-session-612e36…`），原会话 `turns` 不增 |
| C5 | `sessions resume <id>` 后 `run --continue` | **不通过** | `sessions resume session-session-<hash>` 报 `未找到 thread`；改用 run id `04397d85-…` 可 `已恢复 Thread`，但随后 `run --continue` 仍然新建会话，且模型回复明确说明「没有前四个字的上下文」——历史未被带入 |
| C4 | 不带 `--session`/`--continue` 复用当前 cwd 车道 | **不通过（同一根因）** | 同一 cwd 连续两次 `run` 落在两个不同 `session-session-*` 上，上下文不连续 |

根因方向：`--session` / `--continue` 解析的标识空间与 `sessions list` 展示的标识空间不一致，且历史窗口仍由 `data/sessions/rollout.sqlite3` 提供，恢复链在安装态下没有真正接上。这与 W2-12（rollout 退役）未完成直接相关，必须在退役改造里一并闭合。

## 未开始取证的条目

- D1–D12（桌面端）：需要以 `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS='--remote-debugging-port=9333'` 启动并经 `tools/cdp-*.ps1` 取证，尚未执行。
- T1–T7（TUI）：本环境无交互终端，尚未执行。
- X1–X7（跨端）：依赖 C2/C5 的续聊语义先成立，当前被阻塞。
- B6（daemon 重启 3 次）、B7（24 小时运行）：未执行。

## 环境副作用登记

- 装脚本 `desktop` 组件路径不写 `config.toml`、不注册系统服务；Daemon 为手工前台进程，验证结束后已终止。
- 首次尝试 `daemon start` 时计划任务以真实运行态根拉起过一次，在 `$HOME/.axon/data` 留下 `control.sqlite3*`、`outbox.sqlite3*`、`daemon.pid`（均为新建空库，未改动既有数据）；已在本轮结束前清理。
- 临时验证根 `$env:TEMP/axon-verify-home` 保留，供下一轮继续取证。
