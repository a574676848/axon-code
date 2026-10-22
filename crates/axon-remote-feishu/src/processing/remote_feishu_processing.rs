//! Feishu processing 生命周期投影。
//!
//! 先不直接依赖真实 SDK，而是把 processing reaction 的开始、完成、失败
//! 规划成可测试的本地动作计划，后续可由 worker / adapter 执行。

mod remote_feishu_processing_store;

use std::env;

use axon_crypto::sha256_hex;
use axon_domain::{AxonError, AxonResult};

use crate::{
    REMOTE_FEISHU_API_BASE_URL, RemoteFeishuDeliveryRuntime, RemoteFeishuHttpRequestPlan,
    RemoteFeishuOapiExecutorRuntime,
};

pub use remote_feishu_processing_store::{
    RemoteFeishuProcessingStateRecord, RemoteFeishuProcessingStateStoreRuntime,
};

/// 处理中 reaction 类型。
pub const REMOTE_FEISHU_PROCESSING_REACTION_IN_PROGRESS: &str = "Typing";
/// 失败 reaction 类型。
pub const REMOTE_FEISHU_PROCESSING_REACTION_FAILURE: &str = "CrossMark";
/// Feishu processing state 持久化文件名。
pub const REMOTE_FEISHU_PROCESSING_STATE_FILE: &str = "remote_feishu_processing_state.sqlite3";
const REMOTE_FEISHU_PROCESSING_DISPATCH_ID: &str = "feishu-processing";
/// Feishu reaction 路径后缀。
pub const REMOTE_FEISHU_PROCESSING_REACTION_PATH_SUFFIX: &str = "/reactions";

/// processing 动作类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteFeishuProcessingActionKind {
    /// 添加 reaction。
    AddReaction,
    /// 删除 reaction。
    RemoveReaction,
}

/// processing 完成结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RemoteFeishuProcessingOutcome {
    /// 处理成功。
    Success,
    /// 处理失败。
    Failure,
}

/// 单个 processing 动作计划。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuProcessingAction {
    /// 动作类型。
    pub kind: RemoteFeishuProcessingActionKind,
    /// 目标消息 ID。
    pub message_id: String,
    /// reaction 类型。
    pub reaction_type: String,
}

/// processing 生命周期报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuProcessingPlan {
    /// 目标消息 ID。
    pub message_id: String,
    /// 动作列表。
    pub actions: Vec<RemoteFeishuProcessingAction>,
}

/// 单次 processing HTTP 请求计划。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuProcessingRequestPlan {
    /// HTTP 方法。
    pub method: String,
    /// 完整 URL。
    pub url: String,
    /// 请求头。
    pub headers: Vec<(String, String)>,
    /// JSON 请求体。
    pub body_json: Option<String>,
}

/// 注入 HTTP 执行 processing 完成动作的输入。
#[derive(Debug, Clone, Copy)]
pub struct RemoteFeishuProcessingCompleteHttpInput<'a> {
    /// Feishu app id。
    pub app_id: &'a str,
    /// 显式 app secret。
    pub app_secret: &'a str,
    /// API base URL。
    pub api_base_url: Option<&'a str>,
    /// 目标消息 ID。
    pub message_id: &'a str,
    /// 完成结果。
    pub outcome: RemoteFeishuProcessingOutcome,
    /// 当前活跃 reaction state。
    pub active_state: Option<&'a RemoteFeishuProcessingStateRecord>,
    /// 记录时间。
    pub recorded_at_ms: u64,
}

/// processing runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct RemoteFeishuProcessingRuntime;

impl RemoteFeishuProcessingRuntime {
    /// 规划 processing 开始动作。
    pub fn plan_start(message_id: &str) -> AxonResult<RemoteFeishuProcessingPlan> {
        validate_message_id(message_id)?;
        Ok(RemoteFeishuProcessingPlan {
            message_id: message_id.to_owned(),
            actions: vec![RemoteFeishuProcessingAction {
                kind: RemoteFeishuProcessingActionKind::AddReaction,
                message_id: message_id.to_owned(),
                reaction_type: REMOTE_FEISHU_PROCESSING_REACTION_IN_PROGRESS.to_owned(),
            }],
        })
    }

