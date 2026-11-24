//! Agent TUI 会话级完成摘要。

use std::time::Duration;

use axon_time::duration_ms;
use serde::Serialize;

use super::{
    duration_format::format_elapsed,
    run_output::{AgentTuiRuntimeEvent, AgentTuiRuntimePayload, AgentTuiTokenUsage},
};

/// Agent TUI 会话级摘要。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTuiSessionSummary {
    /// 当前 run session id。
    pub session_id: Option<String>,
    /// 模型生成的会话标题。
    pub title: Option<String>,
    /// 本轮恢复自哪个 session。
    pub resumed_from: Option<String>,
    /// 已提交给 runtime 的轮次数。
    pub turns: u64,
    /// 失败轮次数。
    pub failed_turns: u64,
    /// 工具启动次数。
    pub tool_starts: u64,
    /// 工具输出分块数量。
    pub tool_output_chunks: u64,
    /// 累计 token 使用。
    pub token_usage: AgentTuiTokenUsage,
    /// 累计 runtime 执行耗时毫秒。
    pub elapsed_ms: u64,
}

impl AgentTuiSessionSummary {
    pub(super) fn record_session(
        &mut self,
        session_id: String,
        resumed_from: Option<String>,
        title: Option<String>,
    ) {
        self.session_id = Some(session_id);
        self.resumed_from = resumed_from;
        if let Some(title) = normalize_session_title(title.as_deref()) {
            self.title = Some(title);
        }
    }

    pub(super) fn record_turn(
        &mut self,
        failed: bool,
        usage: AgentTuiTokenUsage,
        elapsed: Duration,
    ) {
        self.turns = self.turns.saturating_add(1);
        if failed {
            self.failed_turns = self.failed_turns.saturating_add(1);
        }
        self.token_usage.add_assign(usage);
        self.elapsed_ms = self.elapsed_ms.saturating_add(duration_ms(elapsed));
    }

    pub(super) fn record_runtime_event(&mut self, event: &AgentTuiRuntimeEvent) {
        match &event.payload {
            AgentTuiRuntimePayload::WireEvent { event, .. } => self.record_wire_event(event),
            AgentTuiRuntimePayload::BatchedAssistantDelta { .. }
            | AgentTuiRuntimePayload::ReasoningPartStarted { .. }
            | AgentTuiRuntimePayload::BatchedReasoningDelta { .. }
            | AgentTuiRuntimePayload::ReasoningPartEnded { .. }
            | AgentTuiRuntimePayload::WireResumeSnapshot(_)
            | AgentTuiRuntimePayload::WireRuntimeError(_)
            | AgentTuiRuntimePayload::Watcher(_)
            | AgentTuiRuntimePayload::CancelRequested(_)
            | AgentTuiRuntimePayload::InterruptRequested(_)
            | AgentTuiRuntimePayload::SubscriptionPhase(_)
            | AgentTuiRuntimePayload::Observation(_) => {}
        }
    }

    fn record_wire_event(&mut self, event: &axon_protocol::wire::WireEventEnvelope) {
        match &event.kind {
            axon_protocol::wire::WireEventKind::ToolCall(_) => {
                self.tool_starts = self.tool_starts.saturating_add(1);
            }
            axon_protocol::wire::WireEventKind::ToolResult(result) if result.stream.is_some() => {
                self.tool_output_chunks = self.tool_output_chunks.saturating_add(1);
            }
            _ => {}
        }
    }

    /// 是否已经观察到真实 runtime 活动。
    #[must_use]
    pub fn has_activity(&self) -> bool {
        self.turns > 0
            || self.tool_starts > 0
            || self.tool_output_chunks > 0
            || self.token_usage.has_tokens()
            || self.elapsed_ms > 0
    }

    /// 渲染为适合退出报告和输入区下方状态行复用的中文摘要。
    #[must_use]
    pub fn render_text(&self) -> String {
        if !self.has_activity() {
            return "本会话尚未提交任务。".to_owned();
        }
        let mut parts = vec![format!("轮次 {}", self.turns)];
        if let Some(session_id) = self.session_id.as_deref() {
            parts.push(format!("session={session_id}"));
        }
        if let Some(title) = self.title.as_deref() {
            parts.push(format!("标题={title}"));
        }
        if let Some(parent) = self.resumed_from.as_deref() {
            parts.push(format!("恢复自={parent}"));
        }
        if self.failed_turns > 0 {
            parts.push(format!("失败={}", self.failed_turns));
        }
        if self.tool_starts > 0 {
            parts.push(format!("工具 {}", self.tool_starts));
        }
        if self.token_usage.has_tokens() {
            parts.push(format!("用量 {}", self.token_usage.render_total()));
        }
        if self.elapsed_ms > 0 {
            parts.push(format!(
                "耗时={}",
                format_elapsed(Duration::from_millis(self.elapsed_ms))
            ));
        }
        parts.join(" · ")
    }
}

fn normalize_session_title(value: Option<&str>) -> Option<String> {
    let title = value?.split_whitespace().collect::<Vec<_>>().join(" ");
    let title = title.trim();
    (!title.is_empty()).then(|| title.to_owned())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::AgentTuiSessionSummary;
    use crate::r#loop::agent_tui::AgentTuiTokenUsage;

    #[test]
    fn render_text_uses_compact_token_and_duration_units() {
        let mut summary = AgentTuiSessionSummary::default();
        summary.record_turn(
            false,
            AgentTuiTokenUsage::new(5_009, 0, 0, 104, 0),
            Duration::from_secs(65),
        );

        let text = summary.render_text();

        assert!(text.contains("用量 5.11K"));
        assert!(text.contains("耗时=1m 05s"));
        assert!(!text.contains("5113"));
        assert!(!text.contains("65000ms"));
    }
}
