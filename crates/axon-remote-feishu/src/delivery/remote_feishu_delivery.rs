//! Feishu 远控出站 delivery 规划与执行。
//!
//! 该模块把 Feishu WebSocket adapter dispatch plan 进一步转成真实可执行的
//! HTTP 请求计划，并把发送结果映射回 outbox feedback。默认仍然 fail-closed，
//! 但也提供真实 HTTP 执行器，供 CLI / worker 在本机显式调用。

use axon_domain::{AxonError, AxonResult, SessionId};
use std::{env, path::PathBuf};

use crate::remote_feishu_delivery_types::{
    REMOTE_FEISHU_API_BASE_URL, REMOTE_FEISHU_APP_SECRET_PLACEHOLDER,
    REMOTE_FEISHU_DELIVERY_REASON_FEEDBACK_PLANNED, REMOTE_FEISHU_DELIVERY_REASON_REQUEST_PLANNED,
    REMOTE_FEISHU_TENANT_ACCESS_TOKEN_PATH, RemoteFeishuDeliveryExecutionReport,
    RemoteFeishuDeliveryPlan, RemoteFeishuHttpRequestPlan, RemoteFeishuHttpResponse,
    RemoteFeishuMessageTarget,
};
use crate::{
    RemoteFeishuAgentReplyArtifactRecord, RemoteFeishuMediaFlowRuntime, RemoteFeishuMediaKind,
    RemoteFeishuMultipartHttpRequest, RemoteFeishuOapiExecutorRuntime,
    RemoteFeishuOutboundMediaMessageInput, RemoteFeishuOutboundMessageRuntime,
    RemoteFeishuWebSocketDispatchPlan, RemoteNotificationAdapterEnvelope,
    RemoteNotificationAdapterEnvelopeReport, RemoteNotificationAdapterEnvelopeSurface,
    RemoteNotificationAdapterSendInput, RemoteNotificationAdapterSendOutcome,
    RemoteNotificationAdapterSendReport, RemoteNotificationAdapterSendRuntime,
    RemoteNotificationOutboxRecord, RemoteTransportKind,
};

/// Feishu 本地媒体 delivery 执行输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuDeliveryMediaExecutionInput {
    /// 媒体类型。
    pub kind: RemoteFeishuMediaKind,
    /// 本地文件路径。
    pub file_path: PathBuf,
    /// 显式发送目标；为空时沿用 outbox connection_id。
    pub target: Option<RemoteFeishuMessageTarget>,
}

/// Feishu 本地媒体 delivery HTTP 执行上下文。
#[derive(Debug, Clone)]
pub struct RemoteFeishuDeliveryMediaHttpExecutionInput<'a> {
    /// Feishu dispatch plan。
    pub dispatch: &'a RemoteFeishuWebSocketDispatchPlan,
    /// 显式 app_secret。
    pub app_secret: &'a str,
    /// OpenAPI 基址。
    pub api_base_url: Option<&'a str>,
    /// 已接受的 outbox 记录。
    pub accepted_record: &'a RemoteNotificationOutboxRecord,
    /// 本地媒体输入。
    pub media: RemoteFeishuDeliveryMediaExecutionInput,
    /// 发送时间。
    pub sent_at_ms: u64,
}

/// Feishu delivery runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct RemoteFeishuDeliveryRuntime;

impl RemoteFeishuDeliveryRuntime {
    /// 生成 Feishu delivery 请求计划。
    ///
    /// # Errors
    ///
    /// 当 app_id/app_secret_env、dispatch payload 或 URL 无效时返回错误。
    pub fn plan_requests(
        dispatch: &RemoteFeishuWebSocketDispatchPlan,
        api_base_url: Option<&str>,
        accepted_record: &RemoteNotificationOutboxRecord,
    ) -> AxonResult<RemoteFeishuDeliveryPlan> {
        let target = default_message_target(&accepted_record.connection_id)?;
        Self::plan_requests_for_target(dispatch, api_base_url, accepted_record, &target)
    }