    /// 规划 processing 结束动作。
    pub fn plan_complete(
        message_id: &str,
        outcome: RemoteFeishuProcessingOutcome,
    ) -> AxonResult<RemoteFeishuProcessingPlan> {
        validate_message_id(message_id)?;
        let mut actions = vec![RemoteFeishuProcessingAction {
            kind: RemoteFeishuProcessingActionKind::RemoveReaction,
            message_id: message_id.to_owned(),
            reaction_type: REMOTE_FEISHU_PROCESSING_REACTION_IN_PROGRESS.to_owned(),
        }];
        if outcome == RemoteFeishuProcessingOutcome::Failure {
            actions.push(RemoteFeishuProcessingAction {
                kind: RemoteFeishuProcessingActionKind::AddReaction,
                message_id: message_id.to_owned(),
                reaction_type: REMOTE_FEISHU_PROCESSING_REACTION_FAILURE.to_owned(),
            });
        }
        Ok(RemoteFeishuProcessingPlan {
            message_id: message_id.to_owned(),
            actions,
        })
    }

    /// 规划 add reaction 请求。
    pub fn plan_add_request(
        api_base_url: &str,
        tenant_access_token: &str,
        message_id: &str,
        reaction_type: &str,
    ) -> AxonResult<RemoteFeishuProcessingRequestPlan> {
        validate_message_id(message_id)?;
        validate_base_url(api_base_url)?;
        Ok(RemoteFeishuProcessingRequestPlan {
            method: "POST".to_owned(),
            url: format!(
                "{}/open-apis/im/v1/messages/{}{}",
                api_base_url.trim().trim_end_matches('/'),
                message_id,
                REMOTE_FEISHU_PROCESSING_REACTION_PATH_SUFFIX
            ),
            headers: json_headers(tenant_access_token),
            body_json: Some(
                serde_json::json!({
                    "reaction_type": {
                        "emoji_type": reaction_type,
                    }
                })
                .to_string(),
            ),
        })
    }

    /// 规划 remove reaction 请求。
    pub fn plan_remove_request(
        api_base_url: &str,
        tenant_access_token: &str,
        message_id: &str,
        reaction_id: &str,
    ) -> AxonResult<RemoteFeishuProcessingRequestPlan> {
        validate_message_id(message_id)?;
        if reaction_id.trim().is_empty() {
            return Err(AxonError::runtime_failure(
                "Feishu processing reaction_id 不能为空",
            ));
        }
        validate_base_url(api_base_url)?;
        Ok(RemoteFeishuProcessingRequestPlan {
            method: "DELETE".to_owned(),
            url: format!(
                "{}/open-apis/im/v1/messages/{}/reactions/{}",
                api_base_url.trim().trim_end_matches('/'),
                message_id,
                reaction_id
            ),
            headers: json_headers(tenant_access_token),
            body_json: None,
        })
    }

    /// 执行 processing 开始动作。
    pub fn execute_start_with_http<F>(
        api_base_url: &str,
        tenant_access_token: &str,
        message_id: &str,
        recorded_at_ms: u64,
        mut http: F,
    ) -> AxonResult<RemoteFeishuProcessingStateRecord>
    where
        F: FnMut(&RemoteFeishuProcessingRequestPlan) -> AxonResult<crate::RemoteFeishuHttpResponse>,
    {
        let request = Self::plan_add_request(
            api_base_url,
            tenant_access_token,
            message_id,
            REMOTE_FEISHU_PROCESSING_REACTION_IN_PROGRESS,
        )?;
        let response = http(&request)?;
        let reaction_id = parse_reaction_id(&response)?;
        Ok(RemoteFeishuProcessingStateRecord {
            schema_version: 1,
            message_id_sha256: sha256_hex(message_id),
            reaction_id,
            reaction_type: REMOTE_FEISHU_PROCESSING_REACTION_IN_PROGRESS.to_owned(),
            active: true,
            recorded_at_ms,
        })
    }

