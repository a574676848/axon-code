# 交叉审计问题清单（第 26 轮：rollout 退役专项）

> 本轮范围：`edb5f34fd..671cc9894`（rollout 整删）。两个独立只读审计各出一份：
> 性能（P 组）与闭环/稳定性（S 组）。修复提交见文末；未修项如实留在这里，不标已完成。

## 审计结论（修复前）

- 性能：3 个 P0。用量投影 cron 每 60 秒全量扫描全部 thread 并逐行反序列化；子代理名册每
  10 秒全量扫描；每轮收口的中途落盘把原来的 SQLite 行 upsert 换成「3 遍整文件读 + 1 次追加」，
  单轮收尾约 44 遍全文件读，整体 O(T²)。
- 闭环：1 个 P0。「中途落盘」按位置切片追加，而历史 item 的生成顺序不是只追加尾部
  （工具回执插在调用之间、reasoning 排在最后）；运行中崩溃或本轮失败后没有任何路径重写
  这一轮，错位历史会永久留在模型可见历史里，后续每一轮都带着重复 `tool_call_id` / 缺失
  工具结果发请求。

## P0（已全部修复）

| 编号 | 位置（修复前） | 问题 | 修法 |
| --- | --- | --- | --- |
| P0-1（S 组） | `run_thread_store.rs` Active 分支、`projection.rs::apply_turn_items_appended` | 按位置切片追加 → 回执错位、丢结果、重复 id，且失败/崩溃路径无终态覆盖 | 历史 item 增加稳定身份 `(类别, part_id)`；投影改为 upsert，写入侧只追加「新身份或内容已变」的 item；补 `partial_write_with_settled_receipts_keeps_history_consistent` 回归用例 |
| P0-2（P 组） | `usage_statistics/projection.rs::read_thread_usage_rows` | 每 60 秒 `scan_threads` 全量反序列化全部轮次，水位在解析之后才生效 | 新增 thread 派生索引（`thread-index.v1.jsonl`，append 时写补丁、可 `rebuild`）；投影先用索引判断「哪些会话在上次水位之后写过」，只读这些会话的头部分片 + 尾部分片；索引缺失才回退整表投影 |
| P0-3（P 组） | `run_thread_partial.rs`、`run_thread_store.rs::mirror_existing_turn` | 每次 provider 迭代 3 遍全文件读（mirror 一次 + append_turn_items 一次 + 激活扫描） | 新增 `read_thread_tail`（有字节预算，可分片回溯）；现状 item 从尾部投影得到，尾部不覆盖本轮起跑时才回退整读；追加走 `append_turn_items_at_sequence`（复用激活快照，不再二次整读） |
| P0-4（P 组） | `run_thread_children.rs::child_session_rows` | 名册每次轮询 `list_thread_page` → `read_all` 全量投影 | 名册改读派生索引（父会话 / 来源 / 标题 / 模型 / 末轮状态与时间），索引缺失才回退整表投影 |

## P1（已修）

| 编号 | 位置 | 问题 | 修法 |
| --- | --- | --- | --- |
| P1-1（S 组） | `run_provider_orchestration.rs`、`run_thread_store.rs::prepare_session_from_lookup` | 崩溃修复晚于模型历史装配：恢复后第一条请求必带悬空 tool_call | 恢复流程先读激活快照，存在 active turn 就先 `repair_open_turn`，再装配历史 |
| P1-2（S 组） | `prepare_session_from_lookup` | 损坏 thread 被当成「没有会话」静默返回 | 区分「分片不存在」与「读取失败」：前者返回无会话，后者显式报错 |
| P1-3（S 组） | `latest_thread_id` | `--continue` 兜底取「最近未归档 thread」，可能续到子代理/内务会话 | 兜底排除 `parent_thread_id` 非空的 thread；权威目标仍是恢复指针 |
| P1-4（S 组） | `axon-jsonl::store::rewrite_unlocked` | 换窗 checkpoint 先删全部分片再写一条，崩溃即整会话归零 | 先写临时分片 → rename 就位 → 再删旧分片；空内容仍走删除分支 |
| P1-5（P 组） | `usage_statistics/service.rs::newest_thread_event_ms` | 每次 usage 请求 stat 全部 thread 分片（含 `.lock`） | 优先读派生索引的最新更新时间；索引缺失时才扫目录且只认 `.jsonl` |
| P1-6（P 组） | `run_thread_tail.rs` 四个热查询 | `recent_turn_facts`、`turn_sequence`、`latest_turn_usage`、`previous_reflection_notice` 每次整读 thread | 统一走尾部视图，尾部不足才回退整读 |
| P1-7（S 组） | `session_family_cleanup.rs::family_members` | 保留判定整表投影；`scan_threads` 丢弃损坏分片且不上报，父节点缺失会让子会话脱离家族被误删 | 改读派生索引；对「父 id 存在但投影里没有父」的会话整族保护；回退投影时把损坏分片数写进告警 |

