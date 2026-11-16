//! 子代理名册读模型测试：会话状态写入与按父会话的索引查询。

use super::super::RolloutSqliteStore;
use super::{cleanup, record, temp_db};

#[test]
fn child_sessions_are_listed_by_parent_and_ordered_by_recent_activity() {
    let path = temp_db("child-sessions");
    let store = RolloutSqliteStore::open(&path).unwrap();
    store
        .record_session(&record("lane-run", None, 100))
        .unwrap();
    store
        .commit_session(
            &record("lane-run--subagent--root_older", Some("lane-run"), 200),
            Some("旧子代理"),
        )
        .unwrap();
    store
        .commit_session(
            &record("lane-run--subagent--root_newer", Some("lane-run"), 300),
            Some("新子代理"),
        )
        .unwrap();

    let rows = store.select_child_sessions("lane-run", 10).unwrap();
    assert_eq!(rows.len(), 2, "父会话直接派生的子会话必须全部列出");
    assert_eq!(rows[0].session_id, "lane-run--subagent--root_newer");
    assert_eq!(rows[0].title.as_deref(), Some("新子代理"));
    assert_eq!(rows[1].session_id, "lane-run--subagent--root_older");
    assert_eq!(
        store.select_child_sessions("other-run", 10).unwrap().len(),
        0,
        "别的会话的子代理不得串进这条车道"
    );
    cleanup(&path);
}

#[test]
fn child_session_status_survives_reopen_and_late_writes_do_not_rewind_time() {
    let path = temp_db("child-status");
    let store = RolloutSqliteStore::open(&path).unwrap();
    store
        .commit_session(
            &record("run-a--subagent--root_fast_1", Some("run-a"), 100),
            Some("冒烟测试"),
        )
        .unwrap();
    store
        .mark_session_status("run-a--subagent--root_fast_1", "active", 150)
        .unwrap();
    assert_eq!(
        store
            .select_child_sessions("run-a", 10)
            .unwrap()
            .first()
            .and_then(|row| row.status.clone()),
        Some("active".to_owned()),
        "起跑时名册必须读作在跑"
    );

    // 迟到的终态写入带更早的时间：状态要更新，最近活动时间不得往回拨。
    store
        .mark_session_status("run-a--subagent--root_fast_1", "interrupted", 120)
        .unwrap();
    cleanup(&path);

    let reopened = RolloutSqliteStore::open(&path).unwrap();
    let row = reopened
        .select_child_sessions("run-a", 10)
        .unwrap()
        .into_iter()
        .next()
        .expect("重开后名册仍在");
    assert_eq!(row.status.as_deref(), Some("interrupted"));
    assert_eq!(row.updated_at_ms, 150, "最近活动时间取较大值");
}

#[test]
fn child_sessions_respect_the_limit_and_report_last_turn_time() {
    let path = temp_db("child-limit");
    let store = RolloutSqliteStore::open(&path).unwrap();
    for index in 0..3_u64 {
        let child = record(
            &format!("run-b--subagent--root_{index}"),
            Some("run-b"),
            (index + 1) * 100,
        );
        store.record_session(&child).unwrap();
    }

    assert_eq!(store.select_child_sessions("run-b", 2).unwrap().len(), 2);
    let row = store
        .select_child_sessions("run-b", 10)
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    assert_eq!(
        row.last_turn_at_ms,
        Some(300),
        "最后一轮时间取该子会话最新一轮的记录时间"
    );
    cleanup(&path);
}

#[test]
fn missing_status_column_reads_as_unknown_instead_of_failed() {
    let path = temp_db("child-legacy");
    let store = RolloutSqliteStore::open(&path).unwrap();
    let child = record("run-c--subagent--root_legacy", Some("run-c"), 100);
    store.record_session(&child).unwrap();
    // 旧库没有状态列的历史行：读作未知，调用方不据此判成已完成或失败。
    let rows = store.select_child_sessions("run-c", 10).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, None);
    cleanup(&path);
}

#[test]
fn internal_governance_children_are_not_part_of_the_roster() {
    let path = temp_db("child-origin");
    let store = RolloutSqliteStore::open(&path).unwrap();
    store.record_session(&record("run-d", None, 100)).unwrap();
    // 用户可见的委派。
    store
        .commit_session(
            &record("run-d--subagent--root_delegate_fast_1", Some("run-d"), 200),
            Some("委派子代理"),
        )
        .unwrap();
    store
        .mark_session_origin("run-d--subagent--root_delegate_fast_1", "delegated")
        .unwrap();
    // 轮后治理自己派出去的语义显著度子任务。
    store
        .commit_session(
            &record(
                "run-d--subagent--root_self_iteration_semantic_salience",
                Some("run-d"),
                300,
            ),
            Some("评估以下有界事实包："),
        )
        .unwrap();
    store
        .mark_session_origin(
            "run-d--subagent--root_self_iteration_semantic_salience",
            "internal",
        )
        .unwrap();

    let rows = store.select_child_sessions("run-d", 10).unwrap();
    assert_eq!(rows.len(), 1, "内务子代理不得出现在会话的子代理名册里");
    assert_eq!(rows[0].session_id, "run-d--subagent--root_delegate_fast_1");
    cleanup(&path);
}

#[test]
fn legacy_internal_children_are_backfilled_at_open() {
    let path = temp_db("child-origin-backfill");
    {
        let store = RolloutSqliteStore::open(&path).unwrap();
        store.record_session(&record("run-e", None, 100)).unwrap();
        // 这一行写于补列之前：没有 origin，靠补列时的回填把它标成内部。
        store
            .record_session(&record(
                "run-e--subagent--root_self_iteration_memory_update",
                Some("run-e"),
                200,
            ))
            .unwrap();
    }
    // 重新打开会跑补列与回填。
    let reopened = RolloutSqliteStore::open(&path).unwrap();
    assert_eq!(
        reopened.select_child_sessions("run-e", 10).unwrap().len(),
        0,
        "补列回填后，旧内务会话同样不进名册"
    );
    cleanup(&path);
}

#[test]
fn children_without_origin_are_treated_as_delegated() {
    let path = temp_db("child-origin-null");
    let store = RolloutSqliteStore::open(&path).unwrap();
    store
        .record_session(&record("run-f--subagent--root_unknown", Some("run-f"), 100))
        .unwrap();
    // 旧记录没有来源：按用户委派处理，宁可多列一条也不静默丢掉真实的子代理。
    assert_eq!(store.select_child_sessions("run-f", 10).unwrap().len(), 1);
    cleanup(&path);
}