    /// 使用环境变量中的 app_secret 执行 processing 开始动作。
    pub fn execute_start(
        app_id: &str,
        app_secret_env: &str,
        api_base_url: Option<&str>,
        message_id: &str,
        recorded_at_ms: u64,
    ) -> AxonResult<RemoteFeishuProcessingStateRecord> {
        let app_secret = env::var(app_secret_env).map_err(|_| {
            AxonError::runtime_failure("Feishu processing app_secret 环境变量未设置")
        })?;
        Self::execute_start_with_secret(
            app_id,
            &app_secret,
            api_base_url,
            message_id,
            recorded_at_ms,
        )
    }

    /// 使用显式 app_secret 执行 processing 开始动作。
    pub fn execute_start_with_secret(
        app_id: &str,
        app_secret: &str,
        api_base_url: Option<&str>,
        message_id: &str,
        recorded_at_ms: u64,
    ) -> AxonResult<RemoteFeishuProcessingStateRecord> {
        Self::execute_start_with_http_and_secret(
            app_id,
            app_secret,
            api_base_url,
            message_id,
            recorded_at_ms,
            execute_http_request,
        )
    }

    /// 使用注入 HTTP 与显式 app_secret 执行 processing 开始动作。
    pub fn execute_start_with_http_and_secret<F>(
        app_id: &str,
        app_secret: &str,
        api_base_url: Option<&str>,
        message_id: &str,
        recorded_at_ms: u64,
        mut http: F,
    ) -> AxonResult<RemoteFeishuProcessingStateRecord>
    where
        F: FnMut(&RemoteFeishuProcessingRequestPlan) -> AxonResult<crate::RemoteFeishuHttpResponse>,
    {
        let token_request = token_request_plan(app_id, api_base_url)?;
        let tenant_access_token =
            RemoteFeishuDeliveryRuntime::fetch_tenant_access_token_with_http_and_secret(
                app_id,
                app_secret,
                &token_request,
                &mut |request| {
                    http(&RemoteFeishuProcessingRequestPlan {
                        method: request.method.clone(),
                        url: request.url.clone(),
                        headers: request.headers.clone(),
                        body_json: Some(request.body_json.clone()),
                    })
                },
            )?;
        Self::execute_start_with_http(
            api_base_url.unwrap_or(REMOTE_FEISHU_API_BASE_URL),
            &tenant_access_token,
            message_id,
            recorded_at_ms,
            http,
        )
    }

    /// 执行 processing 结束动作，先移除 Typing，失败时追加 CrossMark。
    pub fn execute_complete_with_http<F>(
        api_base_url: &str,
        tenant_access_token: &str,
        message_id: &str,
        outcome: RemoteFeishuProcessingOutcome,
        active_state: Option<&RemoteFeishuProcessingStateRecord>,
        recorded_at_ms: u64,
        mut http: F,
    ) -> AxonResult<Vec<RemoteFeishuProcessingStateRecord>>
    where
        F: FnMut(&RemoteFeishuProcessingRequestPlan) -> AxonResult<crate::RemoteFeishuHttpResponse>,
    {
        let mut records = Vec::new();
        if let Some(state) = active_state {
            let remove = Self::plan_remove_request(
                api_base_url,
                tenant_access_token,
                message_id,
                &state.reaction_id,
            )?;
            let _ = http(&remove)?;
            records.push(RemoteFeishuProcessingStateRecord {
                active: false,
                recorded_at_ms,
                ..state.clone()
            });
        }
        if outcome == RemoteFeishuProcessingOutcome::Failure {
            let failure = Self::plan_add_request(
                api_base_url,
                tenant_access_token,
                message_id,
                REMOTE_FEISHU_PROCESSING_REACTION_FAILURE,
            )?;
            let response = http(&failure)?;
            let reaction_id = parse_reaction_id(&response)?;
            records.push(RemoteFeishuProcessingStateRecord {
                schema_version: 1,
                message_id_sha256: sha256_hex(message_id),
                reaction_id,
                reaction_type: REMOTE_FEISHU_PROCESSING_REACTION_FAILURE.to_owned(),
                active: true,
                recorded_at_ms,
            });
        }
        Ok(records)
    }