    /// 以显式消息目标生成 Feishu delivery 请求计划。
    ///
    /// # Errors
    ///
    /// 当 app_id/app_secret_env、dispatch payload、目标或 URL 无效时返回错误。
    pub fn plan_requests_for_target(
        dispatch: &RemoteFeishuWebSocketDispatchPlan,
        api_base_url: Option<&str>,
        accepted_record: &RemoteNotificationOutboxRecord,
        target: &RemoteFeishuMessageTarget,
    ) -> AxonResult<RemoteFeishuDeliveryPlan> {
        if dispatch.app_id.trim().is_empty()
            || dispatch.app_secret_env.trim().is_empty()
            || dispatch.artifact.payload.trim().is_empty()
        {
            return Err(AxonError::runtime_failure(
                "Feishu delivery 规划缺少必要字段",
            ));
        }
        if accepted_record.transport != RemoteTransportKind::Feishu {
            return Err(AxonError::runtime_failure(
                "Feishu delivery 仅接受 Feishu outbox 记录",
            ));
        }
        let base = normalized_base_url(api_base_url.unwrap_or(REMOTE_FEISHU_API_BASE_URL))?;
        let token_request = RemoteFeishuHttpRequestPlan {
            dispatch_id: accepted_record.dispatch_id.clone(),
            url: format!("{base}{REMOTE_FEISHU_TENANT_ACCESS_TOKEN_PATH}"),
            method: "POST".to_owned(),
            headers: crate::remote_feishu_delivery_http::json_headers(None),
            body_json: serde_json::json!({
                "app_id": dispatch.app_id,
                "app_secret": REMOTE_FEISHU_APP_SECRET_PLACEHOLDER,
            })
            .to_string(),
        };
        let outbound = RemoteFeishuOutboundMessageRuntime::plan_from_artifact(&dispatch.artifact)?;
        let message_request =
            if let Some(reply_to_message_id) = target.reply_to_message_id.as_deref() {
                crate::remote_feishu_delivery_http::plan_reply_message_request(
                    &base,
                    accepted_record,
                    reply_to_message_id,
                    &outbound.msg_type,
                    &outbound.content_json,
                )
            } else {
                crate::remote_feishu_delivery_http::plan_create_message_request(
                    &base,
                    accepted_record,
                    target,
                    &outbound.msg_type,
                    &outbound.content_json,
                )
            };
        let fallback_message_request = target.reply_to_message_id.as_ref().map(|_| {
            crate::remote_feishu_delivery_http::plan_create_message_request(
                &base,
                accepted_record,
                target,
                &outbound.msg_type,
                &outbound.content_json,
            )
        });
        Ok(RemoteFeishuDeliveryPlan {
            session_id: SessionId(accepted_record.session_id),
            token_request,
            message_request,
            fallback_message_request,
            network_send_enabled: dispatch.notification_sending_enabled,
            reasons: vec![
                REMOTE_FEISHU_DELIVERY_REASON_REQUEST_PLANNED.to_owned(),
                REMOTE_FEISHU_DELIVERY_REASON_FEEDBACK_PLANNED.to_owned(),
            ],
        })
    }

    /// 基于 Agent 回复 artifact 生成 Feishu 文本回复请求计划。
    ///
    /// # Errors
    ///
    /// 当 artifact 或 Feishu 发送目标无效时返回错误。
    pub fn plan_agent_reply_requests(
        dispatch: &RemoteFeishuWebSocketDispatchPlan,
        api_base_url: Option<&str>,
        accepted_record: &RemoteNotificationOutboxRecord,
        artifact: &RemoteFeishuAgentReplyArtifactRecord,
    ) -> AxonResult<Vec<RemoteFeishuDeliveryPlan>> {
        crate::remote_feishu_agent_reply_delivery::plan_agent_reply_requests(
            dispatch,
            api_base_url,
            accepted_record,
            artifact,
        )
    }

