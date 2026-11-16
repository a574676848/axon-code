//! 子代理名册与来源标注测试：关系、状态与末轮时间全部来自 thread 事件面。

use std::path::PathBuf;

use axon_session::{
    AppendTurnCommand, FailTurnCommand, JsonlThreadEventStore, StartEmptyThreadCommand,
    StartTurnCommand, ThreadService, ThreadSettings, ThreadSource, Turn, TurnStatus,
};

use super::{
    CHILD_SESSION_ORIGIN_DELEGATED, CHILD_SESSION_ORIGIN_INTERNAL, child_session_rows,
    record_child_origin,
};

fn temp_root(tag: &str) -> PathBuf {
    axon_test_support::test_temp_root().join(format!(
        "axon-thread-children-{tag}-{}-{}",
        std::process::id(),
        axon_time::unix_now_ms()
    ))
}

fn service() -> ThreadService<JsonlThreadEventStore> {
    ThreadService::new(JsonlThreadEventStore)
}

/// 建一条 thread 并写一轮带终态的轮次；`status` 决定这一轮是完成、失败还是中断。
fn child_thread(
    root: &std::path::Path,
    thread_id: &str,
    parent: Option<&str>,
    started_at_ms: u64,
    status: TurnStatus,
) {
    let service = service();
    service
        .start_empty_thread(
            root,
            StartEmptyThreadCommand {
                thread_id: thread_id.to_owned(),
                session_root_id: parent.unwrap_or(thread_id).to_owned(),
                parent_thread_id: parent.map(ToOwned::to_owned),
                forked_from_thread_id: None,
                source: ThreadSource::CliRun,
                ephemeral: false,
                settings: ThreadSettings {
                    cwd: ".".to_owned(),
                    provider: "fixture".to_owned(),
                    model: "fixture-chat".to_owned(),
                    selected_skill_name: None,
                },
                created_at_ms: started_at_ms,
            },
        )
        .unwrap();
    service
        .start_turn(
            root,
            StartTurnCommand {
                thread_id: thread_id.to_owned(),
                recorded_at_ms: started_at_ms,
                turn: fixture_turn(thread_id, started_at_ms, TurnStatus::Active),
            },
        )
        .unwrap();
    match status {
        TurnStatus::Active => {}
        TurnStatus::Completed => service
            .append_turn(
                root,
                AppendTurnCommand {
                    thread_id: thread_id.to_owned(),
                    recorded_at_ms: started_at_ms,
                    turn: fixture_turn(thread_id, started_at_ms, TurnStatus::Completed),
                },
            )
            .unwrap(),
        TurnStatus::Failed => service
            .fail_turn(
                root,
                FailTurnCommand {
                    thread_id: thread_id.to_owned(),
                    turn_id: fixture_turn_id(thread_id),
                    failure: axon_domain::RuntimeFailure::new(
                        axon_domain::RUNTIME_FAILURE_CATEGORY_RUNTIME,
                        axon_domain::RUNTIME_FAILURE_CODE_RUNTIME_FAILED,
                        "子代理执行失败",
                        started_at_ms,
                    ),
                    recorded_at_ms: started_at_ms,
                },
            )
            .unwrap(),
        TurnStatus::Interrupted => service
            .interrupt_turn(
                root,
                axon_session::InterruptTurnCommand {
                    thread_id: thread_id.to_owned(),
                    turn_id: fixture_turn_id(thread_id),
                    recorded_at_ms: started_at_ms,
                },
            )
            .unwrap(),
    }
}

fn fixture_turn_id(thread_id: &str) -> String {
    format!("{thread_id}-turn-1")
}

fn fixture_turn(thread_id: &str, started_at_ms: u64, status: TurnStatus) -> Turn {
    Turn {
        turn_id: fixture_turn_id(thread_id),
        sequence: 0,
        status,
        history_items: Vec::new(),
        token_usage_json: None,
        started_at_ms,
        completed_at_ms: (status == TurnStatus::Completed).then_some(started_at_ms),
        interrupted_at_ms: None,
        failure: None,
    }
}

