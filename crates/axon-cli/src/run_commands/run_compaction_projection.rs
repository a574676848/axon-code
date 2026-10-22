//! 压缩（代际窗口轮转）生命周期投影的单一事实源。
//!
//! 所有压缩路径都必须通过本模块产出同构的 wire 事实，避免再次出现
//! 「事件已经发出、却没有任何消费者识别」的投影断层：
//! - `CompactionStarted` / `CompactionCompleted` / `CompactionFailed`：
//!   TUI history 生命周期（`runtime_history_lifecycle.rs`）与 `axon run` 文本投影共同识别；
//! - `CompactionPlanned`：空闲轮转只完成排期、尚未执行时的独立事实；
//! - `WireEventKind::ContextRollover`：窗口序号、归档规模与便签注入结果的权威协议事实
//!   （此前协议与 TUI 消费端齐备，却没有任何生产者）。

use axon_display::format_decimal_count;
use axon_protocol::wire::{WireContextRollover, WireLifecycleStatus};

use super::run_progress::{RunLifecycleStatusView, RunProgressSink};

/// TUI history 生命周期与文本投影共同识别的压缩进行中类别。
pub(super) const COMPACTION_STARTED_KIND: &str = "CompactionStarted";
/// 压缩成功类别；TUI 以 terminal 语义原位替换同 part id 的进行中 cell。
pub(super) const COMPACTION_COMPLETED_KIND: &str = "CompactionCompleted";
/// 压缩失败类别。
pub(super) const COMPACTION_FAILED_KIND: &str = "CompactionFailed";
/// 空闲轮转排期类别：turn 结束时只登记计划，真正执行发生在 debounce 之后。
pub(super) const COMPACTION_PLANNED_KIND: &str = "CompactionPlanned";

/// 压缩生命周期所属展示阶段；TUI 依赖该值把压缩归入 history 区域。
pub(super) const COMPACTION_UI_PHASE: &str = "history";
/// 生命周期标题。
pub(super) const COMPACTION_TITLE: &str = "窗口轮转";
/// `axon run` 文本投影的压缩行前缀，与 assistant 正文区分。
pub(super) const COMPACTION_TEXT_PREFIX: &str = "· ";

/// part id 前缀；同一轮转的开始/完成/失败必须复用同一 id，TUI 才能原位替换 cell。
const COMPACTION_PART_PREFIX: &str = "lifecycle:compaction";
/// turn 内联轮转触发码。
const TRIGGER_CODE_INLINE: &str = "inline";
/// 空闲轮转触发码。
const TRIGGER_CODE_IDLE: &str = "idle";
/// turn 内联轮转展示名。
const TRIGGER_LABEL_INLINE: &str = "内联窗口轮转";
/// 空闲轮转展示名。
const TRIGGER_LABEL_IDLE: &str = "空闲窗口轮转";

/// 压缩触发来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CompactionTrigger {
    /// Provider loop 内联轮转；执行者是当前 turn。
    Inline,
    /// turn 结束后按 debounce 排期的空闲轮转。
    Idle,
}

impl CompactionTrigger {
    /// 进入 part id、通知账本与 lifecycle id 的稳定触发码。
    pub(super) const fn code(self) -> &'static str {
        match self {
            Self::Inline => TRIGGER_CODE_INLINE,
            Self::Idle => TRIGGER_CODE_IDLE,
        }
    }

    /// 面向用户的触发来源展示名。
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Inline => TRIGGER_LABEL_INLINE,
            Self::Idle => TRIGGER_LABEL_IDLE,
        }
    }
}

/// 压缩生命周期投影视图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CompactionLifecycleView {
    pub(super) part_id: String,
    pub(super) kind: &'static str,
    pub(super) title: String,
    pub(super) summary: String,
    pub(super) status: WireLifecycleStatus,
    pub(super) started_at_ms: Option<u64>,
    pub(super) duration_ms: Option<u64>,
}

