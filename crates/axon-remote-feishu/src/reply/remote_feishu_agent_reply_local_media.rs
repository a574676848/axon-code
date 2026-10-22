//! Feishu Agent 回复本地媒体投递。

use std::env;

use axon_agent_runtime::AgentReplyAttachmentRuntime;
use axon_domain::{AxonError, AxonResult};

use crate::{
    RemoteFeishuAgentReplyArtifactRecord, RemoteFeishuDeliveryExecutionReport,
    RemoteFeishuDeliveryMediaExecutionInput, RemoteFeishuDeliveryMediaHttpExecutionInput,
    RemoteFeishuHttpRequestPlan, RemoteFeishuHttpResponse, RemoteFeishuMediaKind,
    RemoteFeishuMultipartHttpRequest, RemoteFeishuOapiExecutorRuntime,
    RemoteFeishuWebSocketDispatchPlan, RemoteNotificationAdapterSendStatus,
    RemoteNotificationOutboxRecord,
};

/// Agent 回复本地媒体 delivery HTTP 执行上下文。
#[derive(Debug, Clone)]
pub struct RemoteFeishuAgentReplyLocalMediaHttpExecutionInput<'a> {
    /// Feishu dispatch plan。
    pub dispatch: &'a RemoteFeishuWebSocketDispatchPlan,
    /// 显式 app_secret。
    pub app_secret: &'a str,
    /// OpenAPI 基址。
    pub api_base_url: Option<&'a str>,
    /// 已接受的 outbox 记录。
    pub accepted_record: &'a RemoteNotificationOutboxRecord,
    /// Agent 回复 artifact。
    pub artifact: &'a RemoteFeishuAgentReplyArtifactRecord,
    /// 发送时间。
    pub sent_at_ms: u64,
}

pub fn execute_agent_reply_local_media_with_secret(
    dispatch: &RemoteFeishuWebSocketDispatchPlan,
    app_secret: &str,
    api_base_url: Option<&str>,
    accepted_record: &RemoteNotificationOutboxRecord,
    artifact: &RemoteFeishuAgentReplyArtifactRecord,
    sent_at_ms: u64,
) -> AxonResult<RemoteFeishuDeliveryExecutionReport> {
    execute_agent_reply_local_media_with_http_and_secret(
        RemoteFeishuAgentReplyLocalMediaHttpExecutionInput {
            dispatch,
            app_secret,
            api_base_url,
            accepted_record,
            artifact,
            sent_at_ms,
        },
        RemoteFeishuOapiExecutorRuntime::execute_json,
        RemoteFeishuOapiExecutorRuntime::execute_multipart,
    )
}

pub fn execute_agent_reply_local_media(
    dispatch: &RemoteFeishuWebSocketDispatchPlan,
    api_base_url: Option<&str>,
    accepted_record: &RemoteNotificationOutboxRecord,
    artifact: &RemoteFeishuAgentReplyArtifactRecord,
    sent_at_ms: u64,
) -> AxonResult<RemoteFeishuDeliveryExecutionReport> {
    let app_secret = env::var(&dispatch.app_secret_env)
        .map_err(|_| AxonError::runtime_failure("Feishu app_secret 环境变量未设置"))?;
    execute_agent_reply_local_media_with_secret(
        dispatch,
        &app_secret,
        api_base_url,
        accepted_record,
        artifact,
        sent_at_ms,
    )
}

pub fn execute_agent_reply_local_media_all_with_secret(
    dispatch: &RemoteFeishuWebSocketDispatchPlan,
    app_secret: &str,
    api_base_url: Option<&str>,
    accepted_record: &RemoteNotificationOutboxRecord,
    artifact: &RemoteFeishuAgentReplyArtifactRecord,
    sent_at_ms: u64,
) -> AxonResult<RemoteFeishuDeliveryExecutionReport> {
    execute_agent_reply_local_media_all_with_http_and_secret(
        RemoteFeishuAgentReplyLocalMediaHttpExecutionInput {
            dispatch,
            app_secret,
            api_base_url,
            accepted_record,
            artifact,
            sent_at_ms,
        },
        RemoteFeishuOapiExecutorRuntime::execute_json,
        RemoteFeishuOapiExecutorRuntime::execute_multipart,
    )
}