#[test]
fn children_are_listed_by_parent_and_ordered_by_recent_activity() {
    let root = temp_root("order");
    child_thread(&root, "run-root", None, 100, TurnStatus::Completed);
    child_thread(
        &root,
        "run-root--subagent--older",
        Some("run-root"),
        200,
        TurnStatus::Completed,
    );
    child_thread(
        &root,
        "run-root--subagent--newer",
        Some("run-root"),
        300,
        TurnStatus::Completed,
    );

    let rows = child_session_rows(&root, "run-root", 10).unwrap();
    assert_eq!(rows.len(), 2, "父会话直接派生的子会话必须全部列出");
    assert_eq!(rows[0].session_id, "run-root--subagent--newer");
    assert_eq!(rows[1].session_id, "run-root--subagent--older");
    assert_eq!(
        child_session_rows(&root, "other-root", 10).unwrap().len(),
        0,
        "别的会话的子代理不得串进这条车道"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn last_turn_status_and_time_come_from_the_thread_turn() {
    let root = temp_root("status");
    child_thread(
        &root,
        "run-a--subagent--active",
        Some("run-a"),
        100,
        TurnStatus::Active,
    );
    child_thread(
        &root,
        "run-a--subagent--failed",
        Some("run-a"),
        200,
        TurnStatus::Failed,
    );
    child_thread(
        &root,
        "run-a--subagent--interrupted",
        Some("run-a"),
        300,
        TurnStatus::Interrupted,
    );

    let rows = child_session_rows(&root, "run-a", 10).unwrap();
    assert_eq!(rows.len(), 3);
    let status_of = |session: &str| {
        rows.iter()
            .find(|row| row.session_id == session)
            .and_then(|row| row.status.clone())
    };
    assert_eq!(
        status_of("run-a--subagent--active").as_deref(),
        Some("active"),
        "起跑后名册必须读作在跑"
    );
    assert_eq!(
        status_of("run-a--subagent--failed").as_deref(),
        Some("failed")
    );
    assert_eq!(
        status_of("run-a--subagent--interrupted").as_deref(),
        Some("interrupted")
    );
    assert!(
        rows.iter().all(|row| row.last_turn_at_ms.is_some()),
        "末轮时间必须由轮次事实给出"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn roster_respects_the_limit_and_keeps_the_most_recent() {
    let root = temp_root("limit");
    for index in 0..3_u64 {
        child_thread(
            &root,
            &format!("run-b--subagent--{index}"),
            Some("run-b"),
            (index + 1) * 100,
            TurnStatus::Completed,
        );
    }
    let rows = child_session_rows(&root, "run-b", 2).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].session_id, "run-b--subagent--2");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn threads_without_turns_read_as_unknown_status() {
    let root = temp_root("no-turn");
    service()
        .start_empty_thread(
            &root,
            StartEmptyThreadCommand {
                thread_id: "run-c--subagent--empty".to_owned(),
                session_root_id: "run-c".to_owned(),
                parent_thread_id: Some("run-c".to_owned()),
                forked_from_thread_id: None,
                source: ThreadSource::CliRun,
                ephemeral: false,
                settings: ThreadSettings {
                    cwd: ".".to_owned(),
                    provider: "fixture".to_owned(),
                    model: "fixture-chat".to_owned(),
                    selected_skill_name: None,
                },
                created_at_ms: 100,
            },
        )
        .unwrap();
    let rows = child_session_rows(&root, "run-c", 10).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, None, "没有轮次时状态未知，不猜成已完成");
    assert_eq!(rows[0].last_turn_at_ms, None);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn internal_governance_children_are_not_part_of_the_roster() {
    let root = temp_root("origin");
    child_thread(&root, "run-d", None, 100, TurnStatus::Completed);
    child_thread(
        &root,
        "run-d--subagent--delegate",
        Some("run-d"),
        200,
        TurnStatus::Completed,
    );
    record_child_origin(
        &root.join("config.toml"),
        "run-d--subagent--delegate",
        CHILD_SESSION_ORIGIN_DELEGATED,
    )
    .unwrap();
    child_thread(
        &root,
        "run-d--subagent--salience",
        Some("run-d"),
        300,
        TurnStatus::Completed,
    );
    record_child_origin(
        &root.join("config.toml"),
        "run-d--subagent--salience",
        CHILD_SESSION_ORIGIN_INTERNAL,
    )
    .unwrap();

    let rows = child_session_rows(&root, "run-d", 10).unwrap();
    assert_eq!(rows.len(), 1, "内务子代理不得出现在会话的子代理名册里");
    assert_eq!(rows[0].session_id, "run-d--subagent--delegate");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn children_without_origin_are_treated_as_delegated() {
    let root = temp_root("origin-null");
    child_thread(
        &root,
        "run-f--subagent--unknown",
        Some("run-f"),
        100,
        TurnStatus::Completed,
    );
    // 旧记录没有来源：按用户委派处理，宁可多列一条也不静默丢掉真实的子代理。
    assert_eq!(child_session_rows(&root, "run-f", 10).unwrap().len(), 1);
    let _ = std::fs::remove_dir_all(root);
}