impl CompactionLifecycleView {
    /// 压缩开始；part id 在同一轮转内保持稳定。
    pub(super) fn started(part_id: String, summary: String, started_at_ms: u64) -> Self {
        Self {
            part_id,
            kind: COMPACTION_STARTED_KIND,
            title: COMPACTION_TITLE.to_owned(),
            summary,
            status: WireLifecycleStatus::Running,
            started_at_ms: Some(started_at_ms),
            duration_ms: None,
        }
    }

    /// 压缩成功；携带量化结果便于排查「模型失忆」。
    ///
    /// 跨 turn 补投的投影没有可信的开始时刻，因此开始时间是可选的。
    pub(super) fn completed(
        part_id: String,
        summary: String,
        started_at_ms: Option<u64>,
        duration_ms: Option<u64>,
    ) -> Self {
        Self {
            part_id,
            kind: COMPACTION_COMPLETED_KIND,
            title: COMPACTION_TITLE.to_owned(),
            summary,
            status: WireLifecycleStatus::Completed,
            started_at_ms,
            duration_ms,
        }
    }

    /// 压缩失败；摘要必须显式声明当前历史保持不变。
    pub(super) fn failed(
        part_id: String,
        summary: String,
        started_at_ms: Option<u64>,
        duration_ms: Option<u64>,
    ) -> Self {
        Self {
            part_id,
            kind: COMPACTION_FAILED_KIND,
            title: COMPACTION_TITLE.to_owned(),
            summary,
            status: WireLifecycleStatus::Failed,
            started_at_ms,
            duration_ms,
        }
    }

    /// 空闲轮转排期；尚未执行，因此不携带开始时间。
    pub(super) fn planned(part_id: String, summary: String) -> Self {
        Self {
            part_id,
            kind: COMPACTION_PLANNED_KIND,
            title: COMPACTION_TITLE.to_owned(),
            summary,
            status: WireLifecycleStatus::Running,
            started_at_ms: None,
            duration_ms: None,
        }
    }
}

/// 是否为 `axon run` 文本投影需要输出的压缩类别；进行中的瞬态不输出，避免同一轮转两行。
pub(super) fn is_text_projectable_kind(kind: &str) -> bool {
    matches!(
        kind,
        COMPACTION_COMPLETED_KIND | COMPACTION_FAILED_KIND | COMPACTION_PLANNED_KIND
    )
}

/// 是否为压缩生命周期类别；文本投影据此识别压缩行。
pub(super) fn is_compaction_kind(kind: &str) -> bool {
    matches!(
        kind,
        COMPACTION_STARTED_KIND
            | COMPACTION_COMPLETED_KIND
            | COMPACTION_FAILED_KIND
            | COMPACTION_PLANNED_KIND
    )
}

/// 单一轮转的稳定 part id；开始与结束共用，TUI 才能把「进行中」替换为「完成」。
pub(super) fn compaction_part_id(trigger: CompactionTrigger, session_id: &str) -> String {
    format!("{COMPACTION_PART_PREFIX}:{}:{session_id}", trigger.code())
}

/// 带窗口序号的 part id；用于跨 turn 补投的空闲轮转结果，保证多次轮转各有独立 cell。
pub(super) fn compaction_window_part_id(
    trigger: CompactionTrigger,
    session_id: &str,
    window_number: u64,
) -> String {
    format!(
        "{}:{window_number}",
        compaction_part_id(trigger, session_id)
    )
}

/// 投影一条压缩生命周期事实。
pub(super) fn emit_compaction_lifecycle(
    progress: Option<&RunProgressSink<'_>>,
    view: &CompactionLifecycleView,
) {
    let Some(sink) = progress else {
        return;
    };
    let mut status_view = RunLifecycleStatusView::with_id(
        view.part_id.clone(),
        view.title.clone(),
        view.kind,
        view.summary.clone(),
        COMPACTION_UI_PHASE,
        None,
    );
    status_view = match view.status {
        WireLifecycleStatus::Completed => status_view.completed(),
        WireLifecycleStatus::Failed => status_view.failed(),
        _ => status_view,
    };
    if let Some(started_at_ms) = view.started_at_ms {
        status_view = status_view.with_started_at_ms(started_at_ms);
    }
    if let Some(duration_ms) = view.duration_ms {
        status_view = status_view.with_duration_ms(duration_ms);
    }
    sink.lifecycle_status(status_view);
}

