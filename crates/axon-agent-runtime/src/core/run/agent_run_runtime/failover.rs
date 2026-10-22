//! Headless agent run 渠道 failover 循环。

use axon_config::{ModelConfig, ProviderConfig};
use axon_domain::{ChannelFailureKind, classify_channel_failure};

use crate::ChannelKey;

use super::{
    AGENT_RUN_ERROR_TYPE_CONFIGURATION, AGENT_RUN_ERROR_TYPE_PROVIDER_TRANSPORT,
    AgentRunChannelAttempt, AgentRunExecutionError, AgentRunFailoverPlan, AgentRunResponse,
};

pub(super) const OUTCOME_SUCCESS: &str = "success";
pub(super) const OUTCOME_TRANSIENT: &str = "transient_failure";
pub(super) const OUTCOME_PERMANENT: &str = "permanent_failure";
pub(crate) const OUTCOME_VISIBLE_OUTPUT: &str = "visible_output_failure";
const PROVIDER_REQUEST_BODY_INVALID_MARKER: &str = "REQUEST_BODY_INVALID";
const INVALID_TOOL_USE_FORMAT_MARKER: &str = "Invalid tool use format.";

/// 跨渠道故障转移的实时事件类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentRunChannelEventKind {
    /// 开始尝试一个渠道。
    AttemptStarted,
    /// 当前渠道请求失败。
    AttemptFailed,
    /// 正在切换到下一渠道。
    Failover,
}

/// 渠道使用的底层传输。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentRunChannelTransport {
    /// HTTP streaming。
    Http,
    /// WebSocket streaming。
    WebSocket,
}

impl AgentRunChannelTransport {
    fn from_provider(provider: &ProviderConfig) -> Self {
        if provider.wire_transport == axon_config::PROVIDER_WIRE_TRANSPORT_WEBSOCKET {
            Self::WebSocket
        } else {
            Self::Http
        }
    }
}

/// 跨渠道故障转移的实时事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRunChannelEvent {
    /// 事件类型。
    pub kind: AgentRunChannelEventKind,
    /// 当前 provider 名称。
    pub provider: String,
    /// 当前模型 ID。
    pub model: String,
    /// 当前渠道使用的底层传输。
    pub transport: AgentRunChannelTransport,
    /// 失败响应的 HTTP 状态码。
    pub http_status: Option<u16>,
    /// 已脱敏的失败摘要。
    pub error_message: Option<String>,
    /// 故障转移目标 provider。
    pub next_provider: Option<String>,
    /// 故障转移目标模型 ID。
    pub next_model: Option<String>,
}

impl AgentRunChannelEvent {
    fn attempt_started(model: &ModelConfig, provider: &ProviderConfig) -> Self {
        Self {
            kind: AgentRunChannelEventKind::AttemptStarted,
            provider: provider.name.clone(),
            model: model.id.clone(),
            transport: AgentRunChannelTransport::from_provider(provider),
            http_status: None,
            error_message: None,
            next_provider: None,
            next_model: None,
        }
    }

    fn attempt_failed(
        model: &ModelConfig,
        provider: &ProviderConfig,
        error: &AgentRunExecutionError<impl Sized>,
    ) -> Self {
        Self {
            kind: AgentRunChannelEventKind::AttemptFailed,
            provider: provider.name.clone(),
            model: model.id.clone(),
            transport: AgentRunChannelTransport::from_provider(provider),
            http_status: error.http_status,
            error_message: Some(error.message.clone()),
            next_provider: None,
            next_model: None,
        }
    }

    fn failover(
        model: &ModelConfig,
        provider: &ProviderConfig,
        next_model: &ModelConfig,
        next_provider: &ProviderConfig,
        http_status: Option<u16>,
        error_message: &str,
    ) -> Self {
        Self {
            kind: AgentRunChannelEventKind::Failover,
            provider: provider.name.clone(),
            model: model.id.clone(),
            transport: AgentRunChannelTransport::from_provider(provider),
            http_status,
            error_message: Some(error_message.to_owned()),
            next_provider: Some(next_provider.name.clone()),
            next_model: Some(next_model.id.clone()),
        }
    }
}

