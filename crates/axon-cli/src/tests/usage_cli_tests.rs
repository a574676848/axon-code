use std::fs;

use axon_agent_runtime::{AgentTurnEvent, AgentTurnEventKind};
use axon_domain::TurnId;
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
    let rollout_path = crate::run_commands::resolved_rollout_db_path(&config_path)
        .expect("rollout 路径应可解析");
    fs::create_dir_all(rollout_path.parent().expect("rollout 应有父目录"))
        .expect("rollout 目录应可创建");
    let connection = Connection::open(&rollout_path).expect("rollout SQLite 应可创建");
    create_rollout_schema(&connection);
    insert_rollout_turn(
        &connection,
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
    );
    insert_rollout_turn(
        &connection,
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
    );
    drop(connection);

    let first_projection = crate::usage_statistics::run_usage_rollup_once(&config_path)
        .expect("首次 usage projection 应成功");
    let repeated_projection = crate::usage_statistics::run_usage_rollup_once(&config_path)
        .expect("重复 usage projection 应成功");
    assert_eq!(first_projection.projected_records, 2);
    assert_eq!(first_projection.changed_facts, 2);
    assert_eq!(first_projection.source_watermark_rowid, 2);
    assert_eq!(repeated_projection.projected_records, 0);
    assert_eq!(repeated_projection.changed_facts, 0);
    assert_eq!(repeated_projection.source_watermark_rowid, 2);

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
fn usage_refresh_reconciles_in_place_rollout_terminal_update() {
    let root = test_root("usage-reconcile");
    let config_path = root.join("config.toml");
    let rollout_path =
        crate::run_commands::resolved_rollout_db_path(&config_path).expect("rollout 路径应可解析");
    fs::create_dir_all(rollout_path.parent().expect("rollout 应有父目录"))
        .expect("rollout 目录应可创建");
    let connection = Connection::open(&rollout_path).expect("rollout SQLite 应可创建");
    create_rollout_schema(&connection);
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
    insert_partial_rollout_turn(&connection, &turn);
    drop(connection);

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

    let connection = Connection::open(&rollout_path).expect("rollout SQLite 应可打开");
    let provider_completed = AgentTurnEvent::new(
        TurnId(1),
        AgentTurnEventKind::ProviderCompleted,
        "Provider 完成",
    )
    .with_duration_ms(250);
    let completed = AgentTurnEvent::new(TurnId(1), AgentTurnEventKind::Completed, "Turn 完成");
    let record = json!({
        "sessionId": turn.session_id,
        "parentSessionId": turn.parent_session_id,
        "provider": turn.provider,
        "model": turn.model,
        "assistantPartId": turn.assistant_part_id,
        "usage": {
            "inputTokens": turn.input_tokens,
            "cacheReadInputTokens": 0,
            "cacheWriteInputTokens": 0,
            "outputTokens": turn.output_tokens,
            "reasoningOutputTokens": 0,
            "currentContextTokens": turn.input_tokens + turn.output_tokens,
            "totalInputTokens": turn.input_tokens
        },
        "turnEvents": [provider_completed, completed]
    })
    .to_string();
    connection
        .execute(
            "UPDATE rollout_turns SET record_json = ?1 WHERE session_id = ?2 AND assistant_part_id = ?3",
            params![record, turn.session_id, turn.assistant_part_id],
        )
        .expect("rollout terminal 更新应成功");
    drop(connection);

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

fn insert_partial_rollout_turn(connection: &Connection, turn: &TestUsageTurn<'_>) {
    let recorded_at_ms = axon_time::unix_now_ms();
    let record = json!({
        "sessionId": turn.session_id,
        "parentSessionId": turn.parent_session_id,
        "provider": turn.provider,
        "model": turn.model,
        "assistantPartId": turn.assistant_part_id,
        "usage": null,
        "turnEvents": []
    })
    .to_string();
    connection
        .execute(
            "INSERT INTO rollout_turns (
                session_id, sequence, assistant_part_id, recorded_at_ms, record_json
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                turn.session_id,
                i64::try_from(turn.sequence).expect("测试 sequence 应有效"),
                turn.assistant_part_id,
                i64::try_from(recorded_at_ms).expect("测试时间戳应有效"),
                record
            ],
        )
        .expect("partial rollout turn 应可写入");
}

fn create_rollout_schema(connection: &Connection) {
    connection
        .execute_batch(
            "CREATE TABLE rollout_turns (
                session_id TEXT NOT NULL,
                sequence INTEGER NOT NULL,
                assistant_part_id TEXT NOT NULL,
                recorded_at_ms INTEGER NOT NULL,
                record_json TEXT NOT NULL,
                PRIMARY KEY (session_id, sequence),
                UNIQUE (session_id, assistant_part_id)
             );",
        )
        .expect("rollout schema 应可创建");
}

fn insert_rollout_turn(connection: &Connection, turn: &TestUsageTurn<'_>) {
    let recorded_at_ms = axon_time::unix_now_ms();
    let sequence = i64::try_from(turn.sequence).expect("测试 sequence 应在 i64 范围内");
    let recorded_at =
        i64::try_from(recorded_at_ms).expect("测试时间戳应在 i64 范围内");
    let provider_completed = AgentTurnEvent::new(
        TurnId(turn.sequence.saturating_add(1)),
        AgentTurnEventKind::ProviderCompleted,
        "Provider 完成",
    )
    .with_duration_ms(250);
    let completed = AgentTurnEvent::new(
        TurnId(turn.sequence.saturating_add(1)),
        AgentTurnEventKind::Completed,
        "Turn 完成",
    );
    let record = json!({
        "sessionId": turn.session_id,
        "parentSessionId": turn.parent_session_id,
        "provider": turn.provider,
        "model": turn.model,
        "assistantPartId": turn.assistant_part_id,
        "usage": {
            "inputTokens": turn.input_tokens,
            "cacheReadInputTokens": 0,
            "cacheWriteInputTokens": 0,
            "outputTokens": turn.output_tokens,
            "reasoningOutputTokens": 0,
            "currentContextTokens": turn.input_tokens + turn.output_tokens,
            "totalInputTokens": turn.input_tokens
        },
        "turnEvents": [provider_completed, completed]
    })
    .to_string();
    connection
        .execute(
            "INSERT INTO rollout_turns (
                session_id, sequence, assistant_part_id, recorded_at_ms, record_json
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                turn.session_id,
                sequence,
                turn.assistant_part_id,
                recorded_at,
                record
            ],
        )
        .expect("rollout turn 应可写入");
}
