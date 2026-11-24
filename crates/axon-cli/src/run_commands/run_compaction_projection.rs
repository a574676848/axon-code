//! 压缩（代际窗口轮转）生命周期投影的单一事实源。
//!
//! 所有压缩路径都必须通过本模块产出同构的 wire 事实，避免再次出现
//! 「事件已经发出、却没有任何消费者识别」的投影断层：
//! - `CompactionStarted` / `CompactionCompleted` / `CompactionFailed`：
//!   TUI history 生命周期（`runtime_history_lifecycle.rs`）与 `axon run` 文本投影共同识别；
//! - `WireEventKind::ContextRollover`：窗口序号、归档规模与便签注入结果的权威协议事实
//!   （此前协议与 TUI 消费端齐备，却没有任何生产者）。

use axon_protocol::wire::{WireContextRollover, WireLifecycleStatus};

use super::run_progress::{RunLifecycleStatusView, RunProgressSink};

/// TUI history 生命周期与文本投影共同识别的压缩进行中类别。
pub(super) const COMPACTION_STARTED_KIND: &str = "CompactionStarted";
/// 压缩成功类别；TUI 以 terminal 语义原位替换同 part id 的进行中 cell。
pub(super) const COMPACTION_COMPLETED_KIND: &str = "CompactionCompleted";
/// 压缩失败类别。
pub(super) const COMPACTION_FAILED_KIND: &str = "CompactionFailed";

/// 压缩生命周期所属展示阶段；TUI 依赖该值把压缩归入 history 区域。
pub(super) const COMPACTION_UI_PHASE: &str = "history";
/// 生命周期标题。
pub(super) const COMPACTION_TITLE: &str = "窗口轮转";
/// `axon run` 文本投影的压缩行前缀，与 assistant 正文区分。
pub(super) const COMPACTION_TEXT_PREFIX: &str = "· ";

/// part id 前缀；同一轮转的开始/完成/失败必须复用同一 id，TUI 才能原位替换 cell。
const COMPACTION_PART_PREFIX: &str = "lifecycle:compaction";
/// 压缩触发码。
///
/// 当前只剩 turn 内联轮转一个触发源，但这一段属于 part id 契约：CLI 回归用例与
/// 桌面端渲染测试都按 `lifecycle:compaction:inline:<session>:<window>` 拼装，不能省略。
const TRIGGER_CODE_INLINE: &str = "inline";
/// 面向用户的触发来源展示名。
const TRIGGER_LABEL_INLINE: &str = "内联窗口轮转";

/// 压缩生命周期投影视图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CompactionLifecycleView {
    pub(super) part_id: String,
    /// 这次换窗属于哪一轮；界面的历史行据此归位。
    pub(super) turn_id: String,
    pub(super) kind: &'static str,
    pub(super) title: String,
    pub(super) summary: String,
    pub(super) status: WireLifecycleStatus,
    pub(super) started_at_ms: Option<u64>,
    pub(super) duration_ms: Option<u64>,
}

impl CompactionLifecycleView {
    /// 压缩开始；part id 在同一轮转内保持稳定。
    pub(super) fn started(
        part_id: String,
        turn_id: String,
        summary: String,
        started_at_ms: u64,
    ) -> Self {
        Self {
            part_id,
            turn_id,
            kind: COMPACTION_STARTED_KIND,
            title: COMPACTION_TITLE.to_owned(),
            summary,
            status: WireLifecycleStatus::Running,
            started_at_ms: Some(started_at_ms),
            duration_ms: None,
        }
    }

    /// 压缩成功；携带量化结果便于排查「模型失忆」。
    pub(super) fn completed(
        part_id: String,
        turn_id: String,
        summary: String,
        started_at_ms: Option<u64>,
        duration_ms: Option<u64>,
    ) -> Self {
        Self {
            part_id,
            turn_id,
            kind: COMPACTION_COMPLETED_KIND,
            title: COMPACTION_TITLE.to_owned(),
            summary,
            status: WireLifecycleStatus::Completed,
            started_at_ms,
            duration_ms,
        }
    }

    /// 压缩失败；摘要必须显式声明当前历史保持不变。
    ///
    /// 只用于真正的失败（归档、提交或 replacement 校验）。「便签尚未保存」属于运行时的
    /// 可自愈推迟，不在这里发布。
    pub(super) fn failed(
        part_id: String,
        turn_id: String,
        summary: String,
        started_at_ms: Option<u64>,
        duration_ms: Option<u64>,
    ) -> Self {
        Self {
            part_id,
            turn_id,
            kind: COMPACTION_FAILED_KIND,
            title: COMPACTION_TITLE.to_owned(),
            summary,
            status: WireLifecycleStatus::Failed,
            started_at_ms,
            duration_ms,
        }
    }
}

/// 是否为 `axon run` 文本投影需要输出的压缩类别；进行中的瞬态不输出，避免同一轮转两行。
pub(super) fn is_text_projectable_kind(kind: &str) -> bool {
    matches!(kind, COMPACTION_COMPLETED_KIND | COMPACTION_FAILED_KIND)
}

