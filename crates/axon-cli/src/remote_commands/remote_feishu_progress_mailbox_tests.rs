//! 进度投影邮箱与结算权契约测试。

use std::sync::mpsc;

use super::*;

fn turn(dispatch_id: &str) -> ProgressTurn {
    ProgressTurn {
        dispatch_id: dispatch_id.to_owned(),
        chat_id: "oc_chat".to_owned(),
        reply_to_message_id: None,
    }
}

fn finish(turn: ProgressTurn) -> ProgressCommand {
    let (ack, _receiver) = mpsc::channel();
    ProgressCommand::Finish {
        turn,
        status: CardStatus::Completed,
        ack,
        settlement: Arc::new(ProgressSettlement::new()),
    }
}

fn delta(text: &str) -> ProgressCommand {
    ProgressCommand::Event {
        turn: turn("dispatch"),
        event: ProjectionEvent::AssistantDelta {
            part_id: "part".to_owned(),
            text: text.to_owned(),
        },
    }
}

fn tool(call_id: &str) -> ProgressCommand {
    ProgressCommand::Event {
        turn: turn("dispatch"),
        event: ProjectionEvent::ToolStarted {
            call_id: call_id.to_owned(),
            label: "执行".to_owned(),
            target: None,
        },
    }
}

#[test]
fn finish_is_served_before_queued_projection_work() {
    let mut commands = VecDeque::new();
    commands.push_back(delta("delta-1"));
    commands.push_back(tool("call-1"));
    commands.push_back(finish(turn("dispatch-settle")));
    commands.push_back(delta("delta-2"));

    // 回合收口不能被其他 chat 的投影事件排在后面。
    assert!(matches!(
        take_highest_priority(&mut commands),
        Some(ProgressCommand::Finish { .. })
    ));
    // 其余事件保持入队顺序：投影必须按 wire 到达顺序应用。
    assert!(matches!(
        take_highest_priority(&mut commands),
        Some(ProgressCommand::Event {
            event: ProjectionEvent::AssistantDelta { text, .. },
            ..
        }) if text == "delta-1"
    ));
    assert!(matches!(
        take_highest_priority(&mut commands),
        Some(ProgressCommand::Event {
            event: ProjectionEvent::ToolStarted { call_id, .. },
            ..
        }) if call_id == "call-1"
    ));
    assert!(matches!(
        take_highest_priority(&mut commands),
        Some(ProgressCommand::Event {
            event: ProjectionEvent::AssistantDelta { text, .. },
            ..
        }) if text == "delta-2"
    ));
    assert!(take_highest_priority(&mut commands).is_none());
}

#[test]
fn mailbox_evicts_oldest_delta_before_finish() {
    let mailbox = ProgressMailbox::new();
    for index in 0..PROGRESS_MAILBOX_CAPACITY {
        assert!(mailbox.try_push(delta(&format!("delta-{index}"))));
    }
    assert!(mailbox.try_push(finish(turn("dispatch-evict"))));
    let state = mailbox
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_eq!(state.commands.len(), PROGRESS_MAILBOX_CAPACITY);
    assert!(
        state
            .commands
            .iter()
            .any(|command| matches!(command, ProgressCommand::Finish { .. })),
        "收口命令绝不能被淘汰"
    );
    assert!(
        matches!(
            state.commands.front(),
            Some(ProgressCommand::Event {
                event: ProjectionEvent::AssistantDelta { text, .. },
                ..
            }) if text == "delta-1"
        ),
        "满载时应淘汰最旧的正文增量而非收口命令"
    );
}

#[test]
fn mailbox_never_evicts_tool_rows() {
    let mailbox = ProgressMailbox::new();
    for index in 0..PROGRESS_MAILBOX_CAPACITY {
        assert!(mailbox.try_push(tool(&format!("call-{index}"))));
    }
    // 队列里全是不可淘汰的语义命令：允许超出一个单位，但一条都不能丢。
    assert!(mailbox.try_push(delta("late-delta")));
    let state = mailbox
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_eq!(
        state.commands.len(),
        PROGRESS_MAILBOX_CAPACITY + 1,
        "语义命令不可淘汰"
    );
}

#[test]
fn caller_timeout_withdraws_settlement_before_worker_writes() {
    let settlement = ProgressSettlement::new();
    assert!(
        settlement.abort(),
        "worker 未开始结算时，调用方超时必须能弃权"
    );
    assert!(
        !settlement.begin(),
        "已弃权的收口不得再写飞书，否则与 durable outbox 补偿重复"
    );
}

#[test]
fn worker_claim_blocks_withdrawal_so_projection_ledger_stays_truthful() {
    let settlement = ProgressSettlement::new();
    assert!(settlement.begin(), "首次结算必须取得写飞书的权利");
    assert!(
        !settlement.abort(),
        "worker 已在写卡片时，调用方超时不能撤回已发生的投影"
    );
    assert!(!settlement.begin(), "结算权只能被消费一次");
}
