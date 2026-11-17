# CLI 诊断命令（开发与排障）

本文记录 `axon debug` 系列只读诊断命令。它们面向排障与数据恢复，不属于日常用户命令面；产品文档只保留用户常用命令。

## 1. 会话诊断

```bash
axon debug session list [--limit <n>] [--cursor <c>] [--cwd <dir>] [--archived]
axon debug session show <SESSION_ID>
axon debug session delete <SESSION_ID>
axon debug session read <THREAD_ID>
axon debug session resume <THREAD_ID>
axon debug session replay [--provider-call-id <id>]
axon debug session output [--max-chars <n>]
axon debug session export [--output <path>]
axon debug session fork [--new-thread-id <id>] [--cwd <dir>]
axon debug session doctor [--repair]
```

两个标识不可互换：

- `show` / `delete` 收 `SESSION_ID`（运行态会话标识）。
- `read` / `resume` 收 `THREAD_ID`（历史线程标识）。

`list` 读运行态会话清单，字段与 `axon sessions list` 同源，`--archived` 表示包含归档会话。

## 2. 输出格式

`--format` 既可作为顶层参数（`axon --format json <command> ...`），也可由支持它的子命令自己接收。子命令帮助里没有 `--format` 的命令（例如 `axon config get`、`axon debug session read`）必须使用顶层写法：

```bash
axon --format json debug session read <THREAD_ID>
```

## 3. 与用户命令的关系

日常会话操作走 `axon sessions`：

```bash
axon sessions list [--limit <n>] [--all]
axon sessions show <id>
axon sessions reset <id>
axon sessions delete <id>
axon sessions read <id>
axon sessions resume <id>
axon sessions replay [--provider-call-id <id>]
axon sessions output [--max-chars <n>]
axon sessions export [--output <path>]
axon sessions fork [--new-thread-id <id>] [--cwd <dir>]
axon sessions doctor [--repair]
```

`axon sessions` 是 `debug session` 的提层入口，子命令一一对应；两者行为一致，用户面文档只写前者。

`<id>` 两个清单里的身份都收：`axon sessions list` 的会话 ID 与 `debug session list` 的 Thread ID。入口先按 Thread 直接命中，未命中再回落到该会话的运行窗口（`sessions.run_session_id`），因此按清单里的 ID 直接执行不会撞上「未找到 thread」。

`axon run --session <id>` 只追加到已存在的会话：ID 不存在时直接报错并提示先 `axon sessions list`，不会隐式新建空会话。`axon sessions resume <id>` 只设置 `--continue` 的默认目标，不改变会话内容；随后的 `axon run --continue` 按指针解析出同一段 Thread 再恢复历史（指针为空时取最近一段根会话）。带 `--session` 的 run 结束后该默认目标保持运行前的值。

## 4. 相关文档

- [`../cli.md`](../cli.md) - 用户命令参考
- [`./daemon-internals.md`](./daemon-internals.md) - 守护进程内部实现