    /// 基于 Feishu 发送结果回写 outbox feedback。
    ///
    /// # Errors
    ///
    /// 当 accepted record 与 outcome 不匹配时返回错误。
    pub fn plan_feedback(
        accepted_record: RemoteNotificationOutboxRecord,
        outcome: RemoteNotificationAdapterSendOutcome,
    ) -> AxonResult<RemoteNotificationAdapterSendReport> {
        let envelope_report = RemoteNotificationAdapterEnvelopeReport {
            session_id: SessionId(accepted_record.session_id),
            transport: RemoteTransportKind::Feishu,
            envelopes: vec![RemoteNotificationAdapterEnvelope {
                dispatch_id: accepted_record.dispatch_id.clone(),
                session_id: SessionId(accepted_record.session_id),
                transport: RemoteTransportKind::Feishu,
                surface: RemoteNotificationAdapterEnvelopeSurface::FeishuCard,
                subscriber_id: accepted_record.subscriber_id.clone(),
                connection_id: accepted_record.connection_id.clone(),
                pairing_id: accepted_record.pairing_id.clone(),
                title: "Axon 飞书远控通知".to_owned(),
                summary: "Feishu delivery".to_owned(),
                delivery_count: accepted_record.delivery_count,
                delivery_kinds: accepted_record.delivery_kinds.clone(),
                attempt_count: accepted_record.attempt_count,
                local_only: true,
                network_send_allowed: false,
                raw_payload_forward_allowed: false,
            }],
            local_only: true,
            network_send_allowed: false,
            raw_payload_forward_allowed: false,
            reasons: vec![],
        };
        RemoteNotificationAdapterSendRuntime::plan_send_result(RemoteNotificationAdapterSendInput {
            envelope_report,
            accepted_records: vec![accepted_record],
            outcomes: vec![outcome],
        })
    }

    /// 真正执行 Feishu token/message API，并生成 outbox feedback。
    ///
    /// # Errors
    ///
    /// 当配置、环境变量或 HTTP 交互无效时返回错误。
    pub fn execute(
        dispatch: &RemoteFeishuWebSocketDispatchPlan,
        api_base_url: Option<&str>,
        accepted_record: &RemoteNotificationOutboxRecord,
        sent_at_ms: u64,
    ) -> AxonResult<RemoteFeishuDeliveryExecutionReport> {
        let app_secret = env::var(&dispatch.app_secret_env)
            .map_err(|_| AxonError::runtime_failure("Feishu app_secret 环境变量未设置"))?;
        Self::execute_with_http_and_secret(
            dispatch,
            &app_secret,
            api_base_url,
            accepted_record,
            sent_at_ms,
            RemoteFeishuOapiExecutorRuntime::execute_json,
        )
    }

    /// 使用显式 app_secret 执行 delivery。
    ///
    /// # Errors
    ///
    /// 当请求计划或 HTTP 响应无效时返回错误。
    pub fn execute_with_secret(
        dispatch: &RemoteFeishuWebSocketDispatchPlan,
        app_secret: &str,
        api_base_url: Option<&str>,
        accepted_record: &RemoteNotificationOutboxRecord,
        sent_at_ms: u64,
    ) -> AxonResult<RemoteFeishuDeliveryExecutionReport> {
        Self::execute_with_http_and_secret(
            dispatch,
            app_secret,
            api_base_url,
            accepted_record,
            sent_at_ms,
            RemoteFeishuOapiExecutorRuntime::execute_json,
        )
    }

    /// 使用显式 secret 执行 Agent 回复 artifact 投递。
    ///
    /// # Errors
    ///
    /// 当 token 或 message API 响应不符合预期时返回错误。
    pub fn execute_agent_reply_with_secret(
        dispatch: &RemoteFeishuWebSocketDispatchPlan,
        app_secret: &str,
        api_base_url: Option<&str>,
        accepted_record: &RemoteNotificationOutboxRecord,
        artifact: &RemoteFeishuAgentReplyArtifactRecord,
        sent_at_ms: u64,
    ) -> AxonResult<RemoteFeishuDeliveryExecutionReport> {
        crate::remote_feishu_agent_reply_delivery::execute_agent_reply_with_secret(
            dispatch,
            app_secret,
            api_base_url,
            accepted_record,
            artifact,
            sent_at_ms,
        )
    }