## 第二轮复核后新增修复（本清单随代码同步）

| 编号 | 来源 | 问题 | 修法 |
| --- | --- | --- | --- |
| N-1 | 第二轮性能+闭环 | `JsonlThreadEventStore::read_thread` 覆盖实现被删，trait 默认实现退化为 `read_all`（整语料） | 恢复按 thread 分片读取的覆盖实现；新增回归靠 `thread_index_tests` 与既有 session 用例覆盖 |
| N-2 | 第二轮闭环 | 尾部视图不识别 `ThreadCheckpoint`，换窗后记为「完整但空」 | `project_tail` 应用 checkpoint 里的全量轮次 |
| N-3 | 第二轮闭环 | 索引读/写/重建三条路径互不可见（逻辑路径 vs 分片路径），索引恒不可读 | 读侧改判分片存在、重建改走分片写入；新增 `thread_index_tests`（写入可读、删除 tombstone、rebuild 一致） |
| N-5 | 第二轮闭环 | 用户消息 part_id 含落盘时间戳，重复落盘会追加重复用户输入 | 用户 item 身份改为「会话 + 轮次」；新增 `repeated_partial_writes_keep_a_single_user_message` |
| N-6 | 第二轮闭环 | 删除会话不清理索引条目 | `remove_thread` 写 tombstone 补丁，折叠时移除条目 |
| N-9 | 第二轮闭环 | 热路径追加用预读序号直接写，无冲突校验 | 改走 `append_at_expected_sequence` |
| P1-5（补正） | 第二轮闭环 | 首轮清单写「已修」，实际 service 侧未改 | 现已改为优先读索引最新更新时间，索引缺失才扫目录且只认 `.jsonl` |
| R3 | 第二轮性能 | 激活状态反向扫描按行读 64 KiB 窗口，物理读放大 33× | 改为「整分片从尾回读 + 状态已定即停」，预算按实际读取字节扣减；删除三处旧扫描助手 |
| N-4（部分） | 第二轮闭环 | 换窗重写单分片 8 MiB 上限让大会话无法压缩；rename 与删旧分片之间崩溃会新旧并存 | 重写改为多分片分块 + 临时文件逐个就位；读取侧按 sequence 稳定去重，消除并存窗口的重复轮次 |

## P2（已记录，暂不修）

| 编号 | 位置 | 问题 | 处置 |
| --- | --- | --- | --- |
| P2-1（S 组） | `run_thread_queries.rs::patch_turn_artifacts` | 治理产物回写仍整轮重写（含全部 history items），大轮次二次膨胀，且触到单条 64 MiB 上限时只留告警 | 计划改为独立的轮次注解事件；当前有整轮终态替换兜底，未列入本轮 |
| P2-2（P 组） | `axon-jsonl::store` append 路径 | `repair_tail` 与 `append_target` 各列一次整个 `threads/` 目录，代价随全局分片数增长 | 计划改用激活快照里的目标分片号；属底层写入优化，单独一轮 |
| P2-3（P 组） | `thread_service::compact_thread` | checkpoint 写入完整 detail，压缩后文件不缩小；超 64 MiB 单条上限的会话无法压缩 | 计划在 checkpoint 内只保留窗口内轮次；需与窗口语义一起改 |
| P2-4（P 组） | `workbench_http_session_readings.rs` 注释 | 文档注释仍写「一次 sessions 单行查 + 一次 rollout_turns 最新行查」，与现状不符 | 注释口径修正随 P2-1 一起做 |
| P2-5（S 组） | `usage_statistics` 活跃事实复核 | 复核集合上限 256，且 `abandoned` 不可逆 | 已记录；需要有终态的会话复活语义，单独一轮 |
| P2-6（S 组） | `cleanup_usage_data` | 会话被保留策略删除后用量事实保留 90 天，与「会话级事实随会话走」口径冲突 | 已记录；需产品口径裁决后改 |

## 本轮复核命令

```text
cargo fmt --all --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
pwsh -File tools/verify-architecture.ps1
pwsh -File tools/verify-docs.ps1
```

补的回归用例：`run_thread_store_tests::partial_write_with_settled_receipts_keeps_history_consistent`、
`run_thread_store_tests::partial_updates_append_only_new_history_items`（改为身份口径）、
`session_family_cleanup_tests::current_resume_session_family_is_protected`、
`run_thread_children_tests`（六条名册用例）、`usage_cli_tests`（水位与原地更新）。