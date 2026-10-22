//! Feishu Agent 回复 artifact 的 delivery 执行。

use std::env;

use axon_crypto::sha256_hex_prefix;
use axon_display::{parse_fence_marker, strip_blockquote_prefix};
use axon_domain::{AxonError, AxonResult};

use crate::remote_feishu_delivery_http::{
    execute_message_request_with_fallback, execution_report_from_message_response, json_headers,
};
use crate::remote_feishu_delivery_types::{
    RemoteFeishuDeliveryExecutionReport, RemoteFeishuDeliveryPlan, RemoteFeishuHttpRequestPlan,
    RemoteFeishuHttpResponse, RemoteFeishuMessageTarget,
};
use crate::remote_feishu_outbound::CONTENT_TYPE_MARKDOWN;
use crate::{
    RemoteFeishuAgentReplyArtifactRecord, RemoteFeishuOapiExecutorRuntime,
    RemoteFeishuWebSocketDispatchPlan, RemoteNotificationAdapterSendStatus,
    RemoteNotificationOutboxRecord, RemoteNotificationRenderArtifact,
};

/// 单条 Feishu Agent 回复消息的最大字符数。
pub const REMOTE_FEISHU_AGENT_REPLY_MESSAGE_MAX_CHARS: usize = 8_000;
const REMOTE_FEISHU_AGENT_REPLY_SEGMENT_HASH_CHARS: usize = 24;
const REMOTE_FEISHU_AGENT_REPLY_SEGMENT_PREFIX: &str = "feishu-reply";

/// 基于 Agent 回复 artifact 生成有序 Feishu Markdown 回复请求计划。
pub fn plan_agent_reply_requests(
    dispatch: &RemoteFeishuWebSocketDispatchPlan,
    api_base_url: Option<&str>,
    accepted_record: &RemoteNotificationOutboxRecord,
    artifact: &RemoteFeishuAgentReplyArtifactRecord,
) -> AxonResult<Vec<RemoteFeishuDeliveryPlan>> {
    split_agent_reply_body(&artifact.body)?
        .into_iter()
        .enumerate()
        .map(|(index, body)| {
            let segment_dispatch = dispatch_with_markdown_artifact(dispatch, &body)?;
            let segment_record = segment_record(accepted_record, index);
            crate::RemoteFeishuDeliveryRuntime::plan_requests_for_target(
                &segment_dispatch,
                api_base_url,
                &segment_record,
                &segment_target(artifact, index),
            )
        })
        .collect()
}

/// 使用显式 secret 执行 Agent 回复 artifact 投递。
pub fn execute_agent_reply_with_secret(
    dispatch: &RemoteFeishuWebSocketDispatchPlan,
    app_secret: &str,
    api_base_url: Option<&str>,
    accepted_record: &RemoteNotificationOutboxRecord,
    artifact: &RemoteFeishuAgentReplyArtifactRecord,
    sent_at_ms: u64,
) -> AxonResult<RemoteFeishuDeliveryExecutionReport> {
    execute_agent_reply_with_http_and_secret(
        dispatch,
        app_secret,
        api_base_url,
        accepted_record,
        artifact,
        sent_at_ms,
        RemoteFeishuOapiExecutorRuntime::execute_json,
    )
}

/// 使用环境变量 secret 执行 Agent 回复 artifact 投递。
pub fn execute_agent_reply(
    dispatch: &RemoteFeishuWebSocketDispatchPlan,
    api_base_url: Option<&str>,
    accepted_record: &RemoteNotificationOutboxRecord,
    artifact: &RemoteFeishuAgentReplyArtifactRecord,
    sent_at_ms: u64,
) -> AxonResult<RemoteFeishuDeliveryExecutionReport> {
    let app_secret = env::var(&dispatch.app_secret_env)
        .map_err(|_| AxonError::runtime_failure("Feishu app_secret 环境变量未设置"))?;
    execute_agent_reply_with_http_and_secret(
        dispatch,
        &app_secret,
        api_base_url,
        accepted_record,
        artifact,
        sent_at_ms,
        RemoteFeishuOapiExecutorRuntime::execute_json,
    )
}