    /// 使用环境变量 secret 执行 Agent 回复 artifact 投递。
    ///
    /// # Errors
    ///
    /// 当环境变量、token 或 message API 响应无效时返回错误。
    pub fn execute_agent_reply(
        dispatch: &RemoteFeishuWebSocketDispatchPlan,
        api_base_url: Option<&str>,
        accepted_record: &RemoteNotificationOutboxRecord,
        artifact: &RemoteFeishuAgentReplyArtifactRecord,
        sent_at_ms: u64,
    ) -> AxonResult<RemoteFeishuDeliveryExecutionReport> {
        crate::remote_feishu_agent_reply_delivery::execute_agent_reply(
            dispatch,
            api_base_url,
            accepted_record,
            artifact,
            sent_at_ms,
        )
    }

    /// 使用注入 HTTP 与显式 secret 执行 Agent 回复 artifact 投递。
    ///
    /// # Errors
    ///
    /// 当请求计划或 HTTP 响应无效时返回错误。
    pub fn execute_agent_reply_with_http_and_secret<F>(
        dispatch: &RemoteFeishuWebSocketDispatchPlan,
        app_secret: &str,
        api_base_url: Option<&str>,
        accepted_record: &RemoteNotificationOutboxRecord,
        artifact: &RemoteFeishuAgentReplyArtifactRecord,
        sent_at_ms: u64,
        http: F,
    ) -> AxonResult<RemoteFeishuDeliveryExecutionReport>
    where
        F: FnMut(&RemoteFeishuHttpRequestPlan) -> AxonResult<RemoteFeishuHttpResponse>,
    {
        crate::remote_feishu_agent_reply_delivery::execute_agent_reply_with_http_and_secret(
            dispatch,
            app_secret,
            api_base_url,
            accepted_record,
            artifact,
            sent_at_ms,
            http,
        )
    }

    /// 从 Agent 回复 artifact 中提取首个本地媒体文件并投递。
    ///
    /// # Errors
    ///
    /// 当回复中没有可投递本地文件，或 token、上传、发送任一环节失败时返回错误。
    pub fn execute_agent_reply_local_media_with_secret(
        dispatch: &RemoteFeishuWebSocketDispatchPlan,
        app_secret: &str,
        api_base_url: Option<&str>,
        accepted_record: &RemoteNotificationOutboxRecord,
        artifact: &RemoteFeishuAgentReplyArtifactRecord,
        sent_at_ms: u64,
    ) -> AxonResult<RemoteFeishuDeliveryExecutionReport> {
        crate::remote_feishu_agent_reply_local_media::execute_agent_reply_local_media_with_secret(
            dispatch,
            app_secret,
            api_base_url,
            accepted_record,
            artifact,
            sent_at_ms,
        )
    }

    /// 使用环境变量 secret 投递 Agent 回复 artifact 中的本地媒体文件。
    ///
    /// # Errors
    ///
    /// 当环境变量缺失、回复中没有可投递本地文件，或 OpenAPI 交互失败时返回错误。
    pub fn execute_agent_reply_local_media(
        dispatch: &RemoteFeishuWebSocketDispatchPlan,
        api_base_url: Option<&str>,
        accepted_record: &RemoteNotificationOutboxRecord,
        artifact: &RemoteFeishuAgentReplyArtifactRecord,
        sent_at_ms: u64,
    ) -> AxonResult<RemoteFeishuDeliveryExecutionReport> {
        crate::remote_feishu_agent_reply_local_media::execute_agent_reply_local_media(
            dispatch,
            api_base_url,
            accepted_record,
            artifact,
            sent_at_ms,
        )
    }

    /// 从 Agent 回复 artifact 中提取全部本地媒体文件并逐个投递。
    ///
    /// # Errors
    ///
    /// 当回复中没有可投递本地文件，或任一文件 token、上传、发送环节失败时返回错误。
    pub fn execute_agent_reply_local_media_all(
        dispatch: &RemoteFeishuWebSocketDispatchPlan,
        api_base_url: Option<&str>,
        accepted_record: &RemoteNotificationOutboxRecord,
        artifact: &RemoteFeishuAgentReplyArtifactRecord,
        sent_at_ms: u64,
    ) -> AxonResult<RemoteFeishuDeliveryExecutionReport> {
        crate::remote_feishu_agent_reply_local_media::execute_agent_reply_local_media_all(
            dispatch,
            api_base_url,
            accepted_record,
            artifact,
            sent_at_ms,
        )
    }

