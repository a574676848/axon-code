//! 未收口轮次修复：真实落盘与「只修尾部」缺陷的回归。
//!
//! 独立成文是为了守住 `tests/mod.rs` 的单文件行数上限；判据集中在这里，是因为
//! 「修哪些轮次」直接决定会话能否再起一轮。

use super::{TargetThreadOnlyStore, root, settings, turn};
use crate::{
    JsonlThreadEventStore, StartThreadCommand, ThreadEvent, ThreadEventRecord,
    ThreadEventStore as _, ThreadService, ThreadSource, TurnStatus,
};

/// 端到端：真实 JSONL 存储上把崩溃遗留的 Active 尾轮收口成中断，并解除激活。
///
/// 这条覆盖单测覆盖不到的两段：一次原子的事件落盘，以及落盘后激活状态真的被解除
/// （解除才会让下一轮 `start_turn` 放行，否则会话永久卡死）。
#[test]
fn repair_open_turns_closes_a_crashed_tail_turn_on_real_storage() {
    let root = root("jsonl-turn-repair");
    let service = ThreadService::new(JsonlThreadEventStore);
    service
        .start_thread(
            &root,
            StartThreadCommand {
                thread_id: "thread".to_owned(),
                session_root_id: "thread".to_owned(),
                parent_thread_id: None,
                forked_from_thread_id: None,
                source: ThreadSource::CliRun,
                ephemeral: false,
                settings: settings(),
                created_at_ms: 100,
                // 崩溃现场：进程在轮次中途退出，尾轮停在 Active。
                turn: turn("crashed", 0, TurnStatus::Active),
            },
        )
        .unwrap();

    let outcome = service
        .repair_open_turns(
            &root,
            crate::RepairOpenTurnsCommand {
                thread_id: "thread".to_owned(),
                recorded_at_ms: 200,
                known_next_sequence: None,
            },
        )
        .unwrap();

    let crate::TurnRepairOutcome::Repaired { turn_ids, .. } = &outcome else {
        panic!("应真的收口而不是判定已平衡：{outcome:?}");
    };
    assert_eq!(turn_ids, &vec!["crashed".to_owned()]);
    let detail = service.read_thread(&root, "thread").unwrap();
    assert_eq!(
        detail.turns.last().expect("仍有轮次").status,
        TurnStatus::Interrupted,
        "尾轮必须变成中断"
    );
    assert!(
        service
            .read_thread_activation(&root, "thread")
            .unwrap()
            .active_turn_id
            .is_none(),
        "激活必须解除，否则下一轮永远起不来"
    );
}

/// 回归：尾部已完成、残留落在中间时，收口必须覆盖全部残留。
///
/// 只修尾部的实现在这里返回 `Balanced`（尾部已 `Completed`）且零写入，残留轮次因此
/// 永远占着激活槽，`start_turn` 随后一直拒绝，线上表现就是「重发也起不来」。
#[test]
fn repair_open_turns_closes_residual_under_a_settled_tail() {
    let root = root("jsonl-turn-repair-residual");
    let service = ThreadService::new(JsonlThreadEventStore);
    service
        .start_thread(
            &root,
            StartThreadCommand {
                thread_id: "thread".to_owned(),
                session_root_id: "thread".to_owned(),
                parent_thread_id: None,
                forked_from_thread_id: None,
                source: ThreadSource::CliRun,
                ephemeral: false,
                settings: settings(),
                created_at_ms: 100,
                turn: turn("stale", 0, TurnStatus::Active),
            },
        )
        .unwrap();

    // 再叠一轮并把它收口：尾部因此是 Completed，残留落在中间。
    let schema_version = JsonlThreadEventStore
        .read_all(&root)
        .unwrap()
        .first()
        .expect("起始事件")
        .schema_version;
    let base_sequence = service
        .read_thread_activation(&root, "thread")
        .unwrap()
        .next_sequence;
    for (offset, event) in [
        ThreadEvent::TurnStarted {
            turn: turn("tail", 1, TurnStatus::Active),
        },
        ThreadEvent::TurnCompleted {
            turn: turn("tail", 1, TurnStatus::Completed),
        },
    ]
    .into_iter()
    .enumerate()
    {
        let sequence = base_sequence + offset as u64;
        JsonlThreadEventStore
            .append(
                &root,
                &ThreadEventRecord {
                    schema_version,
                    thread_id: "thread".to_owned(),
                    sequence,
                    recorded_at_ms: 100 + sequence,
                    event,
                },
            )
            .unwrap();
    }

    let outcome = service
        .repair_open_turns(
            &root,
            crate::RepairOpenTurnsCommand {
                thread_id: "thread".to_owned(),
                recorded_at_ms: 300,
                known_next_sequence: None,
            },
        )
        .unwrap();

    let crate::TurnRepairOutcome::Repaired { turn_ids, .. } = &outcome else {
        panic!("中间残留必须被收口，实际得到 {outcome:?}");
    };
    assert_eq!(turn_ids, &vec!["stale".to_owned()]);
    assert!(
        service
            .read_thread_activation(&root, "thread")
            .unwrap()
            .active_turn_id
            .is_none(),
        "残留清掉后激活必须解除"
    );
}

/// 已平衡的历史必须零写入，否则重复调用会不断追加终态事件。
#[test]
fn repair_open_turns_is_zero_write_when_history_is_balanced() {
    let store = TargetThreadOnlyStore::default();
    let service = ThreadService::new(store.clone());
    let root = std::path::PathBuf::from("axon-root");
    let outcome = service
        .repair_open_turns(
            &root,
            crate::RepairOpenTurnsCommand {
                thread_id: "thread-1".to_owned(),
                recorded_at_ms: 1,
                known_next_sequence: None,
            },
        )
        .expect("已平衡的历史不该报错");

    assert_eq!(outcome, crate::TurnRepairOutcome::Balanced);
    assert!(
        store.appended().is_empty(),
        "Balanced 必须零写入，否则重复调用会不断追加终态事件"
    );
}
