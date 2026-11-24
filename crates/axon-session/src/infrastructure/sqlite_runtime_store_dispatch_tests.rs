//! 派发轮终态与带归属事件：不依赖 lease 的结算路径。
//!
//! 执行侧派发没有任何 work_item，因此终态只能写在入站行上、逐轮读数只能按归属写入。

use super::SqliteRuntimeStore;
use super::sqlite_runtime_store_interaction_fixtures::{
    interaction_record, sample_interaction, test_session,
};
use crate::ports::EventLogStore as _;
use crate::{InteractionQueryStore, InteractionStore, RuntimeStore};
use axon_domain::{InteractionStatus, RuntimeHostId};
#[test]
fn dispatch_settlement_marks_terminal_without_a_lease() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let mut dispatch = interaction_record(
        &test_session("session-dispatch"),
        "interaction-dispatch",
        "message-dispatch",
        10,
    );
    // 执行侧派发：渠道就是派发标记，它没有 work_item，因此也拿不到 lease。
    dispatch.source = axon_domain::InteractionSource::new(
        axon_domain::AGENT_DISPATCH_SOURCE_CHANNEL,
        "agent-dispatch:session-dispatch:1",
    )
    .expect("派发来源");
    assert!(store.append(&dispatch).expect("追加").inserted);
    let status = |store: &SqliteRuntimeStore, interaction_id: &axon_domain::InteractionId| {
        store
            .get_interaction(interaction_id)
            .expect("按 ID 读取")
            .expect("记录存在")
            .status
    };
    assert_eq!(
        status(&store, &dispatch.interaction_id),
        InteractionStatus::Received,
        "未结算的派发轮只能是已收到"
    );

    store
        .settle_dispatch(
            &dispatch.interaction_id,
            crate::ports::DispatchTerminalKind::Understood,
            20,
        )
        .expect("结算派发轮");
    assert_eq!(
        status(&store, &dispatch.interaction_id),
        InteractionStatus::Understood
    );

    // 正常入站轮不走这一条：调用不报错，但状态必须原样不动。
    let normal = interaction_record(
        &test_session("session-normal"),
        "interaction-normal",
        "message-normal",
        11,
    );
    store.append(&normal).expect("追加");
    store
        .settle_dispatch(
            &normal.interaction_id,
            crate::ports::DispatchTerminalKind::Rejected,
            21,
        )
        .expect("对非派发轮调用应当无副作用");
    assert_eq!(
        status(&store, &normal.interaction_id),
        InteractionStatus::Received,
        "正常入站轮的终态只能由 work_item 的 lease 结算"
    );
}

#[test]
fn attributed_events_reach_the_owning_interaction_without_a_lease() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let host = store
        .claim_host(
            &RuntimeHostId::new("attributed-event-host").expect("host"),
            1,
            None,
            1,
            1_000,
        )
        .expect("host lease");
    let interaction = sample_interaction();
    store
        .create_session(&interaction.session_id, ".", 1)
        .expect("会话行");
    store.append(&interaction).expect("追加");
    store
        .append_attributed_events(
            &interaction.session_id,
            &host,
            &interaction.interaction_id,
            &[("turn_metrics".to_owned(), "{\"turnId\":\"x\"}".to_owned())],
            5,
        )
        .expect("带归属追加");
    let framed = store
        .replay_interaction_events(&interaction.session_id, &interaction.interaction_id, 0, 10)
        .expect("读归属轮次事件");
    assert_eq!(framed.len(), 1, "归属列必须落到这一轮上");
    // 对照：不带归属写入的帧不会出现在这一轮的定向读取里。
    store
        .append_events(
            &interaction.session_id,
            &host,
            &[("turn_metrics".to_owned(), "{}".to_owned())],
            6,
        )
        .expect("无归属追加");
    let framed = store
        .replay_interaction_events(&interaction.session_id, &interaction.interaction_id, 0, 10)
        .expect("再读归属轮次事件");
    assert_eq!(framed.len(), 1, "无归属帧不得混入这一轮");
}
#[test]
fn repeated_archive_history_does_not_duplicate_the_turn_row() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let session = test_session("session-history-dup");
    store.create_session(&session, ".", 1).expect("会话行");
    let turn = interaction_record(
        &session,
        "interaction-history-dup",
        "message-history-dup",
        10,
    );
    store.append(&turn).expect("交互入库");
    // 正常入站轮：work_item 已结算但没有终态列，终态只出现在归档历史里，且被归档了两次。
    let batch = format!(
        "INSERT INTO work_items(execution_id,session_id,attempt,priority,inbox_id,status,created_at_ms,updated_at_ms) \
             VALUES('exec-1','{session}',1,0,'{interaction}','completed',10,10); \
         INSERT INTO work_item_history(work_id,session_id,attempt,priority,inbox_id,terminal_kind,status,created_at_ms,updated_at_ms,finished_at_ms) \
             VALUES('work-1','{session}',1,0,'{interaction}','understood','completed',10,10,11); \
         INSERT INTO work_item_history(work_id,session_id,attempt,priority,inbox_id,terminal_kind,status,created_at_ms,updated_at_ms,finished_at_ms) \
             VALUES('work-2','{session}',1,0,'{interaction}','understood','completed',12,12,13);",
        session = session.as_str(),
        interaction = turn.interaction_id.as_str(),
    );
    store
        .with_read_connection(|connection| connection.execute_batch(&batch))
        .expect("写入工作项与重复归档");

    // 同一轮被归档两次时，索引页与轮次头都只能各出现一次。
    let records = store
        .interactions_for_session(session.as_str(), 10, None)
        .expect("索引页");
    assert_eq!(records.len(), 1, "重复归档不得让同一轮在索引页出现两次");
    let heads = store
        .interaction_turn_heads(session.as_str(), 10)
        .expect("轮次头");
    assert_eq!(heads.len(), 1);
    assert_eq!(heads[0].status, InteractionStatus::Understood);
}