/// 使用注入 HTTP 与显式 secret 执行 Agent 回复 artifact 投递。
pub fn execute_agent_reply_with_http_and_secret<F>(
    dispatch: &RemoteFeishuWebSocketDispatchPlan,
    app_secret: &str,
    api_base_url: Option<&str>,
    accepted_record: &RemoteNotificationOutboxRecord,
    artifact: &RemoteFeishuAgentReplyArtifactRecord,
    sent_at_ms: u64,
    mut http: F,
) -> AxonResult<RemoteFeishuDeliveryExecutionReport>
where
    F: FnMut(&RemoteFeishuHttpRequestPlan) -> AxonResult<RemoteFeishuHttpResponse>,
{
    let plans = plan_agent_reply_requests(dispatch, api_base_url, accepted_record, artifact)?;
    let Some(first_plan) = plans.first() else {
        return Err(AxonError::runtime_failure("Feishu Agent 回复分段计划为空"));
    };
    let tenant_access_token =
        crate::RemoteFeishuDeliveryRuntime::fetch_tenant_access_token_with_http_and_secret(
            &dispatch.app_id,
            app_secret,
            &first_plan.token_request,
            &mut http,
        )?;
    execute_agent_reply_plans(
        plans,
        &tenant_access_token,
        accepted_record,
        sent_at_ms,
        &mut http,
    )
}

fn execute_agent_reply_plans<F>(
    plans: Vec<RemoteFeishuDeliveryPlan>,
    tenant_access_token: &str,
    accepted_record: &RemoteNotificationOutboxRecord,
    sent_at_ms: u64,
    http: &mut F,
) -> AxonResult<RemoteFeishuDeliveryExecutionReport>
where
    F: FnMut(&RemoteFeishuHttpRequestPlan) -> AxonResult<RemoteFeishuHttpResponse>,
{
    let mut last_report = None;
    for plan in plans {
        let message_request = RemoteFeishuHttpRequestPlan {
            headers: json_headers(Some(tenant_access_token)),
            ..plan.message_request
        };
        let fallback_message_request =
            plan.fallback_message_request
                .map(|request| RemoteFeishuHttpRequestPlan {
                    headers: json_headers(Some(tenant_access_token)),
                    ..request
                });
        let message_response = execute_message_request_with_fallback(
            &message_request,
            fallback_message_request.as_ref(),
            http,
        )?;
        let report =
            execution_report_from_message_response(accepted_record, sent_at_ms, &message_response)?;
        let delivered = matches!(
            report.send_status,
            RemoteNotificationAdapterSendStatus::Delivered
        );
        last_report = Some(report);
        if !delivered {
            break;
        }
    }
    last_report.ok_or_else(|| AxonError::runtime_failure("Feishu Agent 回复未执行任何分段"))
}

pub(crate) fn split_agent_reply_body(body: &str) -> AxonResult<Vec<String>> {
    if body.trim().is_empty() {
        return Err(AxonError::runtime_failure("Feishu Agent 回复正文为空"));
    }
    let mut splitter = MarkdownReplySplitter::default();
    for line in body.split_inclusive('\n') {
        splitter.push_line(line);
    }
    splitter.finish();
    Ok(splitter.segments)
}

#[derive(Debug, Clone)]
struct ActiveFence {
    opening: String,
    closing: String,
    marker: char,
    marker_len: usize,
}

#[derive(Debug, Default)]
struct MarkdownReplySplitter {
    segments: Vec<String>,
    current: String,
    current_chars: usize,
    active_fence: Option<ActiveFence>,
}

impl MarkdownReplySplitter {
    fn push_line(&mut self, line: &str) {
        match fence_transition(line, self.active_fence.as_ref()) {
            FenceTransition::Open(fence) => {
                self.ensure_room(line.chars().count() + fence.closing.chars().count());
                self.push_raw(line);
                self.active_fence = Some(fence);
            }
            FenceTransition::Close => {
                self.push_closing_line(line);
                self.active_fence = None;
            }
            FenceTransition::None => self.push_content(line),
        }
    }

    fn push_content(&mut self, mut content: &str) {
        while !content.is_empty() {
            let reserve = self.closing_reserve();
            let capacity = REMOTE_FEISHU_AGENT_REPLY_MESSAGE_MAX_CHARS
                .saturating_sub(self.current_chars + reserve);
            if capacity == 0 {
                self.flush_segment(true);
                continue;
            }
            let cut = char_prefix_byte_len(content, capacity);
            self.push_raw(&content[..cut]);
            content = &content[cut..];
            if !content.is_empty() {
                self.flush_segment(true);
            }
        }
    }

    fn push_closing_line(&mut self, line: &str) {
        if self.current_chars + line.chars().count() > REMOTE_FEISHU_AGENT_REPLY_MESSAGE_MAX_CHARS {
            self.flush_segment(true);
        }
        self.push_raw(line);
    }