    /// 使用环境变量中的 app_secret 执行 processing 完成动作。
    pub fn execute_complete(
        app_id: &str,
        app_secret_env: &str,
        api_base_url: Option<&str>,
        message_id: &str,
        outcome: RemoteFeishuProcessingOutcome,
        active_state: Option<&RemoteFeishuProcessingStateRecord>,
        recorded_at_ms: u64,
    ) -> AxonResult<Vec<RemoteFeishuProcessingStateRecord>> {
        let app_secret = env::var(app_secret_env).map_err(|_| {
            AxonError::runtime_failure("Feishu processing app_secret 环境变量未设置")
        })?;
        Self::execute_complete_with_secret(
            app_id,
            &app_secret,
            api_base_url,
            message_id,
            outcome,
            active_state,
            recorded_at_ms,
        )
    }

    /// 使用显式 app_secret 执行 processing 完成动作。
    pub fn execute_complete_with_secret(
        app_id: &str,
        app_secret: &str,
        api_base_url: Option<&str>,
        message_id: &str,
        outcome: RemoteFeishuProcessingOutcome,
        active_state: Option<&RemoteFeishuProcessingStateRecord>,
        recorded_at_ms: u64,
    ) -> AxonResult<Vec<RemoteFeishuProcessingStateRecord>> {
        Self::execute_complete_with_http_and_secret(
            &RemoteFeishuProcessingCompleteHttpInput {
                app_id,
                app_secret,
                api_base_url,
                message_id,
                outcome,
                active_state,
                recorded_at_ms,
            },
            execute_http_request,
        )
    }

    /// 使用注入 HTTP 与显式 app_secret 执行 processing 完成动作。
    pub fn execute_complete_with_http_and_secret<F>(
        input: &RemoteFeishuProcessingCompleteHttpInput<'_>,
        mut http: F,
    ) -> AxonResult<Vec<RemoteFeishuProcessingStateRecord>>
    where
        F: FnMut(&RemoteFeishuProcessingRequestPlan) -> AxonResult<crate::RemoteFeishuHttpResponse>,
    {
        let token_request = token_request_plan(input.app_id, input.api_base_url)?;
        let tenant_access_token =
            RemoteFeishuDeliveryRuntime::fetch_tenant_access_token_with_http_and_secret(
                input.app_id,
                input.app_secret,
                &token_request,
                &mut |request| {
                    http(&RemoteFeishuProcessingRequestPlan {
                        method: request.method.clone(),
                        url: request.url.clone(),
                        headers: request.headers.clone(),
                        body_json: Some(request.body_json.clone()),
                    })
                },
            )?;
        Self::execute_complete_with_http(
            input.api_base_url.unwrap_or(REMOTE_FEISHU_API_BASE_URL),
            &tenant_access_token,
            input.message_id,
            input.outcome,
            input.active_state,
            input.recorded_at_ms,
            http,
        )
    }
}

fn validate_message_id(message_id: &str) -> AxonResult<()> {
    if message_id.trim().is_empty() || message_id.chars().count() > 128 {
        return Err(AxonError::runtime_failure(
            "Feishu processing message_id 无效",
        ));
    }
    Ok(())
}

fn validate_base_url(api_base_url: &str) -> AxonResult<()> {
    let trimmed = api_base_url.trim();
    if trimmed.is_empty() || !(trimmed.starts_with("https://") || trimmed.starts_with("http://")) {
        return Err(AxonError::runtime_failure(
            "Feishu processing API base URL 无效",
        ));
    }
    Ok(())
}

fn json_headers(tenant_access_token: &str) -> Vec<(String, String)> {
    vec![
        (
            "Authorization".to_owned(),
            format!("Bearer {tenant_access_token}"),
        ),
        ("Content-Type".to_owned(), "application/json".to_owned()),
    ]
}

