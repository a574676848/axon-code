//! 与 transport 无关的 Agent Turn 执行边界。

use super::provider_request_builder::{ProviderTurnRequest, build_provider_turn_request};
use crate::model::{
    ActionExecutionRequest, NextExecutionRequest, RuntimeEvent, SessionExecutionContext,
    TURN_COMPLETED_EVENT, TurnExecutionError, TurnExecutionOutcome,
};
use crate::ports::{TurnEventSink, TurnExecutor};
use axon_protocol::wire::WireEventEnvelope;
use axon_session::CheckpointTelemetry;

const WIRE_EVENT: &str = "wire_event";
const REMOTE_CONTROL_ACTION_KIND: &str = "remote_control";
const REMOTE_CONTROL_RESPONSE_PREFIX: &str = "远控控制命令已持久化处理：";

/// Provider runner 的规范化输出。
#[derive(Debug, Clone)]
pub struct ProviderTurnResult {
    /// 最终回复文本。
    pub response: String,
    /// Provider/tool loop 产生的稳定 wire events。
    pub wire_events: Vec<WireEventEnvelope>,
    /// 可选下一步 continuation。
    pub continuation: Option<ContinuationResult>,
    /// 可选 transcript/rollout 引用。
    pub transcript_ref: Option<String>,
    /// Provider 返回的 token 使用量。
    pub token_usage: Option<u64>,
    /// Provider turn 耗时。
    pub time_usage_ms: Option<u64>,
    /// Agent tree 摘要。
    pub agent_tree_summary: Option<String>,
    /// 已确认 tool receipt 序号。
    pub tool_receipt_sequence: Option<u64>,
}

/// Provider runner 的 continuation。
#[derive(Debug, Clone)]
pub struct ContinuationResult {
    /// 关联的 Goal；缺失时由当前 Action 的 Goal 继承。
    pub goal_id: Option<axon_domain::GoalId>,
    /// 父 Execution；缺失时由当前 Action 补齐。
    pub parent_execution_id: Option<axon_domain::ExecutionId>,
    /// 当前阶段标识。
    pub stage: Option<String>,
    /// 原始 transport context JSON。
    pub transport_context_json: Option<String>,
    /// Action 类型。
    pub kind: String,
    /// 优先级。
    pub priority: i32,
    /// 幂等键。
    pub dedupe_key: String,
    /// JSON payload。
    pub payload_json: String,
}

/// 具体 Provider/Agent 实现由组合根注入。
pub trait ProviderTurnRunner: Send + Sync {
    /// 执行一次 provider turn；失败必须携带结构化类别。
    fn run(&self, request: ProviderTurnRequest) -> Result<ProviderTurnResult, TurnExecutionError>;

    /// 带实时事件通道地执行一次 provider turn。
    ///
    /// 支持实时投影的实现必须在运行过程中把每条运行事件即时送入 `sink`，并照常返回
    /// 规范化结果：收口时的持久投影仍以返回结果为准，逐条通道只补足过程中的可见性。
    /// 默认实现不支持实时投影，退化为批量执行。
    fn run_streaming(
        &self,
        request: ProviderTurnRequest,
        _sink: Option<&dyn TurnEventSink>,
    ) -> Result<ProviderTurnResult, TurnExecutionError> {
        self.run(request)
    }

    /// 执行不需要 Provider 请求的本地持久 Action。
    fn run_local_action(
        &self,
        _request: ProviderTurnRequest,
    ) -> Result<ProviderTurnResult, TurnExecutionError> {
        Err(TurnExecutionError::permanent(
            "当前 Provider runner 不支持本地持久 Action",
        ))
    }
}

/// 把持久 Action 转换为 provider 输入并规范化事件输出。
#[derive(Debug, Clone)]
pub struct AgentTurnExecutor<R> {
    runner: R,
}

impl<R> AgentTurnExecutor<R> {
    /// 创建执行器。
    #[must_use]
    pub const fn new(runner: R) -> Self {
        Self { runner }
    }
}

