//! Provider streaming 增量归一化为 run progress wire 事件。

use std::path::PathBuf;

use axon_agent_runtime::{
    ProviderStreamPartialNormalizer, ProviderStreamProtocol, ProviderTextToolCallProjection,
    agent_run_assistant_part_id, agent_run_provider_assistant_id, agent_run_provider_reasoning_id,
    agent_run_reasoning_part_id,
};

use super::super::run_progress_streaming_diff_preview::RunProgressStreamingDiffPreview;
use super::RunProgressSink;

/// 从 provider SSE / JSONL 行提取可实时显示的增量。
#[derive(Debug)]
pub(crate) struct RunProgressStreamNormalizer {
    partial_normalizer: ProviderStreamPartialNormalizer,
    text_tool_call_projection: ProviderTextToolCallProjection,
    assistant_part_id: String,
    assistant_message_id: String,
    reasoning_base_part_id: String,
    reasoning_part_id: String,
    reasoning_id: String,
    reasoning_event_key: Option<String>,
    reasoning_part_serial: u64,
    reasoning_part_ended: bool,
    tool_diff_preview: RunProgressStreamingDiffPreview,
}

impl RunProgressStreamNormalizer {
    /// `provider_turn_id` 由调用方保证在本 run 内唯一：渠道 failover 会从上一尝试之后接着
    /// 编号，assistant/reasoning part 身份都从它派生，跨尝试复用会把两段正文拼进同一个
    /// 展示单元与同一条失败诊断记录。
    pub(crate) fn new(
        protocol: ProviderStreamProtocol,
        provider_turn_id: u64,
        workspace_root: PathBuf,
    ) -> Self {
        Self {
            partial_normalizer: ProviderStreamPartialNormalizer::new(protocol),
            text_tool_call_projection: ProviderTextToolCallProjection::default(),
            assistant_part_id: agent_run_assistant_part_id(provider_turn_id),
            assistant_message_id: agent_run_provider_assistant_id(provider_turn_id),
            reasoning_base_part_id: agent_run_reasoning_part_id(provider_turn_id),
            reasoning_part_id: agent_run_reasoning_part_id(provider_turn_id),
            reasoning_id: agent_run_provider_reasoning_id(provider_turn_id),
            reasoning_event_key: None,
            reasoning_part_serial: 0,
            reasoning_part_ended: false,
            tool_diff_preview: RunProgressStreamingDiffPreview::new(
                workspace_root,
                provider_turn_id,
            ),
        }
    }

    pub(crate) fn consume_line(&mut self, line: &str, sink: &RunProgressSink<'_>) -> bool {
        let previews = self
            .tool_diff_preview
            .consume_line(self.partial_normalizer.protocol(), line);
        for preview in previews {
            sink.tool_diff_snapshot(preview);
        }
        if let Some(usage) = self.partial_normalizer.partial_token_usage(line) {
            sink.token_usage_updated(usage);
        }
        let Ok(report) = self.partial_normalizer.normalize(line) else {
            return false;
        };
        if !report.reasoning_deltas.is_empty() || report.reasoning_end {
            self.select_reasoning_part(report.reasoning_event_key.as_deref(), sink);
        }
        let text_delta_count = report.deltas.len();
        let mut generated = !report.reasoning_deltas.is_empty();
        for delta in report.deltas {
            let visible = self.text_tool_call_projection.consume(&delta.content_delta);
            if !visible.is_empty() {
                generated = true;
                sink.assistant_delta(
                    self.assistant_part_id.clone(),
                    self.assistant_message_id.clone(),
                    visible,
                );
            }
        }
        for delta in report.reasoning_deltas {
            sink.reasoning_delta(
                self.reasoning_part_id.clone(),
                self.reasoning_id.clone(),
                delta.content_delta,
            );
        }
        if report.reasoning_end {
            sink.end_reasoning(self.reasoning_part_id.clone(), self.reasoning_id.clone());
            self.reasoning_part_ended = true;
        }
        self.partial_normalizer.commit_text_deltas(text_delta_count);
        generated
    }

    fn select_reasoning_part(&mut self, event_key: Option<&str>, sink: &RunProgressSink<'_>) {
        let same_event = self.reasoning_event_key.as_deref() == event_key;
        if same_event && !self.reasoning_part_ended {
            return;
        }
        if !self.reasoning_part_ended && self.reasoning_event_key.is_some() {
            sink.end_reasoning(self.reasoning_part_id.clone(), self.reasoning_id.clone());
        }
        if self.reasoning_event_key.is_some() || self.reasoning_part_ended {
            self.reasoning_part_serial = self.reasoning_part_serial.saturating_add(1);
            self.reasoning_part_id = format!(
                "{}:{}",
                self.reasoning_base_part_id, self.reasoning_part_serial
            );
        }
        self.reasoning_event_key = event_key.map(str::to_owned);
        self.reasoning_part_ended = false;
        if event_key.is_none() && self.reasoning_part_serial > 0 {
            self.reasoning_event_key = None;
        }
    }

    /// 结束实时投影，将未形成合法工具调用的缓冲内容按正文发出。
    pub(crate) fn finish(&mut self, sink: &RunProgressSink<'_>) -> bool {
        let visible = std::mem::take(&mut self.text_tool_call_projection).finish();
        if visible.is_empty() {
            return false;
        }
        sink.assistant_delta(
            self.assistant_part_id.clone(),
            self.assistant_message_id.clone(),
            visible,
        );
        true
    }
}
