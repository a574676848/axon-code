# v0.0.1 真机验收记录（进行中）

> 验证对象：安装态版本树 `$HOME/.axon/bin/versions/0.0.1/`，与 `target/release/` 逐字节核对一致：
> `axon.exe` = `228445500E35A4AC5258235ECE53F15BB292E2DDFCBB15F181CFE9E2F1B5B95A`，
> `axon-desktop.exe` = `3B662B4B6C122A32802787F34D449C8C4872A28E4952EE97D79F47B6814B8587`。
> 隔离方式：`AXON_RUNTIME_ROOT` 指向 `$env:TEMP/axon-verify-rt`。目录名不能以 `.axon` 结尾，否则 CLI 会判成默认运行态根、端点退化成连不上的 `axon-daemon`；配置、`.env` 与 skills 都是副本，真实 `$HOME/.axon/config.toml` 前后 SHA-256 均为 `2208FC66D456DED6B05C4FF60E4BE7F68BDA65E97FEAB450C3732B4E8FC06646`。
> Daemon：`axon --config <副本> daemon run --root <临时根> --config-path <副本>`，健康检查读 `axon daemon health` 的 JSON。
> 判据：只看输出文本，不看退出码；未取得证据的条目一律标注「未真机验证」。

## 基础（B）

| # | 检查 | 结果 | 证据 |
| --- | --- | --- | --- |
| B1 | `axon --version` | 通过 | `axon-cli 0.0.1` |
| B2 | `Get-Command axon` | 通过 | 解析到 `C:\Users\<用户>\.axon\bin\axon.cmd`，不是 cargo bin |
| B3 | `axon doctor` | 通过 | `Axon doctor: ok`；`config`/`cli`/`preflight`/`provider`/`mcp`/`memory`/`persona`/`lsp`/`smoke` 全项 ok（`channels=11 models=16 registered_tools=37`） |
| B4 | `data/` 布局 | 通过 | 空临时根起跑后 `data/` 下只有 `control.sqlite3`、`outbox.sqlite3` 与运行态子目录；递归查 `rollout.sqlite3` 零命中 |
| B5 | 多回合写入 | 通过 | 每段会话生成 `data/events/<会话>/session.v1.jsonl.zstd` 与 `session.v1.jsonl.lock`；两段会话分别 74198 与 10554 字节，Thread 事件落在 `data/sessions/threads/<hex>`，索引为 `data/sessions/thread-index.v1.*` |
| B6 | Daemon 连续重启 3 次 | 未真机验证 | 未执行 |
| B7 | 24 小时长跑 | 未真机验证 | 未执行 |

## CLI 会话（C1 至 C6）

| # | 检查 | 结果 | 证据 |
| --- | --- | --- | --- |
| C1 | `axon sessions list` | 通过 | 输出 ` session-session-4a60…  记住数字 41。只回复两个字：收到  last=2026-11-17 07:50  turns=2`；同一段对话只出现一行，不再出现「有轮次的行 + 线程 ID 空壳行」两份 |
| C2 | `axon run --session <会话清单 ID>` | 通过 | 直接取 `sessions list` 里的 `session-session-4a60…` 追加：回复 `收到`，该会话 turns 4→5，`sessions list` 不新增会话行 |
| C3 | `--session` 指向不存在的 id | 通过 | `error: 会话不存在：session-does-not-exist。先用 axon sessions list 查看可用会话；--session 不会隐式新建会话` |
| C4 | 不带 `--session`/`--continue`，同 cwd 连续两轮 | 通过 | 第一轮 `记住数字 41。只回复两个字：收到` 回 `收到`；第二轮同 cwd 直接问 `我刚让你记住的数字是多少？只回复数字` 回 `41`，落在同一会话（turns 2） |
| C5 | `sessions resume <会话清单 ID>` 后 `run --continue` | 通过 | `sessions resume session-session-4a60…` 输出 `已恢复 Thread: 4227871b…` 与 `- session: session-session-4a60…`（不再报「未找到 thread」）；随后 `run --continue` 回 `41`，该会话 turns 2→3 |
| C6 | `--session` 与已保存的 resume 指针冲突 | 通过 | 先在临时 cwd 建第二段会话（`记住数字 77`，`session-session-f7cc38…`）并 `sessions resume` 到它；再在仓库目录 `run --session session-session-4a60…` 回 `收到`，会话 A turns 4→5（`--session` 胜出）；随后 `run --continue` 仍指向 B，被护栏拦下：`无法续接会话 session-session-f7cc38…：该会话属于其他工作区或受众，请在原工作目录下运行`；回到 B 的原工作目录 `run --continue` 回 `77`，B turns 1→2，证明 `--session` 没有覆盖恢复指针 |

C5/C6 原先的阻塞根因已闭合：`sessions list` 展示的是运行态会话 ID，Thread 事件按运行窗口 `run_session_id` 落盘，两者在 CLI 复合车道并不相等。会话命令现在统一经一层解析（先按 Thread 命中，未命中回落到该会话的运行窗口），`--continue` 也按恢复指针解析回 Thread 后再恢复历史。

## 桌面（D1 至 D12）

未真机验证。需要 `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS='--remote-debugging-port=9333'` 启动桌面端，再经 `tools/cdp-eval.ps1` / `cdp-click.ps1` / `cdp-type.ps1` 取证，判定只看 `OK:` 前缀。

## TUI（T1 至 T7）

未真机验证。本环境无交互终端。

## 跨端（X1 至 X7）

未真机验证。前置的 C 组续聊语义已在本轮取证通过，X 组仍需桌面端 CDP 与 TUI 侧证据。

## 环境副作用登记

- 源码安装走 `scripts/install-from-source.ps1 -Components all`（当前唯一合规入口；`-Components backend` 会因版本目录缺桌面端二进制被拒）。脚本静默提权后重建版本树，并拉起过一次 Daemon 与桌面端，验证开始前已全部终止。
- 该次 Daemon 启动会向真实 `$HOME/.axon` 追加运行态写入（`data/` 下 WAL、`cron/`、`usage/` 时间戳变化），已删除其留下的 `data/daemon.pid`；真实 `config.toml` 哈希未变，真实运行态库中原有数据未删除。
- 临时验证根 `$env:TEMP/axon-verify-rt` 保留，供下一轮继续取证；验证结束后需清空调试端口环境变量与临时根。
