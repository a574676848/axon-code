//! 会话清单读模型的等价性测试：同一份夹具下新查询与改造前的七子查询逐字段一致。
//!
//! 清单是桌面左侧菜单、TUI 命令与索引页共用的投影，字段语义一旦漂移会在三个入口同时
//! 显形。这里把改造前的 SQL 原样留在测试里当基准，用同一批会话、同一个写入形状比较
//! 两份 JSON 投影：标题回落、首末渠道、作用域回落、入站条数与在跑判定都必须在里面。

use axon_session::{RuntimeStore as _, SqliteRuntimeStore};
use rusqlite::params;
use serde_json::Value;

use super::{SESSION_SCAN_LIMIT, SessionListRow, read_session_rows};

/// 初始化一个空的运行态库。
fn fixture() -> (axon_test_support::TestTempDir, SqliteRuntimeStore) {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::from_root(directory.path().join("data"));
    store.initialize().expect("初始化运行态库");
    (directory, store)
}

/// 登记一条会话；`title` 与 `trust_scope` 由调用方给出，覆盖回落的两个分支。
fn seed_session(
    store: &SqliteRuntimeStore,
    session_id: &str,
    title: Option<&str>,
    trust_scope: &str,
    workspace_id: Option<&str>,
    pinned: i64,
    archived: i64,
    last_activity_at_ms: i64,
) {
    store
        .with_read_connection(|connection| {
            connection
                .execute(
                    "INSERT INTO sessions(session_id,current_run_id,root_session_id,trust_scope,kind,
                     cwd,status,worker_attempt,context_version,last_checkpoint_seq,last_event_sequence,
                     last_activity_at_ms,created_at_ms,updated_at_ms,pinned,archived,workspace_id,title,
                     provider,model,model_name)
                     VALUES(?1,?1,NULL,?2,'primary','/workspace','idle',0,0,0,0,?3,?3,?3,?4,?5,?6,?7,
                     'provider-a','model-a','alias-a')",
                    params![
                        session_id,
                        trust_scope,
                        last_activity_at_ms,
                        pinned,
                        archived,
                        workspace_id,
                        title
                    ],
                )
                .map(|_| ())
        })
        .expect("写入会话");
}

/// 登记会话累计读数；直方图用来覆盖中位数与最大值两列。
fn seed_metrics(store: &SqliteRuntimeStore, session_id: &str, histogram_json: &str) {
    store
        .with_read_connection(|connection| {
            connection
                .execute(
                    "INSERT INTO session_metrics(session_id,turn_count,total_active_ms,ttft_max_ms,
                     ttft_histogram_json,jobs_running,updated_at_ms)
                     VALUES(?1,7,1234,900,?2,2,10)",
                    params![session_id, histogram_json],
                )
                .map(|_| ())
        })
        .expect("写入累计读数");
}

/// 一条入站事实：`client` 为空时渠道回落到 `source_channel`，与读取口径一致。
fn seed_inbox(
    store: &SqliteRuntimeStore,
    session_id: &str,
    interaction_id: &str,
    client: &str,
    source_channel: &str,
    scope_json: &str,
    reply_route_json: &str,
    content: &str,
    received_at_ms: i64,
    terminal_kind: Option<&str>,
) {
    store
        .with_read_connection(|connection| {
            connection
                .execute(
                    "INSERT INTO inbox(interaction_id,source_channel,client,source_message_id,
                     scope_json,audience_json,reply_route_json,content,session_id,received_at_ms,
                     updated_at_ms,terminal_kind)
                     VALUES(?1,?2,?3,?4,?5,'{}',?6,?7,?8,?9,?9,?10)",
                    params![
                        interaction_id,
                        source_channel,
                        client,
                        format!("msg-{interaction_id}"),
                        scope_json,
                        reply_route_json,
                        content,
                        session_id,
                        received_at_ms,
                        terminal_kind
                    ],
                )
                .map(|_| ())
        })
        .expect("写入入站事实");
}

