//! 会话列表投影与写入路由的行为测试。
//!
//! 读路径归 read_session_rows（axon sessions list 与性能门槛共用）；HTTP 侧只保留
//! 写入，GET /api/v1/sessions 由工作台投影接管，因此这里同时锁住「不抢读路由」这条契约。
//! 写入，GET /api/v1/sessions 由工作台投影接管，因此这里同时锁住「不抢读路由」这条契约。

use super::{SessionsHttpAdapter, clip_title, median_of_histogram};
use axon_daemon::GatewayHttpRouteHandler;
use axon_session::{RuntimeStore, SqliteRuntimeStore};
use rusqlite::params;
use serde_json::Value;

/// 初始化一个带两条会话的运行时库，返回临时目录（保持存活）。
fn fixture() -> (axon_test_support::TestTempDir, SqliteRuntimeStore) {
    let directory = axon_test_support::test_temp_dir();
    // 适配器持有运行态根，库落在其 data 子目录：fixture 与生产路径保持一致。
    let store = SqliteRuntimeStore::from_root(directory.path().join("data"));
    store.initialize().expect("初始化运行态库");
    (directory, store)
}

fn seed_session(store: &SqliteRuntimeStore, session_id: &str, pinned: i64, archived: i64) {
    store
        .with_read_connection(|connection| {
            connection
                .execute(
                    "INSERT INTO sessions(session_id,current_run_id,root_session_id,trust_scope,kind,cwd,status,
                     worker_attempt,context_version,last_checkpoint_seq,last_event_sequence,
                     last_activity_at_ms,created_at_ms,updated_at_ms,pinned,archived,workspace_id)
                 VALUES(?1,?1,NULL,'private','primary','/workspace','idle',0,0,0,0,?2,?2,?2,?3,?4,'ws-a')",
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

/// 直接读投影，等价于 xon sessions list 的数据来源。
fn rows(store: &SqliteRuntimeStore) -> Vec<Value> {
    super::read_session_rows(store)
        .expect("读取会话清单")
        .iter()
        .map(super::SessionListRow::to_json)
        .collect()
}
/// 会话清单每行两条相关子查询：缺 `inbox_session_recent` 时每次都要全表扫描 inbox。
fn measure_session_list(store: &SqliteRuntimeStore) -> (usize, u128) {
    let mut best = u128::MAX;
    let mut rows = 0;
    for _ in 0..3 {
        let started = std::time::Instant::now();
        rows = super::read_session_rows(store).expect("读取会话清单").len();
        best = best.min(started.elapsed().as_millis());
    }
    (rows, best)
}

#[test]
#[ignore = "性能验收：200 会话 × 20 万入站事实下会话清单必须走 inbox_session_recent，门槛 30ms"]
fn session_list_query_uses_inbox_session_index_and_stays_within_budget() {
    const SESSIONS: usize = 200;
    const ROWS_PER_SESSION: usize = 1_000;
    let (_directory, store) = fixture();
    store
        .with_read_connection(|connection| {
            connection.execute_batch("BEGIN IMMEDIATE")?;
            for session in 0..SESSIONS {
                connection.execute(
                    "INSERT INTO sessions(session_id,current_run_id,root_session_id,trust_scope,kind,cwd,status,
                     worker_attempt,context_version,last_checkpoint_seq,last_event_sequence,
                     last_activity_at_ms,created_at_ms,updated_at_ms,pinned,archived,workspace_id)
                     VALUES(?1,?1,?1,'/workspace','idle',0,0,0,0,?2,?2,?2,0,0,'ws-a')",
                    params![
                        format!("session-{session:04}"),
                        i64::try_from(session).unwrap_or(0)
                    ],
                )?;
                for row in 0..ROWS_PER_SESSION {
                    let received = i64::try_from(row).unwrap_or(0);
                    connection.execute(
                        "INSERT INTO inbox(interaction_id,source_channel,source_message_id,
                             scope_json,audience_json,reply_route_json,content,session_id,
                             received_at_ms,updated_at_ms)
                         VALUES(?1,'tui',?2,'{\"workspace\":\"ws-a\"}','{}','{}',?3,?4,?5,?5)",
                        params![
                            format!("inbox-{session:04}-{row:04}"),
                            format!("msg-{session:04}-{row:04}"),
                            format!("第 {row} 条"),
                            format!("session-{session:04}"),
                            received
                        ],
                    )?;
                }
            }
            connection.execute_batch("COMMIT")
        })
        .expect("写入大规模会话 fixture");

    let (rows, indexed_ms) = measure_session_list(&store);
    assert_eq!(rows, SESSIONS, "会话清单条数");

    store
        .with_read_connection(|connection| {
            connection.execute_batch("DROP INDEX inbox_session_recent")
        })
        .expect("删除索引以取得基线");
    let (_, baseline_ms) = measure_session_list(&store);
    println!(
        "session_list sessions={SESSIONS} inbox_rows={} indexed={indexed_ms}ms baseline={baseline_ms}ms 门槛=30ms",
        SESSIONS * ROWS_PER_SESSION
    );
    assert!(indexed_ms < 30, "会话清单超出 30ms 门槛：{indexed_ms}ms");
    assert!(
        indexed_ms.saturating_mul(3) < baseline_ms.max(1),
        "索引必须带来数量级收益：indexed={indexed_ms}ms baseline={baseline_ms}ms"
    );
}

#[test]
fn lists_sessions_with_title_from_first_inbound_message_and_pinned_first() {
    let (_directory, store) = fixture();
    seed_session(&store, "session-idle", 0, 0);
    seed_session(&store, "session-pinned", 1, 0);
    seed_inbox(&store, "session-idle", "  第一行标题\n第二行 ", 10);
    seed_inbox(&store, "session-idle", "更晚的一条", 20);
    seed_inbox(&store, "session-pinned", "置顶会话", 5);

    let sessions = rows(&store);

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
fn archived_flag_is_projected_for_callers_to_filter() {
    let (_directory, store) = fixture();
    seed_session(&store, "session-live", 0, 0);
    seed_session(&store, "session-archived", 0, 1);

    let sessions = rows(&store);
    assert_eq!(sessions.len(), 2, "投影保留归档行，过滤由调用方决定");
    let archived = sessions
        .iter()
        .find(|row| row["sessionId"] == "session-archived")
        .expect("归档会话在投影里");
    assert_eq!(archived["archived"], true);
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
    assert!(
        adapter
            .handle_request("GET", "/api/v1/sessions", None, &[])
            .is_none(),
        "清单读取必须让位给工作台投影：桌面契约要求会话表面渠道与目标"
    );
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
    assert!(
        axon_session::is_ulid(session_id),
        "会话标识必须是服务端生成的 ULID：{session_id}"
    );

    // 会话确实落到了控制面库，并出现在清单投影里。
    let listed = rows(&store);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["sessionId"], session_id);
    assert_eq!(listed[0]["title"], "新会话");
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
    let listed = rows(&store);
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
    let after = rows(&store);
    assert_eq!(after.len(), 1, "归档不删行，只标记状态");
    assert_eq!(after[0]["archived"], true, "归档位写进控制面");
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
