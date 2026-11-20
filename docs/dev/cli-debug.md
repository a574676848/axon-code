# CLI 诊断命令（开发与排障）

本文记录 `axon debug` 系列只读诊断命令。它们面向排障与数据恢复，不属于日常用户命令面；产品文档只保留用户常用命令。

## 1. 会话诊断

```bash
axon debug session list [--limit <n>] [--cursor <c>] [--cwd <dir>] [--archived]
axon debug session show <THREAD_ID>
axon debug session delete <THREAD_ID>
axon debug session prune --orphans
axon debug session read <THREAD_ID>
axon debug session resume <THREAD_ID>
axon debug session replay <THREAD_ID> [--provider-call-id <id>]
axon debug session output [--max-chars <n>]
axon debug session export <THREAD_ID> [--output <path>]
axon debug session fork <THREAD_ID> [--new-thread-id <id>] [--cwd <dir>]
axon debug session doctor [--repair]
```

两个标识不可互换：`axon debug session` 只收 `THREAD_ID`（`debug session list` 给出的历史线程标识），`axon sessions` 只收会话 ID（`sessions list` 给出的 ULID）。两个入口各有一次性解析，不做互相回落；传错标识空间直接报错并提示对应的清单命令。`reset` 是会话级操作，两个入口都只收会话 ID。

`list` 读运行态会话清单，字段与 `axon sessions list` 同源，`--archived` 表示包含归档会话。

`prune --orphans` 只清理归属会话已经消失的非主会话（子代理、Cron、目标执行）：它按 `root_session_id` 判定归属，归属仍在或仍在运行的一律跳过并在回执里给出原因；不写 `--orphans` 时命令拒绝执行。日常删除单个会话仍走 `axon sessions delete`。

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
axon sessions replay <id> [--provider-call-id <id>]
axon sessions output [--max-chars <n>]
axon sessions export <id> [--output <path>]
axon sessions fork <id> [--new-thread-id <id>] [--cwd <dir>]
axon sessions doctor [--repair]
```

`axon sessions` 与 `debug session` 子命令一一对应，差别只在标识空间：前者收会话 ID，后者收 Thread ID。用户面文档只写前者。

`<id>` 就是 `axon sessions list` 列出的会话 ID：会话行读一次，Thread 取自该行的 `current_run_id`。Thread 标识只在 `axon debug session` 出现，按 `sessions.current_run_id` 反查归属会话；没有归属行的 Thread（例如 `fork` 出来尚未登记会话的）仍可读取与回放，但不能作为 `resume` 的目标。

`axon run --session <id>` 只追加到已存在的会话：ID 不存在时直接报错并提示先 `axon sessions list`，不会隐式新建空会话。`axon sessions resume <id>` 只设置 `--continue` 的默认目标，不改变会话内容；随后的 `axon run --continue` 按指针解析出同一段 Thread 再恢复历史（指针为空时接续最近一段有内容的主会话（没有归属 Thread 的零轮会话会被跳过））。带 `--session` 的 run 结束后该默认目标保持运行前的值。

## 4. 相关文档

- [`../cli.md`](../cli.md) - 用户命令参考
- [`./daemon-internals.md`](./daemon-internals.md) - 守护进程内部实现
