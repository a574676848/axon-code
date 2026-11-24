//! 收口打偏与多轮残留的回归：终态必须落在本轮，残留必须能被全部收口。
//!
//! 现场形态：历史里躺着两条以上永不收口的 `Active` 轮次，收口却按「第一个 Active」
//! 反写，于是本轮终态落到别人的轮次上，而占着激活槽的残留永远清不掉，最后表现为
//! `start_turn` 一直拒绝、界面提示「本轮被后端拒绝，可重新发送」。

use super::*;
use axon_session::{ThreadEvent, ThreadEventRecord};

/// 收口写在本轮自己的 turn_id 上；随后批量修复能把更早的残留一并清掉。
#[test]
fn mirror_keeps_terminal_on_its_own_turn_and_repair_clears_every_residual() {
    let root = temp_root("mirror-no-drift");
    std::fs::create_dir_all(&root).unwrap();
    let service = ThreadService::new(JsonlThreadEventStore);

    // 第一条残留：模拟上一进程被强杀，轮次停在 Active（未完成的工具调用是 Active 的来源）。
    let mut stale = session_record("旧轮", "", 400);
    stale.assistant_part_id = "assistant:session-1:stale".to_owned();
    stale.tool_calls = vec![ProviderToolCall::new(
        "call-stale",
        "builtin.read_file",
        r#"{"path":"Cargo.toml"}"#,
    )];
    stale.turn_events = vec![tool_started_event("tool:call-stale", "call-stale")];
    let stale_turn = turn_from_record(&stale, 0).unwrap();
    assert_eq!(stale_turn.status, axon_session::TurnStatus::Active);
    service
        .start_empty_thread(
            &root,
            StartEmptyThreadCommand {
                thread_id: stale.session_id.clone(),
                session_root_id: stale.session_id.clone(),
                parent_thread_id: None,
                forked_from_thread_id: None,
                source: ThreadSource::CliRun,
                ephemeral: false,
                settings: settings_from_record(&stale),
                created_at_ms: stale.created_at_ms,
            },
        )
        .unwrap();
    service
        .start_turn(
            &root,
            StartTurnCommand {
                thread_id: stale.session_id.clone(),
                recorded_at_ms: stale.created_at_ms,
                turn: stale_turn.clone(),
            },
        )
        .unwrap();

    // 第二条残留：不经过 start_turn 校验，直接写事件流，复现「历史上叠了两轮 Active」。
    let mut drift = session_record("中间轮", "", 450);
    drift.assistant_part_id = "assistant:session-1:drift".to_owned();
    drift.tool_calls = vec![ProviderToolCall::new(
        "call-drift",
        "builtin.read_file",
        r#"{"path":"Cargo.toml"}"#,
    )];
    drift.turn_events = vec![tool_started_event("tool:call-drift", "call-drift")];
    let drift_turn = turn_from_record(&drift, 0).unwrap();
    let schema_version = JsonlThreadEventStore
        .read_all(&root)
        .unwrap()
        .first()
        .expect("起始事件")
        .schema_version;
    let sequence = service
        .read_thread_activation(&root, &stale.session_id)
        .unwrap()
        .next_sequence;
    JsonlThreadEventStore
        .append(
            &root,
            &ThreadEventRecord {
                schema_version,
                thread_id: stale.session_id.clone(),
                sequence,
                recorded_at_ms: 460,
                event: ThreadEvent::TurnStarted {
                    turn: drift_turn.clone(),
                },
            },
        )
        .unwrap();

    // 收口本轮：旧实现会选中最早那条 Active（stale），把终态写到它头上。
    let mut current = session_record("当前轮", "答完了", 500);
    current.assistant_part_id = "assistant:session-1:current".to_owned();
    let current_turn = turn_from_record(&current, 0).unwrap();
    assert_eq!(
        current_turn.status,
        axon_session::TurnStatus::Completed,
        "夹具必须是本轮成功收口的形态"
    );
    let current_id = current_turn.turn_id.clone();
    let detail = service.read_thread(&root, &stale.session_id).unwrap();
    mirror_existing_turn(
        &service,
        &root,
        &stale.session_id,
        &detail,
        current_turn,
        &current,
    )
    .unwrap();

    let detail = service.read_thread(&root, &stale.session_id).unwrap();
    let status_of = |turn_id: &str| {
        detail
            .turns
            .iter()
            .find(|turn| turn.turn_id == turn_id)
            .unwrap_or_else(|| panic!("轮次 {turn_id} 必须保留"))
            .status
    };
    assert_eq!(
        status_of(&current_id),
        axon_session::TurnStatus::Completed,
        "终态必须落在本轮自己的 turn_id 上"
    );
    assert_eq!(
        status_of(&stale_turn.turn_id),
        axon_session::TurnStatus::Active,
        "残留轮次不该被本轮收口误伤成已完成"
    );
    assert_eq!(
        status_of(&drift_turn.turn_id),
        axon_session::TurnStatus::Active,
        "第二条残留同样不该被误伤"
    );

    // 批量修复：两条残留必须一次清空，激活随之解除。
    let activation = service
        .read_thread_activation(&root, &stale.session_id)
        .unwrap();
    let outcome = service
        .repair_open_turns(
            &root,
            RepairOpenTurnsCommand {
                thread_id: stale.session_id.clone(),
                recorded_at_ms: 600,
                known_next_sequence: Some(activation.next_sequence),
            },
        )
        .unwrap();
    let TurnRepairOutcome::Repaired { turn_ids, .. } = &outcome else {
        panic!("两条残留必须都被收口，实际得到 {outcome:?}");
    };
    assert_eq!(turn_ids.len(), 2, "收口数量必须覆盖全部残留：{turn_ids:?}");

    let detail = service.read_thread(&root, &stale.session_id).unwrap();
    for residual in [&stale_turn.turn_id, &drift_turn.turn_id] {
        assert_eq!(
            detail
                .turns
                .iter()
                .find(|turn| &turn.turn_id == residual)
                .expect("残留轮次仍在历史中")
                .status,
            axon_session::TurnStatus::Interrupted,
            "残留轮次必须收口成中断"
        );
    }
    assert!(
        service
            .read_thread_activation(&root, &stale.session_id)
            .unwrap()
            .active_turn_id
            .is_none(),
        "全部收口后激活必须解除，否则新轮永远起不来"
    );

    // 解卡后的下一轮必须能真正开始。
    let mut next = session_record("下一轮", "", 700);
    next.assistant_part_id = "assistant:session-1:next".to_owned();
    let next_turn = turn_from_record(&next, 0).unwrap();
    service
        .start_turn(
            &root,
            StartTurnCommand {
                thread_id: stale.session_id.clone(),
                recorded_at_ms: 700,
                turn: next_turn,
            },
        )
        .expect("残留清空后新轮必须能开始");

    let _ = std::fs::remove_dir_all(root);
}
