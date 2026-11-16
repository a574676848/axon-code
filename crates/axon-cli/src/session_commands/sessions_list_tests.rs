//! `axon sessions list` 的输出与过滤测试。

use std::path::PathBuf;

use axon_session::{RuntimeStore, SqliteRuntimeStore};
use rusqlite::params;

use super::sessions_list::{SessionsListCommand, run_sessions_list};
use crate::OutputFormat;

fn fixture() -> (axon_test_support::TestTempDir, PathBuf, SqliteRuntimeStore) {
    let directory = axon_test_support::test_temp_dir();
    // 非默认位置的配置路径：运行根取配置所在目录，数据落在其 data 子目录。
    let config_path = directory.path().join("config.toml");
    let store = SqliteRuntimeStore::from_root(directory.path().join("data"));
    store.initialize().expect("初始化运行态库");
    (directory, config_path, store)
}

fn seed_session(
    store: &SqliteRuntimeStore,
    session_id: &str,
    last_activity_at_ms: i64,
    pinned: i64,
    archived: i64,
) {
    store
        .with_read_connection(|connection| {
            connection
                .execute(
                    "INSERT INTO sessions(session_id,run_session_id,root_session_id,cwd,status,
                         worker_attempt,context_version,last_checkpoint_seq,last_event_sequence,
                         last_activity_at_ms,created_at_ms,updated_at_ms,pinned,archived)
                     VALUES(?1,?1,?1,'/workspace','idle',0,0,0,0,?2,?2,?2,?3,?4)",
                    params![session_id, last_activity_at_ms, pinned, archived],
                )
                .map(|_| ())
        })
        .expect("写入会话");
}

fn seed_metrics(store: &SqliteRuntimeStore, session_id: &str, turn_count: i64) {
    store
        .with_read_connection(|connection| {
            connection
                .execute(
                    "INSERT INTO session_metrics(session_id,turn_count,total_active_ms,updated_at_ms)
                     VALUES(?1,?2,1200,1)",
                    params![session_id, turn_count],
                )
                .map(|_| ())
        })
        .expect("写入会话指标");
}

fn seed_inbox(store: &SqliteRuntimeStore, session_id: &str, content: &str, received_at_ms: i64) {
    store
        .with_read_connection(|connection| {
            connection
                .execute(
                    "INSERT INTO inbox(interaction_id,source_channel,source_message_id,scope_json,
                         audience_json,reply_route_json,content,lane_id,received_at_ms,updated_at_ms)
                     VALUES(?1,'tui',?2,'{}','{}','{}',?3,?4,?5,?5)",
                    params![
                        format!("inbox-{session_id}"),
                        format!("msg-{session_id}"),
                        content,
                        session_id,
                        received_at_ms
                    ],
                )
                .map(|_| ())
        })
        .expect("写入入站事实");
}

fn list(
    format: OutputFormat,
    config_path: &std::path::Path,
    limit: usize,
    all: bool,
) -> crate::CliRunResult {
    run_sessions_list(format, config_path, &SessionsListCommand { limit, all })
}

#[test]
fn text_list_shows_title_last_activity_and_turn_count() {
    let (_directory, config_path, store) = fixture();
    seed_session(&store, "session-a", 1_700_000_000_000, 0, 0);
    seed_metrics(&store, "session-a", 3);
    seed_inbox(&store, "session-a", "第一个会话标题", 1_700_000_000_000);

    let result = list(OutputFormat::Text, &config_path, 20, false);

    assert_eq!(result.exit_code, 0);
    assert!(result.stdout.contains("session-a"), "必须列出会话 ID");
    assert!(result.stdout.contains("第一个会话标题"), "必须显示标题");
    assert!(result.stdout.contains("turns=3"), "必须显示轮次数");
    assert!(result.stdout.contains("last="), "必须显示最近活动时间");
}

#[test]
fn archived_sessions_only_appear_with_all() {
    let (_directory, config_path, store) = fixture();
    seed_session(&store, "session-live", 2_000, 0, 0);
    seed_session(&store, "session-archived", 1_000, 0, 1);

    let default_view = list(OutputFormat::Text, &config_path, 20, false);
    assert!(default_view.stdout.contains("session-live"));
    assert!(!default_view.stdout.contains("session-archived"));

    let all_view = list(OutputFormat::Text, &config_path, 20, true);
    assert!(all_view.stdout.contains("session-archived"));
    assert!(all_view.stdout.contains("[已归档]"));
}

#[test]
fn limit_truncates_and_pinned_sessions_come_first() {
    let (_directory, config_path, store) = fixture();
    seed_session(&store, "session-newest", 3_000, 0, 0);
    seed_session(&store, "session-pinned", 1_000, 1, 0);

    let result = list(OutputFormat::Text, &config_path, 1, false);
    let lines = result.stdout.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains("session-pinned"), "置顶会话优先");
}

#[test]
fn json_output_carries_contract_fields() {
    let (_directory, config_path, store) = fixture();
    seed_session(&store, "session-a", 1_700_000_000_000, 0, 0);
    seed_metrics(&store, "session-a", 2);

    let result = list(OutputFormat::Json, &config_path, 20, false);
    assert_eq!(result.exit_code, 0);
    let parsed: serde_json::Value = serde_json::from_str(&result.stdout).expect("JSON 输出");
    let rows = parsed.as_array().expect("数组");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["sessionId"], "session-a");
    assert_eq!(rows[0]["turnCount"], 2);
    assert_eq!(rows[0]["totalActiveMs"], 1200);
}

#[test]
fn empty_runtime_reports_empty_list_without_error() {
    let (_directory, config_path, _store) = fixture();
    let text = list(OutputFormat::Text, &config_path, 20, false);
    assert_eq!(text.exit_code, 0);
    assert!(text.stdout.contains("未找到会话"));
    let json = list(OutputFormat::Json, &config_path, 20, false);
    assert_eq!(json.exit_code, 0);
    assert_eq!(json.stdout.trim(), "[]");
}

#[test]
fn zero_limit_is_rejected() {
    let (_directory, config_path, _store) = fixture();
    let result = list(OutputFormat::Text, &config_path, 0, false);
    assert_ne!(result.exit_code, 0, "非法参数必须非零退出");
    assert!(result.stderr.contains("--limit"));
}