#[test]
fn sessions_with_transcripts_distinguishes_empty_sessions() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    // 只登记、没有任何轮次的会话：起跑前就失败的执行就是这种形态。
    let empty = test_session("transcript-empty-session");
    // 只有派发轮记录、没有事件（目标工具更新）：仍然是有内容的时间线。
    let dispatch_only = test_session("transcript-dispatch-session");
    // 真跑过一轮的会话：事件水位已推进。
    let executed = test_session("transcript-executed-session");
    let missing = test_session("transcript-missing-session");
    store.create_session(&empty, ".", 1).expect("空会话");
    store
        .create_session(&dispatch_only, ".", 2)
        .expect("派发会话");
    store.create_session(&executed, ".", 3).expect("执行会话");
    let turn = interaction_record(
        &dispatch_only,
        "interaction-transcript-dispatch",
        "message-transcript-dispatch",
        10,
    );
    store.append(&turn).expect("交互入库");
    store
        .with_read_connection(|connection| {
            connection.execute(
                "UPDATE sessions SET last_event_sequence=7 WHERE session_id=?1",
                [executed.as_str()],
            )
        })
        .expect("推进事件水位");

    let found = store
        .sessions_with_transcripts(&[
            empty.clone(),
            dispatch_only.clone(),
            executed.clone(),
            missing,
        ])
        .expect("批量判定");
    assert!(
        !found.contains(empty.as_str()),
        "只登记过、没有任何内容的会话不得算成有逐字稿"
    );
    assert!(found.contains(dispatch_only.as_str()));
    assert!(found.contains(executed.as_str()));
    assert!(
        store
            .sessions_with_transcripts(&[])
            .expect("空输入")
            .is_empty()
    );

    // 性能约束：这条判定在目标看板每页调用一次，必须是索引查找而不是全表扫描。
    let plan: Vec<String> = store
        .with_read_connection(|connection| {
            let mut statement = connection.prepare(
                "EXPLAIN QUERY PLAN SELECT s.session_id FROM sessions AS s \
                 WHERE s.session_id IN (?,?) \
                   AND (s.last_event_sequence > 0 \
                        OR EXISTS (SELECT 1 FROM inbox AS i WHERE i.session_id = s.session_id))",
            )?;
            let rows = statement.query_map(["a", "b"], |row| row.get::<_, String>(3))?;
            rows.collect::<rusqlite::Result<Vec<String>>>()
        })
        .expect("查询计划");
    assert!(
        plan.iter().any(|line| line.contains("SEARCH")),
        "必须走索引查找：{plan:?}"
    );
    assert!(
        !plan.iter().any(|line| line.contains("SCAN s")),
        "不得对 sessions 做全表扫描：{plan:?}"
    );
}

#[test]
fn sessions_by_ids_reads_many_rows_and_skips_unknown_ids() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let first = test_session("batch-session-a");
    let second = test_session("batch-session-b");
    store.create_session(&first, ".", 1).expect("会话行");
    store.create_session(&second, ".", 2).expect("会话行");
    let missing = test_session("batch-session-missing");

    let found = store
        .sessions_by_ids(&[first.clone(), second.clone(), missing])
        .expect("批量读取");
    assert_eq!(found.len(), 2, "不存在的标识不得出现在结果里");
    assert!(found.contains_key(first.as_str()));
    assert!(found.contains_key(second.as_str()));
    assert!(store.sessions_by_ids(&[]).expect("空输入").is_empty());
}