fn token_request_plan(
    app_id: &str,
    api_base_url: Option<&str>,
) -> AxonResult<crate::RemoteFeishuHttpRequestPlan> {
    let base = api_base_url.unwrap_or(REMOTE_FEISHU_API_BASE_URL);
    let plan = crate::RemoteFeishuWebSocketDispatchPlan {
        adapter: crate::RemoteNotificationAdapterKind::FeishuWebSocket,
        app_id: app_id.to_owned(),
        app_secret_env: "FEISHU_APP_SECRET".to_owned(),
        websocket_url: "wss://open.feishu.cn/open-apis/ws/v2".to_owned(),
        artifact: crate::RemoteNotificationRenderArtifact {
            adapter: crate::RemoteNotificationAdapterKind::FeishuWebSocket,
            transport: crate::RemoteTransportKind::Feishu,
            content_type: "application/json".to_owned(),
            payload: "{}".to_owned(),
            network_send_allowed: false,
        },
        notification_sending_enabled: false,
        reasons: Vec::new(),
    };
    let accepted =
        crate::RemoteNotificationOutboxRecord::from_input(crate::RemoteNotificationOutboxInput {
            dispatch_id: "processing-token".to_owned(),
            plan: crate::RemoteNotificationFanoutPlan {
                session_id: axon_domain::SessionId(1),
                subscriber_id: "processing".to_owned(),
                connection_id: "processing".to_owned(),
                pairing_id: "processing".to_owned(),
                transport: crate::RemoteTransportKind::Feishu,
                subscribed_kinds: vec![crate::RemoteNotificationKind::TaskStarted],
                delivered: vec![crate::RemoteNotificationDelivery {
                    kind: crate::RemoteNotificationKind::TaskStarted,
                    wire_event: crate::remote_notification_wire_lifecycle_event(
                        axon_domain::SessionId(1),
                        "remote-feishu-processing-token",
                        crate::WireLifecycleStatus::Waiting,
                        "远控任务开始",
                        None,
                        1,
                    ),
                    bounded_body: "processing".to_owned(),
                    reference_ids: vec!["processing".to_owned()],
                    local_action_hint: "processing".to_owned(),
                    severity: crate::RemoteNotificationSeverity::Info,
                }],
                dropped_count: 0,
                local_only: true,
                network_send_allowed: false,
                raw_payload_forward_allowed: false,
            },
            status: crate::RemoteNotificationDispatchStatus::Pending,
            recorded_at_ms: 1,
            updated_at_ms: 1,
            attempt_count: 0,
            next_retry_after_ms: None,
            last_error: None,
            adapter_message_id: None,
            source_trace: "remote_feishu_processing:token".to_owned(),
            reason: "remote_feishu_processing:token".to_owned(),
        })?;
    Ok(RemoteFeishuDeliveryRuntime::plan_requests(&plan, Some(base), &accepted)?.token_request)
}

fn execute_http_request(
    request: &RemoteFeishuProcessingRequestPlan,
) -> AxonResult<crate::RemoteFeishuHttpResponse> {
    RemoteFeishuOapiExecutorRuntime::execute_json(&RemoteFeishuHttpRequestPlan {
        dispatch_id: REMOTE_FEISHU_PROCESSING_DISPATCH_ID.to_owned(),
        method: request.method.clone(),
        url: request.url.clone(),
        headers: request.headers.clone(),
        body_json: request.body_json.clone().unwrap_or_default(),
    })
}

fn parse_reaction_id(response: &crate::RemoteFeishuHttpResponse) -> AxonResult<String> {
    if !(200..300).contains(&response.status) {
        return Err(AxonError::runtime_failure(
            "Feishu processing HTTP 状态失败",
        ));
    }
    let value = serde_json::from_str::<serde_json::Value>(&response.body)
        .map_err(|_| AxonError::runtime_failure("Feishu processing HTTP 响应无效"))?;
    if value.get("code").and_then(serde_json::Value::as_i64) != Some(0) {
        return Err(AxonError::runtime_failure(
            "Feishu processing HTTP 返回错误",
        ));
    }
    value
        .get("data")
        .and_then(|data| data.get("reaction_id"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .filter(|reaction_id| !reaction_id.trim().is_empty())
        .ok_or(AxonError::runtime_failure(
            "Feishu processing reaction_id 缺失",
        ))
}

#[cfg(test)]
#[path = "remote_feishu_processing_tests.rs"]
mod tests;