/// 一条工作项；`inbox_id` 为空表示它不挂在入站事实上（目标执行的工作项就是这样）。
fn seed_work_item(
    store: &SqliteRuntimeStore,
    session_id: &str,
    execution_id: &str,
    inbox_id: Option<&str>,
    kind: &str,
    status: &str,
) {
    store
        .with_read_connection(|connection| {
            connection
                .execute(
                    "INSERT INTO work_items(execution_id,session_id,kind,payload_json,attempt,priority,
                     inbox_id,status,created_at_ms,updated_at_ms)
                     VALUES(?1,?2,?3,'{}',1,0,?4,?5,1,1)",
                    params![execution_id, session_id, kind, inbox_id, status],
                )
                .map(|_| ())
        })
        .expect("写入工作项");
}

/// 改造前的清单投影：一条 SQL 里七个逐会话相关子查询，保留原文作为等价基准。
fn legacy_rows(store: &SqliteRuntimeStore) -> Vec<Value> {
    store
        .with_read_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT s.session_id, s.cwd, s.status, s.pinned, s.archived, s.workspace_id, s.title,
                        s.last_activity_at_ms,
                        COALESCE(m.turn_count, 0), COALESCE(m.total_active_ms, 0),
                        COALESCE(m.ttft_max_ms, 0), COALESCE(m.jobs_running, 0),
                        m.ttft_histogram_json,
                        (SELECT i.content FROM inbox AS i WHERE i.session_id = s.session_id
                         ORDER BY i.received_at_ms ASC, i.interaction_id ASC LIMIT 1),
                        (SELECT i.scope_json FROM inbox AS i WHERE i.session_id = s.session_id
                         ORDER BY i.received_at_ms DESC, i.interaction_id DESC LIMIT 1),
                        COALESCE((SELECT CASE WHEN TRIM(i.client) = '' THEN i.source_channel ELSE i.client END
                                  FROM inbox AS i WHERE i.session_id = s.session_id
                                  ORDER BY i.received_at_ms ASC, i.interaction_id ASC LIMIT 1), ''),
                        COALESCE((SELECT json_extract(i.reply_route_json, '$.destination')
                                  FROM inbox AS i WHERE i.session_id = s.session_id
                                  ORDER BY i.received_at_ms ASC, i.interaction_id ASC LIMIT 1), ''),
                        COALESCE((SELECT CASE WHEN TRIM(i.client) = '' THEN i.source_channel ELSE i.client END
                                  FROM inbox AS i WHERE i.session_id = s.session_id
                                  ORDER BY i.received_at_ms DESC, i.interaction_id DESC LIMIT 1), ''),
                        s.trust_scope,
                        (SELECT COUNT(*) FROM inbox AS i WHERE i.session_id = s.session_id),
                        s.provider, s.model, s.model_name,
                        (SELECT COUNT(*) FROM inbox AS i JOIN work_items AS w ON w.inbox_id = i.interaction_id
                         WHERE i.session_id = s.session_id AND w.status = 'running')
                 FROM sessions AS s
                 LEFT JOIN session_metrics AS m ON m.session_id = s.session_id
                 WHERE s.kind = 'primary'
                 ORDER BY s.pinned DESC, s.last_activity_at_ms DESC, s.session_id ASC
                 LIMIT ?1",
            )?;
            let rows = statement.query_map([SESSION_SCAN_LIMIT], |row| {
                let cwd: String = row.get(1)?;
                let stored_workspace: Option<String> = row.get(5)?;
                let stored_title: Option<String> = row.get(6)?;
                let ttft_histogram_json: Option<String> = row.get(12)?;
                let first_content: Option<String> = row.get(13)?;
                let scope_json: Option<String> = row.get(14)?;
                let trust_scope: String = row.get(18)?;
                let workspace_id = stored_workspace
                    .filter(|value| !value.is_empty())
                    .or_else(|| scope_json.as_deref().and_then(super::workspace_of_scope));
                Ok(SessionListRow {
                    session_id: row.get(0)?,
                    title: super::display_session_title(stored_title, first_content, &cwd),
                    status: row.get(2)?,
                    pinned: row.get::<_, i64>(3)? != 0,
                    archived: row.get::<_, i64>(4)? != 0,
                    workspace_id,
                    last_activity_at_ms: u64::try_from(row.get::<_, i64>(7)?).unwrap_or(0),
                    turn_count: u64::try_from(row.get::<_, i64>(8)?).unwrap_or(0),
                    total_active_ms: u64::try_from(row.get::<_, i64>(9)?).unwrap_or(0),
                    ttft_max_ms: u64::try_from(row.get::<_, i64>(10)?).unwrap_or(0),
                    open_job_count: u64::try_from(row.get::<_, i64>(11)?).unwrap_or(0),
                    ttft_p50_ms: ttft_histogram_json
                        .as_deref()
                        .and_then(super::median_of_histogram),
                    channel: row.get(15)?,
                    destination: row.get(16)?,
                    last_channel: row.get(17)?,
                    audience: super::audience_label(&trust_scope),
                    interaction_count: u64::try_from(row.get::<_, i64>(19)?).unwrap_or(0),
                    provider: row.get::<_, Option<String>>(20)?.unwrap_or_default(),
                    model: row.get::<_, Option<String>>(21)?.unwrap_or_default(),
                    model_name: row.get(22)?,
                    running: row.get::<_, i64>(23)? > 0,
                    pending_question_count: 0,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })
        .expect("读取基准清单")
        .iter()
        .map(SessionListRow::to_json)
        .collect()
}

/// 同一份夹具下新读模型与改造前逐字段一致。
#[test]
fn read_model_matches_the_legacy_single_statement_projection() {
    let (_directory, store) = fixture();

    // 一条什么都没有的会话：标题回落到工作目录，渠道与去向留空。
    seed_session(&store, "session-bare", None, "private", None, 0, 0, 100);
    // 一条只有标题的会话：控制面标题优先于首条正文。
    seed_session(
        &store,
        "session-titled",
        Some("控制面标题"),
        "group_workspace",
        Some("ws-explicit"),
        1,
        0,
        900,
    );
    seed_metrics(
        &store,
        "session-titled",
        "[0,0,0,3,0,0,0,0,0,0,0,0,0,0,0,0]",
    );
    // 一条只有作用域的会话：Workspace 从末条入站的 scope_json 回落。
    seed_session(
        &store,
        "session-scoped",
        None,
        "restricted",
        None,
        0,
        1,
        800,
    );
    // 一条在跑的会话：两条入站事实，一条挂着在跑工作项。
    seed_session(
        &store,
        "session-running",
        None,
        "private",
        Some(""),
        0,
        0,
        700,
    );

    seed_inbox(
        &store,
        "session-titled",
        "inbox-titled-1",
        "",
        "desktop",
        "{\"workspace\":\"ws-scope\"}",
        "{\"destination\":\"window-1\"}",
        "  第一条标题\n第二行",
        10,
        None,
    );
    seed_inbox(
        &store,
        "session-titled",
        "inbox-titled-2",
        "client-b",
        "whatsapp",
        "{\"workspace\":\"ws-scope-later\"}",
        "{\"destination\":\"window-2\"}",
        "更晚的一条",
        20,
        None,
    );
    seed_inbox(
        &store,
        "session-scoped",
        "inbox-scoped-1",
        "",
        "tui",
        "{\"workspace\":\"ws-from-scope\"}",
        "{}",
        "从作用域回落工作区",
        30,
        None,
    );
    seed_inbox(
        &store,
        "session-running",
        "inbox-running-1",
        "",
        "desktop",
        "{}",
        "{\"destination\":\"window-9\"}",
        "在跑的那一轮",
        40,
        None,
    );
    seed_inbox(
        &store,
        "session-running",
        "inbox-running-2",
        "client-c",
        "slack",
        "{}",
        "{}",
        "更晚一条带客户端的入站",
        50,
        None,
    );
    seed_work_item(
        &store,
        "session-running",
        "exec-running",
        Some("inbox-running-2"),
        "turn",
        "running",
    );
    seed_work_item(
        &store,
        "session-running",
        "exec-done",
        Some("inbox-running-1"),
        "turn",
        "completed",
    );
    // 目标执行的工作项没有入站事实：它不参与「在跑」判定。
    seed_work_item(
        &store,
        "session-running",
        "exec-goal",
        None,
        "goal",
        "running",
    );
    // 派发轮已收口的排队工作项：原口径要求 `inbox.terminal_kind IS NULL` 才算排队。
    seed_inbox(
        &store,
        "session-running",
        "inbox-dispatch",
        "",
        "agent",
        "{}",
        "{}",
        "派发轮",
        41,
        Some("understood"),
    );
    seed_work_item(
        &store,
        "session-running",
        "exec-dispatch",
        Some("inbox-dispatch"),
        "turn",
        "queued",
    );

    let legacy = legacy_rows(&store);
    let current = read_session_rows(&store)
        .expect("读取新清单")
        .iter()
        .map(SessionListRow::to_json)
        .collect::<Vec<_>>();

    assert_eq!(legacy.len(), current.len(), "会话条数必须一致");
    for (index, (before, after)) in legacy.iter().zip(current.iter()).enumerate() {
        assert_eq!(
            before, after,
            "第 {index} 行投影不一致：改造前={before} 改造后={after}"
        );
    }
    // 关键字段独立断言：等价比较失败时也能直接读到口径。
    let running = current
        .iter()
        .find(|row| row["sessionId"] == "session-running")
        .expect("在跑的会话在清单里");
    assert_eq!(running["running"], true, "挂着在跑工作项的会话必须报运行态");
    assert_eq!(running["interactionCount"], 3, "入站条数按会话全量统计");
    assert_eq!(running["channel"], "desktop", "渠道取首条入站");
    assert_eq!(
        running["lastChannel"], "client-c",
        "末条渠道优先用客户端标识"
    );
    let scoped = current
        .iter()
        .find(|row| row["sessionId"] == "session-scoped")
        .expect("带作用域的会话在清单里");
    assert_eq!(
        scoped["workspaceId"], "ws-from-scope",
        "Workspace 从末条作用域回落"
    );
    let bare = current
        .iter()
        .find(|row| row["sessionId"] == "session-bare")
        .expect("空会话在清单里");
    assert_eq!(
        bare["title"], "/workspace",
        "没有标题与正文时回落到工作目录"
    );
    assert_eq!(bare["channel"], "", "没有入站事实时渠道为空串");
    assert_eq!(bare["interactionCount"], 0);
}

/// 同一份大规模夹具下，新读模型与改造前语句的耗时对比（性能验收，默认不跑）。
///
/// 入站事实按 200 会话 × 500 条建库；两侧各测三次取最小值。它锁的是「新读模型不得比
/// 原来那条语句慢」这条底线，同时证明会话索引仍在起作用：把 `inbox_session_recent`
/// 删掉之后两侧都会退化到全表扫描。
#[test]
#[ignore = "性能验收：同一夹具下比较新读模型与改造前语句的耗时"]
fn read_model_is_not_slower_than_the_legacy_statement() {
    const SESSIONS: usize = 200;
    const ROWS_PER_SESSION: usize = 500;
    let (_directory, store) = fixture();
    store
        .with_read_connection(|connection| {
            connection.execute_batch("BEGIN IMMEDIATE")?;
            for session in 0..SESSIONS {
                connection.execute(
                    "INSERT INTO sessions(session_id,current_run_id,root_session_id,trust_scope,kind,cwd,status,
                     worker_attempt,context_version,last_checkpoint_seq,last_event_sequence,
                     last_activity_at_ms,created_at_ms,updated_at_ms,pinned,archived,workspace_id)
                     VALUES(?1,?1,NULL,'private','primary','/workspace','idle',0,0,0,0,?2,?2,?2,0,0,'ws-a')",
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
        .expect("写入大规模 fixture");

    let mut best_legacy = u128::MAX;
    for _ in 0..3 {
        let started = std::time::Instant::now();
        let rows = legacy_rows(&store).len();
        best_legacy = best_legacy.min(started.elapsed().as_millis());
        assert_eq!(rows, SESSIONS, "基准条数");
    }
    let mut best_current = u128::MAX;
    for _ in 0..3 {
        let started = std::time::Instant::now();
        let rows = read_session_rows(&store).expect("读取新清单").len();
        best_current = best_current.min(started.elapsed().as_millis());
        assert_eq!(rows, SESSIONS, "新读模型条数");
    }
    println!(
        "session_list_perf sessions={SESSIONS} inbox_rows={} legacy={best_legacy}ms current={best_current}ms",
        SESSIONS * ROWS_PER_SESSION
    );
    assert!(
        best_current <= best_legacy,
        "新读模型不得慢于改造前：legacy={best_legacy}ms current={best_current}ms"
    );
}
