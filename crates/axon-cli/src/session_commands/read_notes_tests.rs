//! 追读口径：`uncertain` 轮次与无归属子代理帧必须给出可读结论。

use axon_domain::{ExecutionId, ExecutionStatus, InteractionId};
use axon_session::{Thread, ThreadDetail, ThreadSettings, ThreadSource, ThreadStatus, Turn};

use super::super::session_repair::DispatchResolution;
use super::super::thread_cli::{SessionReadReport, render_read_text};
use super::{
    READ_NOTE_PREFIX, ReadNoteScan, ReadNotes, unattributed_frames_note, uncertain_turn_note,
};

#[test]
fn sessions_read_explains_uncertain_turn() {
    // 结论未知不是「没有内容」：必须点名 execution 并写清「外部结果未确认」。
    let note = uncertain_turn_note(
        ExecutionId::new("execution-uncertain")
            .expect("执行标识")
            .as_str(),
        ExecutionStatus::Uncertain,
    )
    .expect("uncertain 轮次必须给出可读结论");
    assert!(note.contains("execution-uncertain"), "{note}");
    assert!(note.contains("uncertain"), "{note}");
    assert!(note.contains("外部结果未确认"), "{note}");
    assert!(note.contains("不是「没有内容」"), "{note}");

    // 其余结论状态不再解释：正常收口或明确失败的轮次不追加备注。
    for status in [
        ExecutionStatus::Completed,
        ExecutionStatus::Failed,
        ExecutionStatus::Cancelled,
        ExecutionStatus::Running,
    ] {
        assert_eq!(
            uncertain_turn_note("execution-settled", status),
            None,
            "{status:?} 不应被读成结论未知"
        );
    }

    // 报告文本：正文为空时必须由备注给出原因，不能只留空正文。
    let report = SessionReadReport {
        status: "session_read",
        store_path: "threads.jsonl".to_owned(),
        thread: empty_thread_detail(),
        notes: vec![note],
        uncertain_turn_count: 1,
        unattributed_frame_count: 0,
    };
    let text = render_read_text(&report);
    assert!(
        text.contains("- 正文: 没有可渲染的轮次正文（原因见下面的备注）"),
        "{text}"
    );
    assert!(text.contains(READ_NOTE_PREFIX), "{text}");
    assert!(text.contains("外部结果未确认"), "{text}");
}

#[test]
fn session_read_explains_unattributed_subagent_frames() {
    // 没有归属不是「本来就是空的」：要点名帧数、给出原因，并写清过程仍在库里。
    let note = unattributed_frames_note(
        3,
        &DispatchResolution::Unique(InteractionId::new("interaction-dispatch").expect("交互标识")),
    )
    .expect("无归属帧必须给出可读结论");
    assert!(note.contains("3 帧没有归属"), "{note}");
    assert!(note.contains("interaction-dispatch"), "{note}");
    assert!(note.contains("--attribute"), "{note}");
    assert!(
        note.contains("无法归位不等于这一轮没有过程"),
        "缺少这句时空正文会被读成这一轮没有过程：{note}"
    );

    // 无法唯一确定时如实给原因，不做猜测。
    let undetermined = unattributed_frames_note(
        2,
        &DispatchResolution::Undetermined("派发轮次有多条候选".to_owned()),
    )
    .expect("无归属帧必须给出可读结论");
    assert!(
        undetermined.contains("无法唯一确定派发轮次"),
        "{undetermined}"
    );
    assert!(
        undetermined.contains("派发轮次有多条候选"),
        "{undetermined}"
    );
    assert_eq!(
        unattributed_frames_note(0, &DispatchResolution::Undetermined("n/a".to_owned())),
        None,
        "没有无归属帧时不加备注"
    );

    // 诊断摘要：点名会话与帧数，逐条标注原因。
    let scan = ReadNoteScan {
        uncertain_sessions: vec!["session-uncertain".to_owned()],
        unattributed_sessions: vec![("session-subagent".to_owned(), 3)],
    };
    assert!(!scan.is_clean());
    let summary = scan.summary();
    assert!(summary.contains("结论未知"), "{summary}");
    assert!(summary.contains("session-uncertain"), "{summary}");
    assert!(summary.contains("没有归属"), "{summary}");
    assert!(summary.contains("session-subagent（3 帧）"), "{summary}");
    assert!(ReadNoteScan::default().is_clean());

    // 报告文本：备注必须进 stdout。
    let report = SessionReadReport {
        status: "session_read",
        store_path: "threads.jsonl".to_owned(),
        thread: empty_thread_detail(),
        notes: vec![note],
        uncertain_turn_count: 0,
        unattributed_frame_count: 3,
    };
    let text = render_read_text(&report);
    assert!(text.contains("3 帧没有归属"), "{text}");
    assert_eq!(
        ReadNotes {
            lines: vec!["结论".to_owned()],
            uncertain_turns: 1,
            unattributed_frames: 0,
        }
        .text_lines(),
        vec!["- 备注: 结论".to_owned()]
    );
}

/// 空轮次的 Thread 详情：正文本来就没有可渲染内容，原因只能写在备注里。
fn empty_thread_detail() -> ThreadDetail {
    ThreadDetail {
        thread: Thread {
            thread_id: "thread-conclusion".to_owned(),
            session_root_id: "thread-conclusion".to_owned(),
            parent_thread_id: None,
            forked_from_thread_id: None,
            name: None,
            status: ThreadStatus::Idle,
            source: ThreadSource::CliRun,
            ephemeral: false,
            settings: ThreadSettings {
                cwd: ".".to_owned(),
                provider: "fixture".to_owned(),
                model: "fixture".to_owned(),
                selected_skill_name: None,
            },
            child_origin: None,
            created_at_ms: 1,
            updated_at_ms: 1,
            archived_at_ms: None,
        },
        turns: Vec::<Turn>::new(),
        compacted_history: None,
    }
}