    /// 使用显式 secret 投递 Agent 回复 artifact 中的全部本地媒体文件。
    ///
    /// # Errors
    ///
    /// 当回复中没有可投递本地文件，或任一文件 OpenAPI 交互失败时返回错误。
    pub fn execute_agent_reply_local_media_all_with_secret(
        dispatch: &RemoteFeishuWebSocketDispatchPlan,
        app_secret: &str,
        api_base_url: Option<&str>,
        accepted_record: &RemoteNotificationOutboxRecord,
        artifact: &RemoteFeishuAgentReplyArtifactRecord,
        sent_at_ms: u64,
    ) -> AxonResult<RemoteFeishuDeliveryExecutionReport> {
        crate::remote_feishu_agent_reply_local_media::execute_agent_reply_local_media_all_with_secret(
            dispatch,
            app_secret,
            api_base_url,
            accepted_record,
            artifact,
            sent_at_ms,
        )
    }

    /// 使用注入 HTTP 与显式 secret 投递 Agent 回复 artifact 中的本地媒体文件。
    ///
    /// # Errors
    ///
    /// 当回复中没有可投递本地文件，或 token、上传、发送任一环节失败时返回错误。
    pub fn execute_agent_reply_local_media_with_http_and_secret<F, G>(
        input: crate::remote_feishu_agent_reply_local_media::RemoteFeishuAgentReplyLocalMediaHttpExecutionInput<'_>,
        json_http: F,
        multipart_http: G,
    ) -> AxonResult<RemoteFeishuDeliveryExecutionReport>
    where
        F: FnMut(&RemoteFeishuHttpRequestPlan) -> AxonResult<RemoteFeishuHttpResponse>,
        G: FnMut(&RemoteFeishuMultipartHttpRequest) -> AxonResult<RemoteFeishuHttpResponse>,
    {
        crate::remote_feishu_agent_reply_local_media::execute_agent_reply_local_media_with_http_and_secret(
            input,
            json_http,
            multipart_http,
        )
    }

    /// 使用注入 HTTP 与显式 secret 投递 Agent 回复 artifact 中的全部本地媒体文件。
    ///
    /// # Errors
    ///
    /// 当回复中没有可投递本地文件，或任一文件 token、上传、发送环节失败时返回错误。
    pub fn execute_agent_reply_local_media_all_with_http_and_secret<F, G>(
        input: crate::remote_feishu_agent_reply_local_media::RemoteFeishuAgentReplyLocalMediaHttpExecutionInput<'_>,
        json_http: F,
        multipart_http: G,
    ) -> AxonResult<RemoteFeishuDeliveryExecutionReport>
    where
        F: FnMut(&RemoteFeishuHttpRequestPlan) -> AxonResult<RemoteFeishuHttpResponse>,
        G: FnMut(&RemoteFeishuMultipartHttpRequest) -> AxonResult<RemoteFeishuHttpResponse>,
    {
        crate::remote_feishu_agent_reply_local_media::execute_agent_reply_local_media_all_with_http_and_secret(
            input,
            json_http,
            multipart_http,
        )
    }

    /// 使用注入 HTTP 与显式 secret 投递 Agent 回复的可见正文与本地附件。
    ///
    /// # Errors
    ///
    /// 当正文与附件均为空，或 token、上传、发送任一环节失败时返回错误。
    pub fn execute_agent_reply_delivery_with_http_and_secret<F, G>(
        input: crate::remote_feishu_agent_reply_local_media::RemoteFeishuAgentReplyLocalMediaHttpExecutionInput<'_>,
        json_http: F,
        multipart_http: G,
    ) -> AxonResult<RemoteFeishuDeliveryExecutionReport>
    where
        F: FnMut(&RemoteFeishuHttpRequestPlan) -> AxonResult<RemoteFeishuHttpResponse>,
        G: FnMut(&RemoteFeishuMultipartHttpRequest) -> AxonResult<RemoteFeishuHttpResponse>,
    {
        crate::remote_feishu_agent_reply_local_media::execute_agent_reply_delivery_with_http_and_secret(
            input,
            json_http,
            multipart_http,
        )
    }

