//! 会话列表适配器的行为测试：排序、过滤、标题回落与中位数口径。

use super::{QueryFilters, SessionsHttpAdapter, clip_title, median_of_histogram};
use axon_daemon::GatewayHttpRouteHandler;
use axon_session::{RuntimeStore, SqliteRuntimeStore};
use rusqlite::params;
use serde_json::Value;

/// 初始化一个带两条会话的运行时库，返回临时目录（保持存活）。
fn fixture() -> (axon_test_support::TestTempDir, SqliteRuntimeStore) {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::from_root(directory.path());
    store.initialize().expect("初始化运行态库");
    (directory, store)
}

fn seed_session(store: &SqliteRuntimeStore, session_id: &str, pinned: i64, archived: i64) {
    store
        .with_read_connection(|connection| {
            connection
                .execute(
                    "INSERT INTO sessions(session_id,run_session_id,root_session_id,cwd,status,
                     worker_attempt,context_version,last_checkpoint_seq,last_event_sequence,
                     last_activity_at_ms,created_at_ms,updated_at_ms,pinned,archived,workspace_id)
                 VALUES(?1,?1,?1,'/workspace','idle',0,0,0,0,?2,?2,?2,?3,?4,'ws-a')",
                    params![session_id, 1_000_i64, pinned, archived],
                )
                .map(|_| ())
        })
        .expect("写入会话");
}

fn seed_inbox(store: &SqliteRuntimeStore, session_id: &str, content: &str, received_at_ms: i64) {
    store
        .with_read_connection(|connection| {
            connection
                .execute(
                    "INSERT INTO inbox(interaction_id,source_channel,source_message_id,scope_json,
                         audience_json,reply_route_json,content,session_id,received_at_ms,updated_at_ms)
                     VALUES(?1,'tui',?2,'{\"workspace\":\"ws-a\"}','{}','{}',?3,?4,?5,?5)",
                    params![
                        format!("inbox-{received_at_ms}"),
                        format!("msg-{received_at_ms}"),
                        content,
                        session_id,
                        received_at_ms
                    ],
                )
                .map(|_| ())
        })
        .expect("写入入站事实");
}

fn request(adapter: &SessionsHttpAdapter, query: Option<&str>) -> Vec<Value> {
    let response = adapter
        .handle_request("GET", "/api/v1/sessions", query, &[])
        .expect("命中共用路由");
    let parsed: Value = serde_json::from_slice(&response.body).expect("响应是 JSON");
    parsed.as_array().cloned().unwrap_or_default()
}

#[test]
fn lists_sessions_with_title_from_first_inbound_message_and_pinned_first() {
    let (directory, store) = fixture();
    seed_session(&store, "session-idle", 0, 0);
    seed_session(&store, "session-pinned", 1, 0);
    seed_inbox(&store, "session-idle", "  第一行标题\n第二行 ", 10);
    seed_inbox(&store, "session-idle", "更晚的一条", 20);
    seed_inbox(&store, "session-pinned", "置顶会话", 5);

    let adapter = SessionsHttpAdapter::new(directory.path().to_path_buf());
    let sessions = request(&adapter, None);

    assert_eq!(sessions.len(), 2);
    assert_eq!(
        sessions[0]["sessionId"], "session-pinned",
        "置顶会话排在最前"
    );
    assert_eq!(sessions[0]["title"], "置顶会话");
    assert_eq!(
        sessions[1]["title"], "第一行标题",
        "标题只取首条消息的第一个非空行"
    );
    assert_eq!(sessions[1]["workspaceId"], "ws-a");
}

#[test]
fn archived_sessions_are_hidden_unless_requested() {
    let (directory, store) = fixture();
    seed_session(&store, "session-live", 0, 0);
    seed_session(&store, "session-archived", 0, 1);

    let adapter = SessionsHttpAdapter::new(directory.path().to_path_buf());
    let visible = request(&adapter, None);
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0]["sessionId"], "session-live");

    let all = request(&adapter, Some("include_archived=true"));
    assert_eq!(all.len(), 2, "显式请求时归档会话必须出现");
}

#[test]
fn workspace_filter_matches_scope_derived_workspace() {
    let (directory, store) = fixture();
    seed_session(&store, "session-a", 0, 0);

    let adapter = SessionsHttpAdapter::new(directory.path().to_path_buf());
    assert_eq!(request(&adapter, Some("workspace=ws-a")).len(), 1);
    assert!(request(&adapter, Some("workspace=ws-b")).is_empty());
}