/// 是否为压缩生命周期类别；文本投影据此识别压缩行。
pub(super) fn is_compaction_kind(kind: &str) -> bool {
    matches!(
        kind,
        COMPACTION_STARTED_KIND | COMPACTION_COMPLETED_KIND | COMPACTION_FAILED_KIND
    )
}

/// 单一轮转的稳定 part id；开始与结束共用，TUI 才能把「进行中」替换为「完成」。
///
/// 身份取自这次换窗本身（会话 + 目标窗口号）：同一会话的多次换窗因此各有各的行，
/// 不会在历史里互相覆盖，也不再需要按轮次另行区分。
pub(super) fn compaction_part_id(session_id: &str, window_number: u64) -> String {
    format!("{COMPACTION_PART_PREFIX}:{TRIGGER_CODE_INLINE}:{session_id}:{window_number}")
}

/// 从轮转的领域身份还原 part id；身份缺失时返回 `None`。
pub(super) fn compaction_part_id_from_lifecycle(lifecycle_id: &str) -> Option<String> {
    let rest = lifecycle_id.strip_prefix("inline-rollover:")?;
    Some(format!(
        "{COMPACTION_PART_PREFIX}:{TRIGGER_CODE_INLINE}:{rest}"
    ))
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
    )
    .with_turn_id(view.turn_id.clone());
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
pub(super) fn started_summary() -> String {
    format!("{TRIGGER_LABEL_INLINE}已开始 · 正在整理代际便签与会话目标锚点")
}

/// 压缩成功摘要；覆盖「历史被裁剪、checkpoint 建立」的全部可核对事实。
///
/// 不写任何 token 读数：换窗前后一律只认 Provider 实测，而新窗口在换窗瞬间还没有请求，
/// 拿不到成对的实测值；本地估算不再作为界面读数出现。
pub(super) fn completed_summary(
    before_messages: usize,
    after_messages: usize,
    duration_ms: u64,
) -> String {
    format!(
        "{TRIGGER_LABEL_INLINE}完成 · 消息 {before_messages} → {after_messages} 条 · 耗时 {duration_ms}ms"
    )
}

/// 压缩失败摘要。
pub(super) fn failed_summary(reason: &str) -> String {
    format!("{TRIGGER_LABEL_INLINE}未完成：{reason}；当前任务历史保持不变")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn started_and_completed_share_the_same_part_id() {
        let part_id = compaction_part_id("session-1", 3);
        let started = CompactionLifecycleView::started(
            part_id.clone(),
            "7".to_owned(),
            "开始".to_owned(),
            10,
        );
        let completed = CompactionLifecycleView::completed(
            part_id.clone(),
            "7".to_owned(),
            "完成".to_owned(),
            Some(10),
            Some(5),
        );

        assert_eq!(started.part_id, completed.part_id);
        assert_eq!(started.turn_id, "7");
        assert!(!is_text_projectable_kind(started.kind));
        assert!(is_text_projectable_kind(completed.kind));
    }

    /// part id 是外部契约：身份取自这次换窗本身（会话 + 目标窗口号）。
    /// 真实失败（归档、提交或 replacement 校验）仍然发布失败事实，并带上轮次与窗口身份。
    #[test]
    fn failed_lifecycle_keeps_turn_and_window_identity() {
        let view = CompactionLifecycleView::failed(
            compaction_part_id("session-1", 2),
            "9".to_owned(),
            failed_summary("窗口归档提交失败"),
            Some(1_000),
            Some(20),
        );

        assert_eq!(view.status, WireLifecycleStatus::Failed);
        assert_eq!(view.turn_id, "9");
        assert_eq!(view.part_id, "lifecycle:compaction:inline:session-1:2");
        assert!(view.summary.contains("当前任务历史保持不变"));
    }

    #[test]
    fn part_id_carries_the_window_being_rolled_over() {
        assert_eq!(
            compaction_part_id("session-1", 3),
            "lifecycle:compaction:inline:session-1:3"
        );
        assert_eq!(
            compaction_part_id_from_lifecycle("inline-rollover:session-1:3").as_deref(),
            Some("lifecycle:compaction:inline:session-1:3")
        );
        assert_eq!(compaction_part_id_from_lifecycle("unknown:id"), None);
    }

    #[test]
    fn compaction_kinds_are_recognised_for_text_projection() {
        for kind in [
            COMPACTION_STARTED_KIND,
            COMPACTION_COMPLETED_KIND,
            COMPACTION_FAILED_KIND,
        ] {
            assert!(is_compaction_kind(kind), "{kind} 应被识别为压缩类别");
        }
        assert!(!is_compaction_kind("RunLifecycle"));
    }

    #[test]
    fn completed_summary_carries_message_delta_only() {
        let summary = completed_summary(42, 6, 312);

        assert!(summary.contains("内联窗口轮转完成"), "{summary}");
        assert!(summary.contains("消息 42 → 6 条"), "{summary}");
        assert!(summary.contains("耗时 312ms"), "{summary}");
        assert!(
            !summary.contains("tokens"),
            "换窗摘要不再呈现任何 token 读数：{summary}"
        );
    }
}
