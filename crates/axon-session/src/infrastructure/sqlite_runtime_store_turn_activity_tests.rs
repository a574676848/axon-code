//! 逐轮事实投影：收口批次一次投影出一行，读侧按交互主键取回。
//!
//! 固定三件事：终态里的答案与指标同批落一行、缺行按缺失返回（由读侧回落尾扫）、
//! 同一轮重复投影只更新不重复插入。

use std::sync::atomic::{AtomicU64, Ordering};

use axon_domain::{DaemonSessionId, InteractionId};

use super::SqliteRuntimeStore;
use crate::ports::RuntimeStore as _;

static FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn fixture() -> (SqliteRuntimeStore, DaemonSessionId) {
    let directory = axon_test_support::test_temp_path(&format!(
        "turn-activity-{}",
        FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let store = SqliteRuntimeStore::new(directory.join("runtime.sqlite3"));
    store.initialize().expect("初始化");
    let session_id = DaemonSessionId::new("session-turn-activity").expect("session id");
    store
        .create_session(&session_id, ".", 10)
        .expect("create session");
    (store, session_id)
}

fn terminal(interaction_id: &str, answer: &str) -> (String, String) {
    (
        "turn_completed".to_owned(),
        serde_json::json!({
            "interactionId": interaction_id,
            "response": answer,
            "status": "completed",
        })
        .to_string(),
    )
}

fn metrics(interaction_id: &str) -> (String, String) {
    (
        "turn_metrics".to_owned(),
        serde_json::json!({
            "interactionId": interaction_id,
            "activeMs": 1_234,
            "inputTokens": 10,
            "outputTokens": 20,
            "toolCalls": 1,
        })
        .to_string(),
    )
}

#[test]
fn terminal_batch_projects_answer_and_metrics_into_one_row() {
    let (store, session_id) = fixture();
    let interaction_id = InteractionId::new("01TURNACTIVITY000000000001").expect("interaction id");
    let events = [
        terminal(interaction_id.as_str(), "  收口答案  "),
        metrics(interaction_id.as_str()),
    ];
    super::turn_activity_projection::project_turn_activity(&store, &session_id, &events, 42)
        .expect("投影必须成功");

    let found = store
        .turn_activities_for(session_id.as_str(), &[interaction_id.as_str()])
        .expect("读投影");
    let row = found.get(interaction_id.as_str()).expect("必须有一行");
    assert_eq!(
        row.answer_text.as_deref(),
        Some("收口答案"),
        "答案应去首尾空白"
    );
    assert!(row.metrics_json.is_some(), "读数与终态同批落库");
}

#[test]
fn repeated_projection_updates_in_place_and_missing_reads_as_absent() {
    let (store, session_id) = fixture();
    let interaction_id = InteractionId::new("01TURNACTIVITY000000000002").expect("interaction id");
    let events = [
        terminal(interaction_id.as_str(), "第一次"),
        metrics(interaction_id.as_str()),
    ];
    super::turn_activity_projection::project_turn_activity(&store, &session_id, &events, 43)
        .expect("投影必须成功");
    // 同一轮再投影一次（重放、重试）：只更新，不新增行。
    super::turn_activity_projection::project_turn_activity(&store, &session_id, &events, 44)
        .expect("重放必须成功");

    let found = store
        .turn_activities_for(session_id.as_str(), &[interaction_id.as_str(), "01UNKNOWN"])
        .expect("读投影");
    assert_eq!(found.len(), 1, "同一轮只能有一行");
    assert!(!found.contains_key("01UNKNOWN"), "缺行必须显式缺失");
}