#[test]
fn titles_clip_and_collapse_whitespace() {
    let long = "项目".repeat(80);
    let title = clip_title(&format!("\n  {long}  \n"));
    assert_eq!(title.chars().filter(|c| *c == '…').count(), 1);
    assert_eq!(title.chars().count(), 61);
    assert_eq!(clip_title("   "), "");
}

#[test]
fn median_of_histogram_uses_bucket_midpoint() {
    // 桶 3 是 5..10 毫秒：三样本全部落在该桶，中位数取其中点。
    let histogram = "[0,0,0,3,0,0,0,0,0,0,0,0,0,0,0,0]";
    assert_eq!(median_of_histogram(histogram), Some(7));
    assert_eq!(median_of_histogram("[]"), None);
    assert_eq!(median_of_histogram("not-json"), None);
}

#[test]
fn unknown_method_and_path_are_not_claimed() {
    let directory = axon_test_support::test_temp_dir();
    let adapter = SessionsHttpAdapter::new(directory.path().to_path_buf());
    assert!(
        adapter
            .handle_request("DELETE", "/api/v1/sessions", None, &[])
            .is_none(),
        "未实现的方法不接管"
    );
    assert!(
        adapter
            .handle_request("GET", "/api/v1/sessions/x", None, &[])
            .is_none(),
        "子路径不接管"
    );
}

#[test]
fn filters_parse_percent_encoded_workspace() {
    let filters = QueryFilters::parse(Some("workspace=ws%2Da&include_archived=1"));
    assert_eq!(filters.workspace_id.as_deref(), Some("ws-a"));
    assert!(filters.include_archived);
    let default = QueryFilters::parse(None);
    assert!(default.workspace_id.is_none());
    assert!(!default.include_archived);
}

#[test]
fn creates_a_session_and_lists_it() {
    let (directory, store) = fixture();
    let adapter = SessionsHttpAdapter::new(directory.path().to_path_buf());
    let response = adapter
        .handle_request(
            "POST",
            "/api/v1/sessions",
            None,
            r#"{"workspaceId":"ws-a","cwd":"/workspace","title":"新会话"}"#.as_bytes(),
        )
        .expect("命中共用路由");
    assert_eq!(response.status, 200);
    let created: Value = serde_json::from_slice(&response.body).expect("JSON");
    let session_id = created["sessionId"].as_str().expect("返回会话 ID");
    assert!(session_id.starts_with("session-"), "会话 ID 应带前缀");

    // 会话确实落到了控制面库。
    let listed = request(&adapter, Some("workspace=ws-a"));
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["sessionId"], session_id);
    assert_eq!(listed[0]["title"], "新会话");
    drop(store);
}

#[test]
fn patches_pinned_and_archived_state() {
    let (directory, store) = fixture();
    seed_session(&store, "session-live", 0, 0);
    let adapter = SessionsHttpAdapter::new(directory.path().to_path_buf());

    let response = adapter
        .handle_request(
            "PATCH",
            "/api/v1/sessions/session-live",
            None,
            br#"{"pinned":true}"#,
        )
        .expect("命中单会话路由");
    assert_eq!(response.status, 200);
    let listed = request(&adapter, None);
    assert_eq!(listed[0]["pinned"], true);

    let archived = adapter
        .handle_request(
            "PATCH",
            "/api/v1/sessions/session-live",
            None,
            br#"{"archived":true}"#,
        )
        .expect("命中单会话路由");
    assert_eq!(archived.status, 200);
    assert!(request(&adapter, None).is_empty(), "归档后默认列表为空");
    assert_eq!(
        request(&adapter, Some("include_archived=true")).len(),
        1,
        "显式请求时归档会话仍可找回"
    );
}

#[test]
fn patch_rejects_unknown_session_and_empty_body() {
    let (directory, store) = fixture();
    seed_session(&store, "session-live", 0, 0);
    let adapter = SessionsHttpAdapter::new(directory.path().to_path_buf());

    let missing = adapter
        .handle_request(
            "PATCH",
            "/api/v1/sessions/missing",
            None,
            br#"{"pinned":true}"#,
        )
        .expect("命中单会话路由");
    assert_eq!(missing.status, 404);

    let empty = adapter
        .handle_request("PATCH", "/api/v1/sessions/session-live", None, b"{}")
        .expect("命中单会话路由");
    assert_eq!(empty.status, 400);
}

#[test]
fn create_rejects_malformed_body() {
    let (directory, _store) = fixture();
    let adapter = SessionsHttpAdapter::new(directory.path().to_path_buf());
    let response = adapter
        .handle_request("POST", "/api/v1/sessions", None, b"not-json")
        .expect("命中共用路由");
    assert_eq!(response.status, 400);
}
