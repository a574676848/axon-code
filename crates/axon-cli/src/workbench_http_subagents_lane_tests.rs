//! 名册只列当前车道子代理的回归测试：别的车道的子会话不能混进这一页。
//!
//! 名册的候选集合改成按 `sessions` 的 `(kind, root_session_id)` 索引取之后，「谁挂在这条
//! 车道下」由控制面登记决定，thread 侧只负责补标题、模型与末轮状态。这里锁住三件事：
//! 车道之间不串行、内务子代理不进名册、thread 事实按会话自己的运行实例定向读取。

use std::path::Path;

use axon_domain::DaemonSessionId;
use axon_session::{
    JsonlThreadEventStore, NameThreadCommand, NewSessionRequest, RuntimeStore, SessionKind,
    SessionRuntimeRecord, SessionTrustScope, StartEmptyThreadCommand, StartTurnCommand,
    ThreadService, ThreadSettings, ThreadSource, Turn, TurnStatus, new_session_id,
};

use super::session_subagents_payload;
use crate::workbench_http_adapter::runtime_store;

/// 建一个临时运行态根；会话库落在它的 data 子目录，与生产路径一致。
fn fixture() -> (axon_test_support::TestTempDir, std::path::PathBuf) {
    let directory = axon_test_support::test_temp_dir();
    let root = directory.path().to_path_buf();
    (directory, root)
}

/// 登记一条主会话（车道）。
fn lane_session(root: &Path) -> SessionRuntimeRecord {
    let store = runtime_store(root);
    store
        .create_session(&new_session_id(), ".", 10)
        .expect("主会话")
}

/// 登记一条挂在车道下的子代理会话。
fn child_session(
    root: &Path,
    parent: &DaemonSessionId,
    agent_path: &str,
    now_ms: u64,
) -> SessionRuntimeRecord {
    let store = runtime_store(root);
    store
        .create_session_with(&NewSessionRequest {
            session_id: new_session_id(),
            kind: SessionKind::Subagent,
            trust_scope: SessionTrustScope::Private,
            workspace_id: None,
            root_session_id: Some(parent.clone()),
            agent_path: Some(agent_path.to_owned()),
            goal_id: None,
            execution_id: None,
            cwd: ".".to_owned(),
            now_ms,
        })
        .expect("子代理会话")
}

/// 给子会话写一条 thread：带标题与模型，可选留一轮 Active。
fn child_thread(root: &Path, thread_id: &str, name: &str, with_active_turn: bool) {
    let service = ThreadService::new(JsonlThreadEventStore);
    service
        .start_empty_thread(
            root,
            StartEmptyThreadCommand {
                thread_id: thread_id.to_owned(),
                session_root_id: thread_id.to_owned(),
                parent_thread_id: None,
                forked_from_thread_id: None,
                source: ThreadSource::CliRun,
                ephemeral: false,
                settings: ThreadSettings {
                    cwd: ".".to_owned(),
                    provider: "fixture".to_owned(),
                    model: "fixture-chat".to_owned(),
                    selected_skill_name: None,
                },
                created_at_ms: 10,
            },
        )
        .expect("建 thread");
    service
        .name_thread(
            root,
            NameThreadCommand {
                thread_id: thread_id.to_owned(),
                recorded_at_ms: 11,
                name: Some(name.to_owned()),
            },
        )
        .expect("命名 thread");
    if with_active_turn {
        service
            .start_turn(
                root,
                StartTurnCommand {
                    thread_id: thread_id.to_owned(),
                    recorded_at_ms: 12,
                    turn: Turn {
                        turn_id: format!("{thread_id}-turn-1"),
                        sequence: 0,
                        status: TurnStatus::Active,
                        history_items: Vec::new(),
                        token_usage_json: None,
                        started_at_ms: 12,
                        completed_at_ms: None,
                        interrupted_at_ms: None,
                        failure: None,
                    },
                },
            )
            .expect("写 Active 轮次");
    }
}