pub fn execute_agent_reply_local_media_all(
    dispatch: &RemoteFeishuWebSocketDispatchPlan,
    api_base_url: Option<&str>,
    accepted_record: &RemoteNotificationOutboxRecord,
    artifact: &RemoteFeishuAgentReplyArtifactRecord,
    sent_at_ms: u64,
) -> AxonResult<RemoteFeishuDeliveryExecutionReport> {
    let app_secret = env::var(&dispatch.app_secret_env)
        .map_err(|_| AxonError::runtime_failure("Feishu app_secret 环境变量未设置"))?;
    execute_agent_reply_local_media_all_with_secret(
        dispatch,
        &app_secret,
        api_base_url,
        accepted_record,
        artifact,
        sent_at_ms,
    )
}

pub fn execute_agent_reply_local_media_with_http_and_secret<F, G>(
    input: RemoteFeishuAgentReplyLocalMediaHttpExecutionInput<'_>,
    json_http: F,
    multipart_http: G,
) -> AxonResult<RemoteFeishuDeliveryExecutionReport>
where
    F: FnMut(&RemoteFeishuHttpRequestPlan) -> AxonResult<RemoteFeishuHttpResponse>,
    G: FnMut(&RemoteFeishuMultipartHttpRequest) -> AxonResult<RemoteFeishuHttpResponse>,
{
    let media = agent_reply_local_media(input.artifact)?;
    crate::RemoteFeishuDeliveryRuntime::execute_media_with_http_and_secret(
        RemoteFeishuDeliveryMediaHttpExecutionInput {
            dispatch: input.dispatch,
            app_secret: input.app_secret,
            api_base_url: input.api_base_url,
            accepted_record: input.accepted_record,
            media,
            sent_at_ms: input.sent_at_ms,
        },
        json_http,
        multipart_http,
    )
}

pub fn execute_agent_reply_local_media_all_with_http_and_secret<F, G>(
    input: RemoteFeishuAgentReplyLocalMediaHttpExecutionInput<'_>,
    mut json_http: F,
    mut multipart_http: G,
) -> AxonResult<RemoteFeishuDeliveryExecutionReport>
where
    F: FnMut(&RemoteFeishuHttpRequestPlan) -> AxonResult<RemoteFeishuHttpResponse>,
    G: FnMut(&RemoteFeishuMultipartHttpRequest) -> AxonResult<RemoteFeishuHttpResponse>,
{
    let media_items = agent_reply_local_media_items(input.artifact)?;
    let mut reports = Vec::with_capacity(media_items.len());
    for media in media_items {
        reports.push(
            crate::RemoteFeishuDeliveryRuntime::execute_media_with_http_and_secret(
                RemoteFeishuDeliveryMediaHttpExecutionInput {
                    dispatch: input.dispatch,
                    app_secret: input.app_secret,
                    api_base_url: input.api_base_url,
                    accepted_record: input.accepted_record,
                    media,
                    sent_at_ms: input.sent_at_ms,
                },
                &mut json_http,
                &mut multipart_http,
            )?,
        );
    }
    merge_delivery_reports(input.accepted_record, reports)
}

