use std::{fs, path::Path};

use rusqlite::{Connection, params};
use serde_json::{Value, json};

use super::cli_test_helpers::test_root;
use super::{AxonCli, run_parsed_cli};

struct TestUsageTurn<'a> {
    session_id: &'a str,
    sequence: u64,
    assistant_part_id: &'a str,
    parent_session_id: Option<&'a str>,
    provider: &'a str,
    model: &'a str,
    input_tokens: u64,
    output_tokens: u64,
}

#[test]
fn usage_refresh_queries_preaggregated_data_with_filters() {
    let root = test_root("usage-refresh");
    let config_path = root.join("config.toml");
    fs::create_dir_all(&root).expect("测试根目录应可创建");
    seed_usage_turn(
        &root,
        &TestUsageTurn {
            session_id: "session-main",
            sequence: 0,
            assistant_part_id: "assistant-main",
            parent_session_id: None,
            provider: "provider-a",
            model: "model-a",
            input_tokens: 100,
            output_tokens: 200,
        },
        true,
    );
    seed_usage_turn(
        &root,
        &TestUsageTurn {
            session_id: "session-subagent",
            sequence: 0,
            assistant_part_id: "assistant-subagent",
            parent_session_id: Some("session-main"),
            provider: "provider-b",
            model: "model-b",
            input_tokens: 300,
            output_tokens: 400,
        },
        true,
    );


    let first_projection = crate::usage_statistics::run_usage_rollup_once(&config_path)
        .expect("首次 usage projection 应成功");
    let repeated_projection = crate::usage_statistics::run_usage_rollup_once(&config_path)
        .expect("重复 usage projection 应成功");
    assert_eq!(first_projection.projected_records, 2);
    assert_eq!(first_projection.changed_facts, 2);
    assert!(first_projection.source_watermark_rowid > 0);
    // 水位按毫秒比较且取 `>=`：同一毫秒的边界轮会重投影一次，但事实幂等、不再变更。
    // 边界同毫秒的轮会重读一次；跨毫秒时只剩水位那一行，因此只断言「不扩大范围」。
    assert!(repeated_projection.projected_records <= 2);
    assert_eq!(repeated_projection.changed_facts, 0);
    assert_eq!(
        repeated_projection.source_watermark_rowid,
        first_projection.source_watermark_rowid
    );

    let config_arg = config_path.to_string_lossy().to_string();
    let provider_query = AxonCli::parse_from_args([
        "axon",
        "--config",
        &config_arg,
        "usage",
        "--refresh",
        "--period",
        "all",
        "--provider",
        "provider-a",
        "--format",
        "json",
    ]);
    let provider_result = run_parsed_cli(&provider_query);
    let provider_body: Value =
        serde_json::from_str(&provider_result.stdout).expect("usage 应输出 JSON");

    assert_eq!(provider_result.exit_code, 0);
    assert_eq!(provider_body["summary"]["requestCount"], 1);
    assert_eq!(provider_body["summary"]["totalInputTokens"], 100);
    assert_eq!(provider_body["summary"]["outputTokens"], 200);
    assert_eq!(provider_body["summary"]["providerCallCount"], 1);
    assert_eq!(provider_body["summary"]["averageProviderLatencyMs"], 250);
    assert_eq!(provider_body["summary"]["costStatus"], "unpriced");
    assert_eq!(provider_body["projection"]["projectedFactCount"], 2);
    assert_eq!(provider_body["providers"][0]["provider"], "provider-a");

    let subagent_query = AxonCli::parse_from_args([
        "axon",
        "--config",
        &config_arg,
        "usage",
        "--refresh",
        "--period",
        "all",
        "--model",
        "model-b",
        "--source",
        "subagent",
        "--format",
        "json",
    ]);
    let subagent_result = run_parsed_cli(&subagent_query);
    let subagent_body: Value =
        serde_json::from_str(&subagent_result.stdout).expect("usage 应输出 JSON");

    assert_eq!(subagent_result.exit_code, 0);
    assert_eq!(subagent_body["summary"]["requestCount"], 1);
    assert_eq!(subagent_body["summary"]["totalInputTokens"], 300);
    assert_eq!(subagent_body["recentRequests"][0]["source"], "subagent");
    assert_eq!(subagent_body["projection"]["projectedFactCount"], 2);

    let session_query = AxonCli::parse_from_args([
        "axon",
        "--config",
        &config_arg,
        "usage",
        "--period",
        "all",
        "--session",
        "session-main",
        "--format",
        "json",
    ]);
    let session_result = run_parsed_cli(&session_query);
    assert_eq!(
        session_result.exit_code, 0,
        "session usage 失败：stdout={} stderr={}",
        session_result.stdout,
        session_result.stderr
    );
    let session_body: Value =
        serde_json::from_str(&session_result.stdout).expect("session usage 应输出 JSON");
    assert_eq!(session_result.exit_code, 0);
    assert_eq!(session_body["summary"]["requestCount"], 1);
    assert_eq!(session_body["summary"]["totalInputTokens"], 100);
    assert_eq!(session_body["filters"]["sessionId"], "session-main");

    let run_query = AxonCli::parse_from_args([
        "axon",
        "--config",
        &config_arg,
        "usage",
        "--period",
        "all",
        "--run",
        "assistant-subagent",
        "--format",
        "json",
    ]);
    let run_result = run_parsed_cli(&run_query);
    let run_body: Value =
        serde_json::from_str(&run_result.stdout).expect("run usage 应输出 JSON");
    assert_eq!(run_result.exit_code, 0);
    assert_eq!(run_body["summary"]["requestCount"], 1);
    assert_eq!(run_body["summary"]["totalInputTokens"], 300);
    assert_eq!(run_body["filters"]["runId"], "assistant-subagent");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn usage_refresh_reconciles_in_place_thread_terminal_update() {
    let root = test_root("usage-reconcile");
    let config_path = root.join("config.toml");
    fs::create_dir_all(&root).expect("测试根目录应可创建");
    let turn = TestUsageTurn {
        session_id: "session-reconcile",
        sequence: 0,
        assistant_part_id: "assistant-reconcile",
        parent_session_id: None,
        provider: "provider-a",
        model: "model-a",
        input_tokens: 10,
        output_tokens: 20,
    };
    // 中途落盘：轮次还在跑，先投影出 active 事实。
    seed_usage_turn(&root, &turn, false);

    crate::usage_statistics::run_usage_rollup_once(&config_path)
        .expect("partial usage projection 应成功");
    let usage_path = crate::runtime_paths::runtime_axon_root_path_static(&config_path)
        .expect("usage root 应可解析")
        .join("usage")
        .join("usage.sqlite3");
    let usage = Connection::open(&usage_path).expect("usage SQLite 应可打开");
    let status: String = usage
        .query_row(
            "SELECT status FROM usage_facts WHERE session_id = ?1 AND assistant_part_id = ?2",
            params![turn.session_id, turn.assistant_part_id],
            |row| row.get(0),
        )
        .expect("partial usage fact 应存在");
    assert_eq!(status, "active");
    drop(usage);

    // 同一轮收口：thread 追加 turnCompleted，投影必须重新读这一轮并改写事实。
    seed_usage_turn(&root, &turn, true);

    let report = crate::usage_statistics::run_usage_rollup_once(&config_path)
        .expect("terminal usage projection 应成功");
    assert_eq!(report.projected_records, 1);
    assert_eq!(report.changed_facts, 1);

    let usage = Connection::open(&usage_path).expect("usage SQLite 应可打开");
    let status: String = usage
        .query_row(
            "SELECT status FROM usage_facts WHERE session_id = ?1 AND assistant_part_id = ?2",
            params![turn.session_id, turn.assistant_part_id],
            |row| row.get(0),
        )
        .expect("terminal usage fact 应存在");
    assert_eq!(status, "completed");
    let _ = fs::remove_dir_all(root);
}


/// 用 thread 事件面播种一轮：容器是 `threadStarted` + `turnStarted`/`turnCompleted`。
///
/// 用量事实只在轮次 metadata 里：这里逐字段写出生产会落的那几项，避免测试引入
/// 第二套字段口径。
fn seed_usage_turn(root: &Path, turn: &TestUsageTurn<'_>, completed: bool) {
    let thread_id = turn.session_id;
    let service = axon_session::ThreadService::new(axon_session::JsonlThreadEventStore);
    let existing = service.read_thread(root, thread_id).ok();
    if existing.is_none() {
        let started = json!({
            "schemaVersion": 4,
            "threadId": thread_id,
            "sequence": 0,
            "recordedAtMs": axon_time::unix_now_ms(),
            "event": {
                "type": "threadStarted",
                "session_root_id": turn.parent_session_id.unwrap_or(thread_id),
                "parent_thread_id": turn.parent_session_id,
                "forked_from_thread_id": null,
                "source": "cliRun",
                "ephemeral": false,
                "settings": {
                    "cwd": ".",
                    "provider": turn.provider,
                    "model": turn.model,
                    "selectedSkillName": null
                }
            }
        });
        append_thread_event(root, thread_id, started);
    }
    let metadata = json!({
        "usage": completed.then(|| json!({
            "inputTokens": turn.input_tokens,
            "cacheReadInputTokens": 0,
            "cacheWriteInputTokens": 0,
            "outputTokens": turn.output_tokens,
            "reasoningOutputTokens": 0,
            "currentContextTokens": turn.input_tokens + turn.output_tokens,
            "totalInputTokens": turn.input_tokens
        })),
        "assistantPartId": turn.assistant_part_id,
        "latestFailure": null,
        "suggestedNextStep": null,
        "toolReceipts": [],
        "reasoningMessageCount": 0,
        "turnEventCount": 2,
        "modelName": null,
        "reflectionNotice": null,
        "reasoningEffort": null,
        "promptChars": 4,
        "failed": false,
        "affectSequence": [],
        "upstreamModel": null,
        "requestPath": null,
        "speedMode": null,
        "ttftMs": null,
        "generationSpeedTps": null,
        "providerCallCount": 1,
        "providerDurationMs": 250
    });
    let turn_json = json!({
        "turnId": format!("{thread_id}-turn-{}", turn.assistant_part_id),
        "sequence": turn.sequence,
        "status": if completed { "completed" } else { "active" },
        "historyItems": [],
        "tokenUsageJson": metadata.to_string(),
        "startedAtMs": axon_time::unix_now_ms(),
        "completedAtMs": completed.then(axon_time::unix_now_ms),
        "interruptedAtMs": null
    });
    let event_type = if completed { "turnCompleted" } else { "turnStarted" };
    let event = json!({
        "schemaVersion": 4,
        "threadId": thread_id,
        "sequence": next_thread_sequence(root, thread_id),
        "recordedAtMs": axon_time::unix_now_ms(),
        "event": { "type": event_type, "turn": turn_json }
    });
    append_thread_event(root, thread_id, event);
}

/// 下一个可用的事件序号：直接取存储给出的激活快照，避免手写序号漂移。
fn next_thread_sequence(root: &Path, thread_id: &str) -> u64 {
    axon_session::ThreadEventStore::read_thread_activation(
        &axon_session::JsonlThreadEventStore,
        root,
        thread_id,
    )
    .expect("thread 激活快照应可读取")
    .next_sequence
}

fn append_thread_event(root: &Path, _thread_id: &str, value: Value) {
    let record: axon_session::ThreadEventRecord =
        serde_json::from_value(value).expect("thread 事件 fixture 应有效");
    axon_session::ThreadEventStore::append(&axon_session::JsonlThreadEventStore, root, &record)
        .expect("thread 事件应可写入");
}

/// 回归：索引里仍挂着、分片却被外部删掉的 thread 不得把整桶用量统计打死。
///
/// 现场表现是 usage 定时任务每个周期都失败在「读取 thread 失败：未找到 thread」，
/// 看板从此停在旧水位。聚合按轮次累加，少一条幽灵记录只是少一份历史用量。
#[test]
fn usage_rollup_skips_threads_whose_shard_is_gone() {
    let root = test_root("usage-ghost-thread");
    let config_path = root.join("config.toml");
    fs::create_dir_all(&root).expect("测试根目录应可创建");
    seed_usage_turn(
        &root,
        &TestUsageTurn {
            session_id: "session-live",
            sequence: 0,
            assistant_part_id: "assistant-live",
            parent_session_id: None,
            provider: "provider-a",
            model: "model-a",
            input_tokens: 10,
            output_tokens: 20,
        },
        true,
    );
    seed_usage_turn(
        &root,
        &TestUsageTurn {
            session_id: "session-ghost",
            sequence: 0,
            assistant_part_id: "assistant-ghost",
            parent_session_id: None,
            provider: "provider-a",
            model: "model-a",
            input_tokens: 30,
            output_tokens: 40,
        },
        true,
    );
    // 只删分片、保留索引条目：这就是「幽灵 thread」的真实形态。
    remove_shard_files(&root, "session-ghost");

    let report = crate::usage_statistics::run_usage_rollup_once(&config_path)
        .expect("幽灵 thread 不得阻断 usage projection");
    assert_eq!(
        report.projected_records, 1,
        "只投影仍有分片的那一条，幽灵记录被跳过"
    );
}

/// 删除某个 thread 的分片文件；分片名是 thread 标识的十六进制编码，索引分片名不含它。
fn remove_shard_files(root: &Path, thread_id: &str) {
    let needle: String = thread_id
        .bytes()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    fn walk(directory: &Path, needle: &str) {
        let Ok(entries) = fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, needle);
            } else if path
                .file_name()
                .map(|name| name.to_string_lossy().contains(needle))
                .unwrap_or(false)
            {
                fs::remove_file(&path).expect("测试现场应可删除分片");
            }
        }
    }
    walk(root, &needle);
}