    fn ensure_room(&mut self, required: usize) {
        if !self.current.is_empty()
            && self.current_chars + required > REMOTE_FEISHU_AGENT_REPLY_MESSAGE_MAX_CHARS
        {
            self.flush_segment(true);
        }
    }

    fn closing_reserve(&self) -> usize {
        self.active_fence
            .as_ref()
            .map_or(0, |fence| fence.closing.chars().count())
    }

    fn flush_segment(&mut self, reopen_fence: bool) {
        if self.current.is_empty() {
            return;
        }
        if let Some(fence) = &self.active_fence {
            self.current.push_str(&fence.closing);
        }
        self.segments.push(std::mem::take(&mut self.current));
        self.current_chars = 0;
        if reopen_fence && let Some(fence) = &self.active_fence {
            self.current.push_str(&fence.opening);
            self.current_chars = fence.opening.chars().count();
        }
    }

    fn push_raw(&mut self, value: &str) {
        self.current.push_str(value);
        self.current_chars += value.chars().count();
    }

    fn finish(&mut self) {
        self.flush_segment(false);
    }
}

#[derive(Debug)]
enum FenceTransition {
    Open(ActiveFence),
    Close,
    None,
}

fn fence_transition(line: &str, active: Option<&ActiveFence>) -> FenceTransition {
    let trimmed = line.trim_end_matches(['\n', '\r']);
    let scan = strip_blockquote_prefix(trimmed.trim_start());
    let Some((marker, marker_len)) = parse_fence_marker(scan) else {
        return FenceTransition::None;
    };
    let tail = fence_tail(scan, marker_len);
    if active.is_some_and(|fence| {
        marker == fence.marker && marker_len >= fence.marker_len && tail.is_empty()
    }) {
        return FenceTransition::Close;
    }
    if active.is_some() {
        return FenceTransition::None;
    }
    let prefix_len = trimmed.len().saturating_sub(scan.len());
    let prefix = &trimmed[..prefix_len];
    let opening = format!("{trimmed}\n");
    let closing = format!("{prefix}{}\n", marker.to_string().repeat(marker_len));
    if opening.chars().count() + closing.chars().count()
        >= REMOTE_FEISHU_AGENT_REPLY_MESSAGE_MAX_CHARS
    {
        return FenceTransition::None;
    }
    FenceTransition::Open(ActiveFence {
        opening,
        closing,
        marker,
        marker_len,
    })
}

fn fence_tail(line: &str, marker_len: usize) -> &str {
    let marker_end = line
        .char_indices()
        .nth(marker_len)
        .map_or(line.len(), |(index, _)| index);
    line[marker_end..].trim()
}

fn char_prefix_byte_len(value: &str, max_chars: usize) -> usize {
    value
        .char_indices()
        .nth(max_chars)
        .map_or(value.len(), |(index, _)| index)
}

fn segment_target(
    artifact: &RemoteFeishuAgentReplyArtifactRecord,
    index: usize,
) -> RemoteFeishuMessageTarget {
    let mut target = artifact.target();
    if index > 0 {
        target.reply_to_message_id = None;
    }
    target
}

fn segment_record(
    accepted_record: &RemoteNotificationOutboxRecord,
    index: usize,
) -> RemoteNotificationOutboxRecord {
    let mut record = accepted_record.clone();
    if index > 0 {
        let source = format!("{}:{index}", accepted_record.dispatch_id);
        record.dispatch_id = format!(
            "{REMOTE_FEISHU_AGENT_REPLY_SEGMENT_PREFIX}-{}",
            sha256_hex_prefix(
                source.as_bytes(),
                REMOTE_FEISHU_AGENT_REPLY_SEGMENT_HASH_CHARS
            )
        );
    }
    record
}

fn dispatch_with_markdown_artifact(
    dispatch: &RemoteFeishuWebSocketDispatchPlan,
    body: &str,
) -> AxonResult<RemoteFeishuWebSocketDispatchPlan> {
    if body.trim().is_empty() {
        return Err(AxonError::runtime_failure("Feishu Agent 回复正文为空"));
    }
    Ok(RemoteFeishuWebSocketDispatchPlan {
        artifact: RemoteNotificationRenderArtifact {
            adapter: dispatch.artifact.adapter,
            transport: dispatch.artifact.transport,
            content_type: CONTENT_TYPE_MARKDOWN.to_owned(),
            payload: body.to_owned(),
            network_send_allowed: dispatch.artifact.network_send_allowed,
        },
        ..dispatch.clone()
    })
}