    /// 使用显式 secret 投递 Agent 回复的可见正文与本地附件。
    ///
    /// # Errors
    ///
    /// 当正文与附件均为空，或 token、上传、发送任一环节失败时返回错误。
    pub fn execute_agent_reply_delivery_with_secret(
        dispatch: &RemoteFeishuWebSocketDispatchPlan,
        app_secret: &str,
        api_base_url: Option<&str>,
        accepted_record: &RemoteNotificationOutboxRecord,
        artifact: &RemoteFeishuAgentReplyArtifactRecord,
        sent_at_ms: u64,
    ) -> AxonResult<RemoteFeishuDeliveryExecutionReport> {
        crate::remote_feishu_agent_reply_local_media::execute_agent_reply_delivery_with_secret(
            dispatch,
            app_secret,
            api_base_url,
            accepted_record,
            artifact,
            sent_at_ms,
        )
    }

    /// 使用环境变量 secret 投递 Agent 回复的可见正文与本地附件。
    ///
    /// # Errors
    ///
    /// 当环境变量缺失、正文与附件均为空，或 OpenAPI 交互失败时返回错误。
    pub fn execute_agent_reply_delivery(
        dispatch: &RemoteFeishuWebSocketDispatchPlan,
        api_base_url: Option<&str>,
        accepted_record: &RemoteNotificationOutboxRecord,
        artifact: &RemoteFeishuAgentReplyArtifactRecord,
        sent_at_ms: u64,
    ) -> AxonResult<RemoteFeishuDeliveryExecutionReport> {
        crate::remote_feishu_agent_reply_local_media::execute_agent_reply_delivery(
            dispatch,
            api_base_url,
            accepted_record,
            artifact,
            sent_at_ms,
        )
    }

    /// 上传本地媒体并发送 Feishu 媒体消息。
    ///
    /// # Errors
    ///
    /// 当 token、媒体上传或消息发送任一环节失败时返回错误。
    pub fn execute_media_with_http_and_secret<F, G>(
        input: RemoteFeishuDeliveryMediaHttpExecutionInput<'_>,
        mut json_http: F,
        multipart_http: G,
    ) -> AxonResult<RemoteFeishuDeliveryExecutionReport>
    where
        F: FnMut(&RemoteFeishuHttpRequestPlan) -> AxonResult<RemoteFeishuHttpResponse>,
        G: FnMut(&RemoteFeishuMultipartHttpRequest) -> AxonResult<RemoteFeishuHttpResponse>,
    {
        let plan = Self::plan_requests(input.dispatch, input.api_base_url, input.accepted_record)?;
        let tenant_access_token = Self::fetch_tenant_access_token_with_http_and_secret(
            &input.dispatch.app_id,
            input.app_secret,
            &plan.token_request,
            &mut json_http,
        )?;
        let media_message = RemoteFeishuMediaFlowRuntime::upload_outbound_media_with_http(
            RemoteFeishuOutboundMediaMessageInput {
                kind: input.media.kind,
                file_path: input.media.file_path,
                tenant_access_token: tenant_access_token.clone(),
                api_base_url: input.api_base_url.map(str::to_owned),
            },
            multipart_http,
        )?;
        let base = normalized_base_url(input.api_base_url.unwrap_or(REMOTE_FEISHU_API_BASE_URL))?;
        let target = input.media.target.unwrap_or(default_message_target(
            &input.accepted_record.connection_id,
        )?);
        let message_request = crate::remote_feishu_delivery_http::media_message_request(
            &base,
            input.accepted_record,
            &target,
            &media_message.message,
            &tenant_access_token,
        );
        let fallback_message_request = target.reply_to_message_id.as_ref().map(|_| {
            let mut request = crate::remote_feishu_delivery_http::plan_create_message_request(
                &base,
                input.accepted_record,
                &target,
                &media_message.message.msg_type,
                &media_message.message.content_json,
            );
            request.headers =
                crate::remote_feishu_delivery_http::json_headers(Some(&tenant_access_token));
            request
        });
        let message_response =
            crate::remote_feishu_delivery_http::execute_message_request_with_fallback(
                &message_request,
                fallback_message_request.as_ref(),
                &mut json_http,
            )?;
        crate::remote_feishu_delivery_http::execution_report_from_message_response(
            input.accepted_record,
            input.sent_at_ms,
            &message_response,
        )
    }

