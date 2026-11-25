//! 窗口轮转边界：起跑占位轮的序号，以及「边界被写成 0」的存量快照。

use super::*;

/// 占位轮把换窗边界写成 0 的存量快照：0 等价于「无边界」，已归档轮次会被整段装回来。
///
/// 反向判据：摘掉 `effective_through_sequence` 的反推，这条用例立刻失败。
#[test]
fn zero_through_sequence_does_not_reload_archived_turns() {
    let records = vec![
        session_record("archived-user", "archived-assistant", 100),
        session_record("recent-user", "recent-assistant", 200),
    ];
    let mut detail = thread_detail_from_records(&records).unwrap();
    detail.compacted_history = Some(ThreadCompaction {
        history_json: serde_json::json!([
            {"role": "user", "content": "checkpoint", "cache_excluded": true}
        ])
        .to_string(),
        through_sequence: 0,
        checkpoint_text: "checkpoint".to_owned(),
        // 换窗发生在第二轮进行中：两轮都已起跑，只有第二轮留在窗口里。
        recorded_at_ms: 300,
    });

    let history = thread_model_history_json(&detail).unwrap();

    assert!(
        !history.contains("archived-user"),
        "已归档轮次不得重新装回窗口"
    );
    assert!(history.contains("recent-user"), "换窗那轮必须留在窗口里");
}

/// 首轮换窗后追加后续轮次：合法边界 0 不得被当成「无边界」而修正掉。
#[test]
fn zero_through_sequence_stays_valid_when_the_first_turn_opened_the_window() {
    let records = vec![
        session_record("first-user", "first-assistant", 100),
        session_record("later-user", "later-assistant", 300),
    ];
    let mut detail = thread_detail_from_records(&records).unwrap();
    detail.compacted_history = Some(ThreadCompaction {
        history_json: serde_json::json!([
            {"role": "user", "content": "checkpoint", "cache_excluded": true}
        ])
        .to_string(),
        through_sequence: 0,
        checkpoint_text: "checkpoint".to_owned(),
        // 换窗发生在第一轮进行中：第一轮是首个可见轮次，后续轮次本来就该可见。
        recorded_at_ms: 150,
    });

    let history = thread_model_history_json(&detail).unwrap();

    assert!(
        history.contains("first-user"),
        "首轮换窗时第一轮必须保留，不得被当成无边界修正掉"
    );
    assert!(
        history.contains("later-user"),
        "换窗之后产生的轮次必须继续可见"
    );
}

/// 整条 thread 只有一轮时，边界 0 是合法值：那一轮自己就是首个可见轮次。
#[test]
fn zero_through_sequence_stays_valid_for_single_turn_thread() {
    let record = session_record("only-user", "only-assistant", 100);
    let mut detail = thread_detail_from_records(&[record]).unwrap();
    detail.compacted_history = Some(ThreadCompaction {
        history_json: serde_json::json!([
            {"role": "user", "content": "checkpoint", "cache_excluded": true}
        ])
        .to_string(),
        through_sequence: 0,
        checkpoint_text: "checkpoint".to_owned(),
        recorded_at_ms: 200,
    });

    let history = thread_model_history_json(&detail).unwrap();

    assert!(
        history.contains("only-user"),
        "单轮会话的边界 0 必须保留该轮"
    );
}

/// 换窗安装的边界必须等于本轮收口时会拿到的轮序，而不是占位轮上的 0。
///
/// 反向判据：把 `install_active_compaction_projection` 改回 `current.sequence` 即失败。
#[test]
fn compaction_projection_writes_the_sequence_the_turn_will_close_with() {
    let root = temp_root("compaction-boundary");
    std::fs::create_dir_all(&root).unwrap();
    let service = ThreadService::new(JsonlThreadEventStore);
    let thread_id = "thread-boundary";
    let record = session_record("历史提问", "历史回答", 400);
    service
        .start_empty_thread(
            &root,
            StartEmptyThreadCommand {
                thread_id: thread_id.to_owned(),
                session_root_id: thread_id.to_owned(),
                parent_thread_id: None,
                forked_from_thread_id: None,
                source: ThreadSource::CliRun,
                ephemeral: false,
                settings: settings_from_record(&record),
                created_at_ms: 400,
            },
        )
        .unwrap();

    // 两条已收口轮次：序号 0、1。
    for sequence in 0..2_u64 {
        let turn = Turn {
            turn_id: format!("{thread_id}-turn-assistant:{sequence}"),
            ..turn_from_record(&record, sequence).unwrap()
        };
        service
            .append_turn_at_tail(
                &root,
                AppendTurnCommand {
                    thread_id: thread_id.to_owned(),
                    recorded_at_ms: 500 + sequence,
                    turn,
                },
            )
            .unwrap();
    }

    // 起跑占位轮：序号恒为 0，与已收口的 Turn 0 撞号。
    let placeholder = Turn {
        turn_id: format!("{thread_id}-turn-assistant-job-1"),
        status: TurnStatus::Active,
        ..turn_from_record(&record, 0).unwrap()
    };
    service
        .append_turn_at_tail(
            &root,
            AppendTurnCommand {
                thread_id: thread_id.to_owned(),
                recorded_at_ms: 600,
                turn: placeholder,
            },
        )
        .unwrap();

    let expected_sequence = thread_event_sequence_snapshot(&root, thread_id).unwrap();
    let through_sequence = install_active_compaction_projection(
        &root,
        thread_id,
        "assistant-job-1",
        expected_sequence,
        ThreadCompaction {
            history_json: "[]".to_owned(),
            through_sequence: 0,
            checkpoint_text: "checkpoint".to_owned(),
            recorded_at_ms: 700,
        },
    )
    .unwrap();

    assert_eq!(
        through_sequence, 2,
        "边界必须是本轮收口后的轮序（max+1），而不是占位轮的 0"
    );
}

/// 起跑占位轮（序号恒为 0）不参与轮序计算：换窗边界不会因此退化成 0。
///
/// 这是 `next_sequence` 自身的口径；接线层的反向判据在
/// `compaction_projection_writes_the_sequence_the_turn_will_close_with`。
#[test]
fn next_sequence_skips_the_placeholder_turn() {
    let records = vec![
        session_record("第一轮", "答完了", 100),
        session_record("第二轮", "答完了", 200),
    ];
    let mut detail = thread_detail_from_records(&records).unwrap();
    assert_eq!(detail.turns.len(), 2);
    assert_eq!(detail.turns[1].sequence, 1);

    let mut placeholder = detail.turns[0].clone();
    placeholder.turn_id = "thread-turn-placeholder".to_owned();
    placeholder.sequence = 0;
    detail.turns.push(placeholder);

    assert_eq!(
        super::super::mirror_turn::next_sequence(&detail, Some("thread-turn-placeholder")),
        2,
        "占位轮必须被排除，边界取本轮收口后的轮序"
    );
}