/// 名册只列本车道的子代理，thread 侧事实按子会话自己的运行实例取。
#[test]
fn roster_lists_only_the_lane_children_with_their_thread_facts() {
    let (_directory, root) = fixture();
    let lane = lane_session(&root);
    let other_lane = lane_session(&root);
    let first = child_session(&root, &lane.session_id, "/root/first", 10);
    let second = child_session(&root, &lane.session_id, "/root/second", 20);
    let foreign = child_session(&root, &other_lane.session_id, "/root/foreign", 30);
    child_thread(&root, &first.current_run_id, "第一条子代理", true);
    child_thread(&root, &second.current_run_id, "第二条子代理", false);
    child_thread(&root, &foreign.current_run_id, "别的车道的子代理", true);

    let payload = session_subagents_payload(&root, lane.session_id.as_str()).expect("名册载荷");
    assert_eq!(
        payload["runSessionId"].as_str(),
        Some(lane.current_run_id.as_str()),
        "载荷回显本车道的运行实例标识"
    );
    let subagents = payload["subagents"].as_array().expect("子代理数组");
    assert_eq!(subagents.len(), 2, "只列本车道的两条子代理");
    let listed = subagents
        .iter()
        .map(|row| row["sessionId"].as_str().unwrap_or_default().to_owned())
        .collect::<Vec<_>>();
    assert!(
        !listed.contains(&foreign.session_id.as_str().to_owned()),
        "别的车道的子代理不得出现在本车道名册里：{listed:?}"
    );

    let first_row = subagents
        .iter()
        .find(|row| row["sessionId"].as_str() == Some(first.session_id.as_str()))
        .expect("第一条子代理在名册里");
    assert_eq!(
        first_row["title"].as_str(),
        Some("第一条子代理"),
        "标题取自子会话自己的 thread"
    );
    assert_eq!(first_row["model"].as_str(), Some("fixture-chat"));
    assert_eq!(first_row["provider"].as_str(), Some("fixture"));
    assert_eq!(first_row["agentId"].as_str(), Some("subagent:root/first"));
    assert_eq!(first_row["slug"].as_str(), Some("root_first"));
    assert_eq!(first_row["hasRuntime"], true);
    assert_eq!(
        first_row["threadTurnStatus"].as_str(),
        Some("active"),
        "thread 侧末轮状态如实透出"
    );
    assert_eq!(first_row["lastTurnAtMs"].as_u64(), Some(12));
    assert_eq!(
        first_row["status"].as_str(),
        Some("idle"),
        "没有派发轮就是空闲"
    );

    let second_row = subagents
        .iter()
        .find(|row| row["sessionId"].as_str() == Some(second.session_id.as_str()))
        .expect("第二条子代理在名册里");
    assert_eq!(second_row["lastTurnAtMs"], serde_json::Value::Null);
    assert_eq!(second_row["threadTurnStatus"], serde_json::Value::Null);
    assert_eq!(
        second_row["updatedAtMs"].as_u64(),
        Some(11),
        "更新时间取 thread 侧"
    );
}

/// 运行侧内务子代理不进名册：它们没有可进入的逐字稿。
#[test]
fn internal_agent_paths_are_not_listed() {
    let (_directory, root) = fixture();
    let lane = lane_session(&root);
    let internal = child_session(&root, &lane.session_id, "/self_iteration/reflect", 10);
    let visible = child_session(&root, &lane.session_id, "/root/visible", 20);
    child_thread(&root, &internal.current_run_id, "内务", true);
    child_thread(&root, &visible.current_run_id, "可见", true);

    let payload = session_subagents_payload(&root, lane.session_id.as_str()).expect("名册载荷");
    let subagents = payload["subagents"].as_array().expect("子代理数组");
    assert_eq!(subagents.len(), 1, "内务子代理不得出现在名册里");
    assert_eq!(
        subagents[0]["sessionId"].as_str(),
        Some(visible.session_id.as_str())
    );
}

/// 车道没有登记过运行会话：返回空名册，不报错。
#[test]
fn unknown_lane_returns_an_empty_roster() {
    let (_directory, root) = fixture();
    // 生产路径上库一定已经被建过：这里先把 schema 建出来，再问一条不存在的车道。
    runtime_store(&root).initialize().expect("初始化运行态库");
    let payload = session_subagents_payload(&root, "01J8Z0000000000000000000ZZ").expect("名册载荷");
    assert_eq!(payload["runSessionId"], serde_json::Value::Null);
    assert_eq!(payload["subagents"].as_array().map(Vec::len), Some(0));
}