    /// 使用注入的 HTTP 执行器执行 delivery。
    ///
    /// # Errors
    ///
    /// 当 token 或 message API 响应不符合预期时返回错误。
    pub fn execute_with_http_and_secret<F>(
        dispatch: &RemoteFeishuWebSocketDispatchPlan,
        app_secret: &str,
        api_base_url: Option<&str>,
        accepted_record: &RemoteNotificationOutboxRecord,
        sent_at_ms: u64,
        mut http: F,
    ) -> AxonResult<RemoteFeishuDeliveryExecutionReport>
    where
        F: FnMut(&RemoteFeishuHttpRequestPlan) -> AxonResult<RemoteFeishuHttpResponse>,
    {
        let plan = Self::plan_requests(dispatch, api_base_url, accepted_record)?;
        let tenant_access_token = Self::fetch_tenant_access_token_with_http_and_secret(
            &dispatch.app_id,
            app_secret,
            &plan.token_request,
            &mut http,
        )?;
        let message_request = RemoteFeishuHttpRequestPlan {
            headers: crate::remote_feishu_delivery_http::json_headers(Some(&tenant_access_token)),
            ..plan.message_request.clone()
        };
        let fallback_message_request =
            plan.fallback_message_request
                .as_ref()
                .map(|request| RemoteFeishuHttpRequestPlan {
                    headers: crate::remote_feishu_delivery_http::json_headers(Some(
                        &tenant_access_token,
                    )),
                    ..request.clone()
                });
        let message_response =
            crate::remote_feishu_delivery_http::execute_message_request_with_fallback(
                &message_request,
                fallback_message_request.as_ref(),
                &mut http,
            )?;
        crate::remote_feishu_delivery_http::execution_report_from_message_response(
            accepted_record,
            sent_at_ms,
            &message_response,
        )
    }

    /// 使用注入 HTTP 与显式 secret 获取 tenant_access_token。
    ///
    /// # Errors
    ///
    /// 当 token API 交互失败时返回错误。
    pub fn fetch_tenant_access_token_with_http_and_secret<F>(
        app_id: &str,
        app_secret: &str,
        token_request_template: &RemoteFeishuHttpRequestPlan,
        http: &mut F,
    ) -> AxonResult<String>
    where
        F: FnMut(&RemoteFeishuHttpRequestPlan) -> AxonResult<RemoteFeishuHttpResponse>,
    {
        let token_request = RemoteFeishuHttpRequestPlan {
            body_json: serde_json::json!({
                "app_id": app_id,
                "app_secret": app_secret,
            })
            .to_string(),
            ..token_request_template.clone()
        };
        let token_response =
            crate::remote_feishu_delivery_http::execute_with_retry(&token_request, http)?;
        crate::remote_feishu_delivery_http::parse_tenant_access_token(&token_response)
    }
}

fn normalized_base_url(value: &str) -> AxonResult<String> {
    let trimmed = value.trim().trim_end_matches('/');
    if trimmed.starts_with("https://") || trimmed.starts_with("http://") {
        return Ok(trimmed.to_owned());
    }
    Err(AxonError::runtime_failure(
        "Feishu API base URL 必须为 http/https",
    ))
}

fn default_message_target(connection_id: &str) -> AxonResult<RemoteFeishuMessageTarget> {
    if connection_id.trim().is_empty() {
        return Err(AxonError::runtime_failure(
            "Feishu 消息目标 connection_id 不能为空",
        ));
    }
    let receive_id_type = if connection_id.starts_with("ou_") {
        "open_id"
    } else {
        "chat_id"
    };
    Ok(RemoteFeishuMessageTarget {
        receive_id: connection_id.to_owned(),
        receive_id_type: receive_id_type.to_owned(),
        reply_to_message_id: None,
    })
}