/// 投影窗口轮转的权威协议事实。
pub(super) fn emit_context_rollover(
    progress: Option<&RunProgressSink<'_>>,
    rollover: &WireContextRollover,
) {
    let Some(sink) = progress else {
        return;
    };
    sink.context_rollover(rollover.clone());
}

/// 构造窗口轮转协议事实。
pub(super) fn context_rollover_fact(
    window_number: u64,
    window_id: String,
    previous_window_id: Option<String>,
    archived_messages_count: usize,
    notes_injected: bool,
) -> WireContextRollover {
    WireContextRollover {
        window_number,
        window_id,
        previous_window_id,
        archived_messages_count,
        notes_injected,
    }
}

/// 压缩开始摘要。
pub(super) fn started_summary(trigger: CompactionTrigger) -> String {
    format!("{}已开始 · 正在整理代际便签与会话目标锚点", trigger.label())
}

/// 压缩成功摘要；覆盖「历史被裁剪、checkpoint 建立」的全部可核对事实。
pub(super) fn completed_summary(
    trigger: CompactionTrigger,
    before_tokens: u64,
    after_tokens: u64,
    before_messages: usize,
    after_messages: usize,
    duration_ms: u64,
) -> String {
    format!(
        "{}完成 · 消息 {before_messages} → {after_messages} 条 · tokens {} → {} · 耗时 {duration_ms}ms",
        trigger.label(),
        format_decimal_count(before_tokens),
        format_decimal_count(after_tokens)
    )
}

/// 压缩失败摘要。
pub(super) fn failed_summary(trigger: CompactionTrigger, reason: &str) -> String {
    format!("{}未完成：{reason}；当前任务历史保持不变", trigger.label())
}

/// 空闲轮转排期摘要。
pub(super) fn planned_summary(
    current_context_tokens: u64,
    trigger_tokens: u64,
    debounce_ms: u64,
) -> String {
    format!(
        "空闲窗口轮转已排期 · 当前上下文 {} tokens ≥ 触发线 {} · {debounce_ms}ms 空闲后执行",
        format_decimal_count(current_context_tokens),
        format_decimal_count(trigger_tokens)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn started_and_completed_share_the_same_part_id() {
        let part_id = compaction_part_id(CompactionTrigger::Inline, "session-1");
        let started = CompactionLifecycleView::started(part_id.clone(), "开始".to_owned(), 10);
        let completed = CompactionLifecycleView::completed(
            part_id.clone(),
            "完成".to_owned(),
            Some(10),
            Some(5),
        );

        assert_eq!(started.part_id, completed.part_id);
        assert!(!is_text_projectable_kind(started.kind));
        assert!(is_text_projectable_kind(completed.kind));
    }

    #[test]
    fn window_part_id_separates_successive_rollovers() {
        let first = compaction_window_part_id(CompactionTrigger::Idle, "session-1", 2);
        let second = compaction_window_part_id(CompactionTrigger::Idle, "session-1", 3);

        assert_ne!(first, second);
        assert!(first.starts_with(&compaction_part_id(CompactionTrigger::Idle, "session-1")));
    }

    #[test]
    fn compaction_kinds_are_recognised_for_text_projection() {
        for kind in [
            COMPACTION_STARTED_KIND,
            COMPACTION_COMPLETED_KIND,
            COMPACTION_FAILED_KIND,
            COMPACTION_PLANNED_KIND,
        ] {
            assert!(is_compaction_kind(kind), "{kind} 应被识别为压缩类别");
        }
        assert!(!is_compaction_kind("RunLifecycle"));
    }

    #[test]
    fn completed_summary_carries_message_and_token_delta() {
        let summary = completed_summary(CompactionTrigger::Idle, 118_300, 9_240, 42, 6, 312);

        assert!(summary.contains("空闲窗口轮转完成"), "{summary}");
        assert!(summary.contains("消息 42 → 6 条"), "{summary}");
        assert!(summary.contains("tokens 118K → 9.24K"), "{summary}");
        assert!(summary.contains("耗时 312ms"), "{summary}");
    }
}