/// Runtime-owned channel failover loop for headless agent runs.
pub fn run_agent_channel_failover<ToolReceipt, F>(
    plan: AgentRunFailoverPlan<'_>,
    mut health_success: impl FnMut(&ChannelKey),
    mut health_failure: impl FnMut(&ChannelKey, u64),
    mut record_affinity: impl FnMut(&str, &str, &str, u64, u64),
    mut observe: impl FnMut(AgentRunChannelEvent),
    mut execute: F,
) -> Result<AgentRunResponse<ToolReceipt>, Box<AgentRunExecutionError<ToolReceipt>>>
where
    F: FnMut(
        &ModelConfig,
        &ProviderConfig,
    )
        -> Result<AgentRunResponse<ToolReceipt>, Box<AgentRunExecutionError<ToolReceipt>>>,
{
    let last_index = plan.chain.len().saturating_sub(1);
    let mut attempts: Vec<AgentRunChannelAttempt> = Vec::new();

    for (index, (model, provider)) in plan.chain.iter().enumerate() {
        let key = ChannelKey {
            model_name: plan.routing_key.to_owned(),
            provider: provider.name.clone(),
        };
        observe(AgentRunChannelEvent::attempt_started(model, provider));

        match execute(model, provider) {
            Ok(mut response) => {
                health_success(&key);
                record_affinity(
                    plan.routing_key,
                    plan.session_id,
                    &provider.name,
                    plan.now_ms,
                    plan.balancer.affinity_ttl_ms,
                );
                attempts.push(AgentRunChannelAttempt {
                    provider: provider.name.clone(),
                    model: model.id.clone(),
                    http_status: Some(response.http_status),
                    outcome: OUTCOME_SUCCESS,
                });
                response.channel_attempts = attempts;
                return Ok(response);
            }
            Err(error) => {
                observe(AgentRunChannelEvent::attempt_failed(
                    model,
                    provider,
                    error.as_ref(),
                ));
                let failure_kind = classify_agent_run_failover_error(&error);
                let visible_output_failover = can_failover_after_visible_output(error.as_ref());
                if error.visible_output_emitted && !visible_output_failover {
                    if failure_kind == ChannelFailureKind::Transient {
                        health_failure(&key, plan.now_ms);
                    }
                    attempts.push(AgentRunChannelAttempt {
                        provider: provider.name.clone(),
                        model: model.id.clone(),
                        http_status: error.http_status,
                        outcome: OUTCOME_VISIBLE_OUTPUT,
                    });
                    return Err(finalize_agent_run_error(*error, attempts));
                }
                let failure_kind = if visible_output_failover {
                    ChannelFailureKind::Transient
                } else {
                    failure_kind
                };
                match failure_kind {
                    ChannelFailureKind::Permanent => {
                        // 传输类永久失败同样供数熔断器：400/401/404 类渠道（参数不兼容、
                        // 鉴权失效）若不计入连续失败，将永远保持“健康”并被反复选中。
                        // 非传输类永久错误（工具循环耗尽等运行时问题）与渠道健康无关，不计入。
                        if error.error_type == AGENT_RUN_ERROR_TYPE_PROVIDER_TRANSPORT {
                            health_failure(&key, plan.now_ms);
                        }
                        attempts.push(AgentRunChannelAttempt {
                            provider: provider.name.clone(),
                            model: model.id.clone(),
                            http_status: error.http_status,
                            outcome: OUTCOME_PERMANENT,
                        });
                        return Err(finalize_agent_run_error(*error, attempts));
                    }
                    ChannelFailureKind::Transient => {
                        health_failure(&key, plan.now_ms);
                        attempts.push(AgentRunChannelAttempt {
                            provider: provider.name.clone(),
                            model: model.id.clone(),
                            http_status: error.http_status,
                            outcome: if error.visible_output_emitted {
                                OUTCOME_VISIBLE_OUTPUT
                            } else {
                                OUTCOME_TRANSIENT
                            },
                        });
                        if index == last_index {
                            let mut aggregated = *error;
                            aggregated.message = format!(
                                "全部 {} 个渠道均失败，最后错误：{}",
                                plan.chain.len(),
                                aggregated.message
                            );
                            return Err(finalize_agent_run_error(aggregated, attempts));
                        }
                        let (next_model, next_provider) = &plan.chain[index + 1];
                        observe(AgentRunChannelEvent::failover(
                            model,
                            provider,
                            next_model,
                            next_provider,
                            error.http_status,
                            &error.message,
                        ));
                    }
                }
            }
        }
    }

    Err(Box::new(AgentRunExecutionError {
        message: "渠道链为空，无可用渠道".to_owned(),
        http_status: None,
        error_type: AGENT_RUN_ERROR_TYPE_CONFIGURATION,
        error_code: None,
        provider_protocol: None,
        error_param: None,
        failure_kind: None,
        call_index: None,
        provider_error: None,
        token_usage: None,
        tool_receipts: Box::new([]),
        visible_output_emitted: false,
        channel_attempts: attempts,
        provider_event_type: None,
        upstream_request_id: None,
        committed_side_effects: false,
    }))
}

/// Classify whether a failed run can advance to the next channel.
pub fn classify_agent_run_failover_error<ToolReceipt>(
    error: &AgentRunExecutionError<ToolReceipt>,
) -> ChannelFailureKind {
    if error.error_type == AGENT_RUN_ERROR_TYPE_PROVIDER_TRANSPORT {
        classify_channel_failure(error.http_status, &error.message)
    } else {
        ChannelFailureKind::Permanent
    }
}

fn finalize_agent_run_error<ToolReceipt>(
    mut error: AgentRunExecutionError<ToolReceipt>,
    attempts: Vec<AgentRunChannelAttempt>,
) -> Box<AgentRunExecutionError<ToolReceipt>> {
    error.channel_attempts = attempts;
    Box::new(error)
}

/// 判断已有可见输出的失败是否属于可安全切换渠道的情形。
///
/// 判定分三类：
/// - 渠道无关的确定性请求格式错误（Provider 明确说请求体或工具调用格式不被接受）：换渠道确实能过。
/// - 瞬时传输失败且**没有提交不可重放副作用**：允许换渠道续跑，已有工具回执与部分正文继续沿用。
/// - 其余情况（已写文件、永久性错误）：停下等人工裁决，避免把副作用做第二遍。
fn can_failover_after_visible_output<ToolReceipt>(
    error: &AgentRunExecutionError<ToolReceipt>,
) -> bool {
    if !error.visible_output_emitted
        || error.committed_side_effects
        || error.error_type != AGENT_RUN_ERROR_TYPE_PROVIDER_TRANSPORT
    {
        return false;
    }
    if error.http_status == Some(400)
        && error.message.contains(PROVIDER_REQUEST_BODY_INVALID_MARKER)
        && error.message.contains(INVALID_TOOL_USE_FORMAT_MARKER)
    {
        return true;
    }
    classify_channel_failure(error.http_status, &error.message) == ChannelFailureKind::Transient
}
