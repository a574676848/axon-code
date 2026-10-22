//! 统一完成通知的行为测试（合同 §5.1.4）。
//!
//! 覆盖判据「同一终态重复投递只展示一次」与「故障注入覆盖重复消费与载荷截断」，
//! 以及单次条数上限的合并摘要。
//!
//! 隔离策略：通知总线是**进程级**设施，因此每个用例使用独立的 owner：
//! 否则并行用例的投递会互相计数。

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use super::*;
use crate::runtime_limits::COMPLETION_NOTICE_SUMMARY_MAX_CHARS;

fn notice(owner: &str, key: &str, summary: &str) -> CompletionNotice {
    CompletionNotice::new(
        JobProducerKind::Shell,
        key.to_owned(),
        format!("epoch:{key}"),
        owner.to_owned(),
        JobTerminalKind::Completed,
        "正常结束",
        summary,
        Some("/tmp/tee/job-1.shell.log".to_owned()),
        None,
    )
}

/// 收集批次，用于断言投递次数与内容。
#[derive(Default)]
struct RecordingHandler {
    batches: Mutex<Vec<CompletionNoticeBatch>>,
    calls: AtomicUsize,
}

impl RecordingHandler {
    fn record(&self, batch: CompletionNoticeBatch) {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.batches
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(batch);
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::Relaxed)
    }

    fn batches(&self) -> Vec<CompletionNoticeBatch> {
        self.batches
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

fn subscribe(owner: &str, recorder: &Arc<RecordingHandler>) -> CompletionNoticeSubscription {
    let recorder = Arc::clone(recorder);
    subscribe_completion_notices(owner, Arc::new(move |batch| recorder.record(batch)))
}

/// 同一终态重复生产只投递一次：去重由事件键保证，不依赖调用方约定。
#[test]
fn repeated_publication_of_the_same_terminal_delivers_once() {
    let owner = "notice-test-dedup";
    let recorder = Arc::new(RecordingHandler::default());
    let _subscription = subscribe(owner, &recorder);

    publish_completion_notice(notice(owner, "dedup-key-1", "第一次"));
    publish_completion_notice(notice(owner, "dedup-key-1", "第一次"));

    assert_eq!(recorder.calls(), 1, "同一事件键重复生产只允许投递一次");
    let batches = recorder.batches();
    assert_eq!(batches[0].notices.len(), 1);
    assert_eq!(batches[0].notices[0].summary, "第一次");
}

/// 单次投递条数上限：超限部分合并为计数摘要，不得无界入队。
#[test]
fn batch_beyond_per_turn_limit_becomes_a_count_summary() {
    let owner = "notice-test-burst";
    let recorder = Arc::new(RecordingHandler::default());
    let _subscription = subscribe(owner, &recorder);

    let total = COMPLETION_NOTICE_MAX_PER_TURN + 3;
    let notices = (0..total)
        .map(|index| notice(owner, &format!("burst-key-{index}"), "批量完成"))
        .collect::<Vec<_>>();
    publish_completion_notices(notices);

    assert_eq!(recorder.calls(), 1, "一次批量生产只投递一次");
    let batches = recorder.batches();
    assert_eq!(batches[0].notices.len(), COMPLETION_NOTICE_MAX_PER_TURN);
    assert_eq!(batches[0].merged_count, 3);
    let rendered = batches[0].render();
    assert!(
        rendered.contains("另有 3 个后台作业已完成"),
        "超限部分必须合并为计数摘要：{rendered}"
    );
}

/// 摘要必须有界：超长正文被截断到上限，但终态与归档引用必须保留。
#[test]
fn notice_summary_is_bounded_and_keeps_terminal_and_reference() {
    let long_summary = "长".repeat(COMPLETION_NOTICE_SUMMARY_MAX_CHARS * 3);
    let notice = notice("notice-test-bounded", "bounded-key-1", &long_summary);

    assert_eq!(
        notice.summary.chars().count(),
        COMPLETION_NOTICE_SUMMARY_MAX_CHARS,
        "摘要必须按字符数收敛到上限"
    );
    let rendered = notice.render();
    assert!(rendered.contains("终态：completed"), "终态不得被截断丢失");
    assert!(
        rendered.contains("/tmp/tee/job-1.shell.log"),
        "归档引用不得被截断丢失"
    );
    assert!(
        rendered.chars().count() <= COMPLETION_NOTICE_MAX_CHARS,
        "单条通知正文不得超过上限"
    );
}

/// 正文超限时先丢摘要正文，终态与引用仍必须完整。
#[test]
fn oversized_notice_body_truncates_summary_before_references() {
    let notice = CompletionNotice::new(
        JobProducerKind::Subagent,
        "truncate-key-1".to_owned(),
        "epoch:9".to_owned(),
        "notice-test-truncate".to_owned(),
        JobTerminalKind::Timeout,
        "墙钟超时",
        &"结".repeat(COMPLETION_NOTICE_SUMMARY_MAX_CHARS),
        Some("/tmp/tee/job-9.shell.log".to_owned()),
        Some("/root/review".to_owned()),
    );

    let rendered = notice.render();
    assert!(
        rendered.chars().count() <= COMPLETION_NOTICE_MAX_CHARS,
        "正文必须收敛到上限"
    );
    for required in ["终态：timeout", "/tmp/tee/job-9.shell.log", "/root/review"] {
        assert!(
            rendered.contains(required),
            "截断不得丢字段：{required} / {rendered}"
        );
    }
}

/// 退订之后不再收到通知：句柄 drop 即断开，不留下悬空回调。
#[test]
fn unsubscribed_owner_stops_receiving_notices() {
    let owner = "notice-test-unsubscribe";
    let recorder = Arc::new(RecordingHandler::default());
    {
        let _subscription = subscribe(owner, &recorder);
        publish_completion_notice(notice(owner, "unsubscribe-key-1", "第一条"));
    }
    assert_eq!(recorder.calls(), 1);

    publish_completion_notice(notice(owner, "unsubscribe-key-2", "第二条"));
    assert_eq!(recorder.calls(), 1, "退订后不得继续投递");
}

// ---------- 结算通知文案规范（合同 §5.1.9） ----------

/// 构造一条子代理结算通知；`final_message` 为空串表示子代理没有留下结束语。
fn subagent_notice(
    terminal: JobTerminalKind,
    final_message: &str,
    mailbox_ref: Option<&str>,
) -> CompletionNotice {
    CompletionNotice::new(
        JobProducerKind::Subagent,
        format!("subagent:epoch:7:{}", terminal.as_str()),
        "epoch:7".to_owned(),
        "notice-test-copy".to_owned(),
        terminal,
        "固定退出原因",
        final_message,
        Some("/tmp/tee/job-7.log".to_owned()),
        mailbox_ref.map(str::to_owned),
    )
}

fn first_line(notice: &CompletionNotice) -> String {
    notice
        .render()
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// 六类终态的首行措辞逐一快照：每类固定，且互不相同。
#[test]
fn six_terminal_kinds_render_the_standardized_first_line() {
    let cases = [
        (JobTerminalKind::Completed, "任务已完成"),
        (JobTerminalKind::Refusal, "任务未被接受"),
        (JobTerminalKind::Failed, "任务执行失败"),
        (JobTerminalKind::Killed, "任务已被终止"),
        (JobTerminalKind::Timeout, "任务已因墙钟超时被终止"),
        (JobTerminalKind::Unresponsive, "任务已因空闲超时被终止"),
    ];
    assert_eq!(cases.len(), 6, "合同 §8.1 第 16 条要求六类终态文案");

    let mut titles = Vec::new();
    for (terminal, expected) in cases {
        let line = first_line(&subagent_notice(
            terminal,
            "结束语正文",
            Some("/root/review"),
        ));
        assert_eq!(
            line,
            format!("{expected}（生产者为 subagent）："),
            "{terminal:?} 的首行必须使用模板表里的固定措辞"
        );
        titles.push(expected);
    }
    let mut unique = titles.to_vec();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        unique.len(),
        titles.len(),
        "六类首行措辞必须互不相同，否则父代理无法只读首行判断结局"
    );
}

/// refusal 专项：父代理收到的是「任务未被接受」，不得被读成完成或普通失败。
#[test]
fn refusal_notice_states_the_task_was_not_accepted() {
    let notice = subagent_notice(JobTerminalKind::Refusal, "", Some("/root/review"));
    let rendered = notice.render();

    assert_eq!(first_line(&notice), "任务未被接受（生产者为 subagent）：");
    assert!(rendered.contains("终态：refusal"), "{rendered}");
    assert!(
        !rendered.contains("任务已完成") && !rendered.contains("任务执行失败"),
        "拒绝不得投影为完成或普通失败：{rendered}"
    );
}

/// completed 类对可续接子代理必须携带行为提示。
#[test]
fn completed_subagent_notice_carries_the_continuable_hint() {
    let rendered = subagent_notice(
        JobTerminalKind::Completed,
        "已按证据改完并复验",
        Some("/root/delegate_fast_1"),
    )
    .render();

    assert!(
        rendered.contains("除非你再给它发消息，否则它不会做更多工作。"),
        "可续接子代理的完成通知必须携带行为提示：{rendered}"
    );
    assert!(
        rendered.contains("结束语：已按证据改完并复验"),
        "结束语必须是子代理的最后一条非空 assistant 文本块：{rendered}"
    );
}

/// 不可续接（无 mailbox 引用）时不得给出「还能继续」的暗示。
#[test]
fn completed_notice_without_collaboration_ref_omits_the_hint() {
    let rendered = subagent_notice(JobTerminalKind::Completed, "结论", None).render();

    assert!(
        !rendered.contains("除非你再给它发消息"),
        "没有协作引用时不得承诺可续接：{rendered}"
    );
}

/// 空结束语专项：无非空 assistant 文本块时使用固定替换句。
#[test]
fn empty_final_message_uses_the_fixed_replacement() {
    let rendered =
        subagent_notice(JobTerminalKind::Completed, "   ", Some("/root/review")).render();

    assert!(
        rendered.contains("结束语：它没有留下结束语。"),
        "空结束语必须落地为固定替换句：{rendered}"
    );
}

/// Shell 作业没有结束语概念：无摘要时不得伪造一段结算正文。
#[test]
fn shell_notice_without_summary_has_no_settlement_block() {
    let notice = CompletionNotice::new(
        JobProducerKind::Shell,
        "shell-key-1".to_owned(),
        "epoch:7".to_owned(),
        "notice-test-shell".to_owned(),
        JobTerminalKind::Completed,
        "退出码 0",
        "",
        Some("/tmp/tee/job-7.log".to_owned()),
        None,
    );
    let rendered = notice.render();

    assert!(
        !rendered.contains("结束语") && !rendered.contains("它没有留下结束语。"),
        "Shell 作业不得出现子代理的结束语段：{rendered}"
    );
    assert!(rendered.contains("终态：completed"), "{rendered}");
}