/// 投递 Agent 回复：可见正文与本地附件在同一契约下统一出站。
///
/// 契约：
/// - 正文按 `MEDIA:` 指令剥离后**始终投递**（非空时），说明与结论不得因附件而丢失；
/// - 附件只在确实存在可投递绝对路径文件时追加投递，路径非法时退化为纯正文；
/// - 两部分结果合并为一份执行报告，任一环节失败即整体失败以便按 dispatch 重试。
pub fn execute_agent_reply_delivery_with_http_and_secret<F, G>(
    input: RemoteFeishuAgentReplyLocalMediaHttpExecutionInput<'_>,
    mut json_http: F,
    mut multipart_http: G,
) -> AxonResult<RemoteFeishuDeliveryExecutionReport>
where
    F: FnMut(&RemoteFeishuHttpRequestPlan) -> AxonResult<RemoteFeishuHttpResponse>,
    G: FnMut(&RemoteFeishuMultipartHttpRequest) -> AxonResult<RemoteFeishuHttpResponse>,
{
    let mut reports = Vec::new();
    let visible_body = AgentReplyAttachmentRuntime::visible_text(&input.artifact.body);
    if !visible_body.trim().is_empty() {
        let text_artifact = text_only_artifact(input.artifact, visible_body);
        reports.push(
            crate::remote_feishu_agent_reply_delivery::execute_agent_reply_with_http_and_secret(
                input.dispatch,
                input.app_secret,
                input.api_base_url,
                input.accepted_record,
                &text_artifact,
                input.sent_at_ms,
                &mut json_http,
            )?,
        );
    }
    for media in existing_local_media_items(input.artifact) {
        reports.push(
            crate::RemoteFeishuDeliveryRuntime::execute_media_with_http_and_secret(
                RemoteFeishuDeliveryMediaHttpExecutionInput {
                    dispatch: input.dispatch,
                    app_secret: input.app_secret,
                    api_base_url: input.api_base_url,
                    accepted_record: input.accepted_record,
                    media,
                    sent_at_ms: input.sent_at_ms,
                },
                &mut json_http,
                &mut multipart_http,
            )?,
        );
    }
    merge_reply_delivery_reports(input.accepted_record, reports)
}

/// 使用显式 secret 投递 Agent 回复的正文与本地附件。
pub fn execute_agent_reply_delivery_with_secret(
    dispatch: &RemoteFeishuWebSocketDispatchPlan,
    app_secret: &str,
    api_base_url: Option<&str>,
    accepted_record: &RemoteNotificationOutboxRecord,
    artifact: &RemoteFeishuAgentReplyArtifactRecord,
    sent_at_ms: u64,
) -> AxonResult<RemoteFeishuDeliveryExecutionReport> {
    execute_agent_reply_delivery_with_http_and_secret(
        RemoteFeishuAgentReplyLocalMediaHttpExecutionInput {
            dispatch,
            app_secret,
            api_base_url,
            accepted_record,
            artifact,
            sent_at_ms,
        },
        RemoteFeishuOapiExecutorRuntime::execute_json,
        RemoteFeishuOapiExecutorRuntime::execute_multipart,
    )
}

/// 使用环境变量 secret 投递 Agent 回复的正文与本地附件。
pub fn execute_agent_reply_delivery(
    dispatch: &RemoteFeishuWebSocketDispatchPlan,
    api_base_url: Option<&str>,
    accepted_record: &RemoteNotificationOutboxRecord,
    artifact: &RemoteFeishuAgentReplyArtifactRecord,
    sent_at_ms: u64,
) -> AxonResult<RemoteFeishuDeliveryExecutionReport> {
    let app_secret = env::var(&dispatch.app_secret_env)
        .map_err(|_| AxonError::runtime_failure("Feishu app_secret 环境变量未设置"))?;
    execute_agent_reply_delivery_with_secret(
        dispatch,
        &app_secret,
        api_base_url,
        accepted_record,
        artifact,
        sent_at_ms,
    )
}

fn text_only_artifact(
    artifact: &RemoteFeishuAgentReplyArtifactRecord,
    visible_body: String,
) -> RemoteFeishuAgentReplyArtifactRecord {
    let mut text_only = artifact.clone();
    text_only.body = visible_body;
    text_only
}

fn merge_reply_delivery_reports(
    accepted_record: &RemoteNotificationOutboxRecord,
    reports: Vec<RemoteFeishuDeliveryExecutionReport>,
) -> AxonResult<RemoteFeishuDeliveryExecutionReport> {
    let mut iter = reports.into_iter();
    let Some(mut merged) = iter.next() else {
        return Err(AxonError::runtime_failure(
            "Feishu Agent 回复正文与附件均为空",
        ));
    };
    for report in iter {
        merged.network_send_attempted |= report.network_send_attempted;
        merged.token_obtained |= report.token_obtained;
        if report.adapter_message_id.is_some() {
            merged.adapter_message_id = report.adapter_message_id;
        }
        if merged.send_status == RemoteNotificationAdapterSendStatus::Delivered {
            merged.send_status = report.send_status;
        }
        merged.feedback.records.extend(report.feedback.records);
        merged.feedback.receipts.extend(report.feedback.receipts);
        merged.feedback.delivered_count += report.feedback.delivered_count;
        merged.feedback.retryable_failure_count += report.feedback.retryable_failure_count;
        merged.feedback.permanent_failure_count += report.feedback.permanent_failure_count;
        merged.feedback.reasons.extend(report.feedback.reasons);
        merged.reasons.extend(report.reasons);
    }
    merged
        .feedback
        .records
        .retain(|record| record.dispatch_id == accepted_record.dispatch_id);
    Ok(merged)
}