impl<R: ProviderTurnRunner> AgentTurnExecutor<R> {
    fn execute_inner(
        &self,
        request: ActionExecutionRequest,
        sink: Option<&dyn TurnEventSink>,
    ) -> Result<TurnExecutionOutcome, TurnExecutionError> {
        let ActionExecutionRequest {
            action,
            context,
            runtime_profile,
        } = request;
        if action.kind == REMOTE_CONTROL_ACTION_KIND {
            return execute_remote_control_action(&action, &context);
        }
        let input = build_provider_turn_request(&action, &context, runtime_profile)
            .map_err(TurnExecutionError::permanent)?;
        let transport_context = input.transport_context.clone();
        let mut result = self.runner.run_streaming(input, sink)?;
        if action.goal_id.is_some()
            && result.continuation.is_none()
            && let Some(report) = super::goal_execution_report::parse(&result.response)
            && let Some(next) = report.continuation
        {
            if next.stage.trim().is_empty() || next.prompt.trim().is_empty() {
                return Err(TurnExecutionError::permanent(
                    "Task continuation 的 stage 和 prompt 不能为空",
                ));
            }
            result.continuation = Some(ContinuationResult {
                goal_id: action.goal_id.clone(),
                parent_execution_id: axon_domain::ExecutionId::new(action.action_id.as_str()).ok(),
                stage: Some(next.stage.clone()),
                transport_context_json: transport_context.as_ref().map(ToString::to_string),
                kind: action.kind.clone(),
                priority: action.priority,
                dedupe_key: format!("next-{}-{}", action.action_id.as_str(), next.stage),
                payload_json: serde_json::json!({"prompt": next.prompt}).to_string(),
            });
        }
        if let Some(sink) = sink {
            // 顺序契约：canonical assistant -> foreground/run completed -> turn_completed。
            // 收口失败必须在此终止，禁止在 durable 缺口上继续追加 turn_completed。
            super::agent_turn_canonical_terminal::publish_canonical_terminal_events(
                sink,
                &result.wire_events,
            )?;
        }
        let response_payload = serde_json::json!({
            "actionId": action.action_id,
            "kind": action.kind,
            "response": result.response,
            "transportContext": transport_context,
        })
        .to_string();
        let next_execution = result.continuation.map(|item| NextExecutionRequest {
            goal_id: action.goal_id.clone(),
            parent_execution_id: axon_domain::ExecutionId::new(action.action_id.as_str()).ok(),
            stage: item.stage,
            transport_context_json: transport_context.map(|value| value.to_string()),
            kind: item.kind,
            priority: item.priority,
            dedupe_key: item.dedupe_key,
            payload_json: item.payload_json,
            goal_revision: action.goal_revision,
            context_version: action.context_version,
        });
        let checkpoint = CheckpointTelemetry {
            transcript_ref: result.transcript_ref,
            token_usage: result.token_usage,
            time_usage_ms: result.time_usage_ms,
            agent_tree_summary: result.agent_tree_summary,
            tool_receipt_sequence: result.tool_receipt_sequence,
        };
        let mut events = if sink.is_none() {
            result
                .wire_events
                .into_iter()
                .map(|event| {
                    wire_event_to_runtime_event(&event).expect("WireEventEnvelope 序列化必须成功")
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        events.push(RuntimeEvent {
            event_type: TURN_COMPLETED_EVENT.to_owned(),
            payload_json: response_payload,
        });
        Ok(TurnExecutionOutcome {
            events,
            next_execution,
            checkpoint,
        })
    }
}

impl<R: ProviderTurnRunner> TurnExecutor for AgentTurnExecutor<R> {
    fn execute(
        &self,
        request: ActionExecutionRequest,
    ) -> Result<TurnExecutionOutcome, TurnExecutionError> {
        self.execute_inner(request, None)
    }

    fn execute_streaming(
        &self,
        request: ActionExecutionRequest,
        sink: Option<&dyn TurnEventSink>,
    ) -> Result<TurnExecutionOutcome, TurnExecutionError> {
        self.execute_inner(request, sink)
    }
}

/// 把 Provider wire 事件规范化为实时投影使用的 Runtime 事件。
pub fn wire_event_to_runtime_event(event: &WireEventEnvelope) -> Result<RuntimeEvent, String> {
    serde_json::to_string(event)
        .map(|payload_json| RuntimeEvent {
            event_type: WIRE_EVENT.to_owned(),
            payload_json,
        })
        .map_err(|error| format!("Provider wire 事件序列化失败：{error}"))
}

fn execute_remote_control_action(
    action: &axon_session::SessionActionRecord,
    context: &SessionExecutionContext,
) -> Result<TurnExecutionOutcome, TurnExecutionError> {
    let payload: serde_json::Value =
        serde_json::from_str(&action.payload_json).map_err(|error| {
            TurnExecutionError::permanent(format!("远控控制 Action payload 解析失败：{error}"))
        })?;
    let control = payload
        .get("control")
        .ok_or_else(|| TurnExecutionError::permanent("远控控制 Action 缺少 control payload"))?;
    let preview = control
        .get("preview")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("控制命令");
    let session_id = &context.run_session_id;
    let response_payload = serde_json::json!({
        "actionId": action.action_id,
        "kind": action.kind,
        "response": remote_control_response(preview, session_id),
        "transportContext": payload.get("transportContext").cloned(),
    })
    .to_string();
    Ok(TurnExecutionOutcome {
        events: vec![RuntimeEvent {
            event_type: TURN_COMPLETED_EVENT.to_owned(),
            payload_json: response_payload,
        }],
        next_execution: None,
        checkpoint: CheckpointTelemetry::default(),
    })
}

fn remote_control_response(preview: &str, _session_id: &str) -> String {
    let command = preview
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .trim_start_matches('/')
        .to_ascii_lowercase();
    match command.as_str() {
        "stop" | "cancel" | "interrupt" => "已停止当前会话。".to_owned(),
        "status" => "状态: 已处理".to_owned(),
        "queue" => "队列查询已完成。".to_owned(),
        _ => format!("{REMOTE_CONTROL_RESPONSE_PREFIX}{preview}"),
    }
}

#[cfg(test)]
#[path = "agent_turn_executor_tests.rs"]
mod tests;