fn agent_reply_local_media(
    artifact: &RemoteFeishuAgentReplyArtifactRecord,
) -> AxonResult<RemoteFeishuDeliveryMediaExecutionInput> {
    let file_path = AgentReplyAttachmentRuntime::existing_files(&artifact.body)
        .into_iter()
        .next()
        .ok_or(AxonError::runtime_failure(
            "Feishu Agent 回复未包含可投递本地文件",
        ))?;
    Ok(RemoteFeishuDeliveryMediaExecutionInput {
        kind: media_kind_for_path(&file_path),
        file_path,
        target: Some(artifact.target()),
    })
}

fn agent_reply_local_media_items(
    artifact: &RemoteFeishuAgentReplyArtifactRecord,
) -> AxonResult<Vec<RemoteFeishuDeliveryMediaExecutionInput>> {
    let items = existing_local_media_items(artifact);
    if items.is_empty() {
        return Err(AxonError::runtime_failure(
            "Feishu Agent 回复未包含可投递本地文件",
        ));
    }
    Ok(items)
}

/// 收集确实存在的本地附件投递项；没有可投递文件时返回空集合而不是错误。
///
/// 判定与执行必须共用同一存在性契约：只有真实存在的绝对路径文件才进入附件投递，
/// 否则整条回复会因附件分支失败而完全不投递。
fn existing_local_media_items(
    artifact: &RemoteFeishuAgentReplyArtifactRecord,
) -> Vec<RemoteFeishuDeliveryMediaExecutionInput> {
    AgentReplyAttachmentRuntime::existing_files(&artifact.body)
        .into_iter()
        .map(|file_path| RemoteFeishuDeliveryMediaExecutionInput {
            kind: media_kind_for_path(&file_path),
            file_path,
            target: Some(artifact.target()),
        })
        .collect()
}

fn media_kind_for_path(path: &std::path::Path) -> RemoteFeishuMediaKind {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match extension.as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" => RemoteFeishuMediaKind::Image,
        "mp3" | "wav" | "m4a" | "aac" | "flac" | "ogg" | "opus" => RemoteFeishuMediaKind::Audio,
        "mp4" | "mov" | "avi" | "mkv" | "webm" | "m4v" => RemoteFeishuMediaKind::Media,
        _ => RemoteFeishuMediaKind::File,
    }
}

fn merge_delivery_reports(
    accepted_record: &RemoteNotificationOutboxRecord,
    reports: Vec<RemoteFeishuDeliveryExecutionReport>,
) -> AxonResult<RemoteFeishuDeliveryExecutionReport> {
    let mut iter = reports.into_iter();
    let Some(mut merged) = iter.next() else {
        return Err(AxonError::runtime_failure(
            "Feishu Agent 回复未包含可投递本地文件",
        ));
    };
    for report in iter {
        merged
            .adapter_message_id
            .clone_from(&report.adapter_message_id);
        merged.feedback.records.extend(report.feedback.records);
        merged.feedback.receipts.extend(report.feedback.receipts);
        merged.feedback.delivered_count += report.feedback.delivered_count;
        merged.feedback.retryable_failure_count += report.feedback.retryable_failure_count;
        merged.feedback.permanent_failure_count += report.feedback.permanent_failure_count;
        merged.feedback.reasons.extend(report.feedback.reasons);
        merged.reasons.extend(report.reasons);
    }
    merged
        .feedback
        .records
        .retain(|record| record.dispatch_id == accepted_record.dispatch_id);
    Ok(merged)
}
