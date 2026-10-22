//! 主 turn Goal 工具执行适配。
//!
//! 工具执行进程不直连 Goal 事实源：先解析模型输入、绑定运行时 Workspace 与
//! 幂等键，再通过 Daemon IPC 完成查询与写入；Daemon 是 Goal mutation 的唯一写入口。

use std::path::Path;

use axon_agent_runtime::{GoalToolRuntime, ProviderToolCall};
use axon_protocol::{
    DAEMON_PROTOCOL_VERSION, DaemonCommand, DaemonRequest, DaemonResponse, GoalToolRequest,
};
use axon_session::WorkspaceResolver;
use serde_json::Value;

use crate::tool_orchestrator::{
    ToolOrchestratorContext, ToolReceipt, completed_tool_receipt, failed_tool_input_receipt,
    failed_tool_receipt, failed_tool_receipt_from_axon_error, now_ms,
};

/// Goal 工具的默认 actor；渠道接入后由运行时注入真实来源。
const GOAL_TOOL_DEFAULT_ACTOR: &str = "local_agent";
/// Daemon JSONL envelope 的消息类型。
const DAEMON_MESSAGE_TYPE: &str = "command";
/// Goal 工具 IPC 客户端标识。
const GOAL_TOOL_CLIENT_ID: &str = "axon-cli-goal-tool";
/// 幂等键保留的 SHA-256 前缀长度。
const IDEMPOTENCY_KEY_LENGTH: usize = 32;

/// 执行一个主 turn Goal 工具调用。
pub(super) fn execute_goal_tool(
    tool_call: &ProviderToolCall,
    context: &ToolOrchestratorContext,
) -> ToolReceipt {
    let Some(value) = tool_call.input.as_json() else {
        return failed_tool_input_receipt(tool_call, "Goal 工具需要 JSON 对象输入");
    };
    // 参数解析全链路只在 runtime 的 GoalToolRuntime::parse_command 内发生一次；
    // 本层不持有逐命令的名称或字段知识，只透传结果并绑定运行时上下文。
    let command = match GoalToolRuntime::parse_command(&tool_call.name, value) {
        Ok(command) => command,
        Err(error) => return failed_tool_receipt_from_axon_error(tool_call, error),
    };
    let endpoint = match crate::runtime_paths::resolve_daemon_endpoint(
        crate::runtime_paths::DEFAULT_DAEMON_ENDPOINT,
        context.config_path.as_deref(),
    ) {
        Ok(endpoint) => endpoint,
        Err(message) => {
            return failed_tool_receipt(tool_call, format!("Goal 工具需要 Daemon：{message}"));
        }
    };
    let idempotency_key = idempotency_key(context, tool_call);
    let request = DaemonRequest {
        message_type: DAEMON_MESSAGE_TYPE.to_owned(),
        request_id: format!("goal-tool-{idempotency_key}"),
        protocol_version: DAEMON_PROTOCOL_VERSION,
        client_id: GOAL_TOOL_CLIENT_ID.to_owned(),
        session_id: Some(context.thread_session_id.clone()),
        command: DaemonCommand::GoalTool(GoalToolRequest {
            actor: GOAL_TOOL_DEFAULT_ACTOR.to_owned(),
            workspace_id: workspace_id(&context.cwd),
            idempotency_key,
            source: Some(axon_protocol::GoalSource::OneOff),
            disclosure_policy: Some(context.goal_disclosure_policy),
            transport_context: Some(context.goal_transport_context.clone().unwrap_or_else(|| serde_json::json!({
                "channel": crate::run_commands::CLI_INTERACTION_CHANNEL,
                "destination": crate::run_commands::cli_reply_destination(),
                "audience": axon_domain::AudienceContext::new(axon_domain::AudienceKind::Private, context.goal_disclosure_policy),
            }))),
            command,
        }),
    };
    match send_request(&endpoint, &request) {
        Ok(response) if response.ok => completed_tool_receipt(
            tool_call,
            summarize(&response.body),
            Some(response.body),
            false,
        ),
        Ok(response) => failed_tool_receipt(tool_call, daemon_error_message(&response)),
        Err(message) => failed_tool_receipt(tool_call, message),
    }
}

/// 解析运行时绑定的 Workspace；无法解析时退化为仅全局作用域。
fn workspace_id(cwd: &Path) -> Option<String> {
    WorkspaceResolver
        .resolve(cwd, now_ms())
        .ok()
        .map(|workspace| workspace.workspace_id)
}

/// 由会话与 provider 调用标识派生稳定幂等键，保证工具重试不产生重复 Goal。
fn idempotency_key(context: &ToolOrchestratorContext, tool_call: &ProviderToolCall) -> String {
    derive_idempotency_key(&[
        context.thread_session_id.as_str(),
        &context.session_id.0.to_string(),
        tool_call.id.as_str(),
    ])
}

/// 以 `:` 连接素材并截断 SHA-256，控制存储与日志体积。
fn derive_idempotency_key(material: &[&str]) -> String {
    axon_crypto::sha256_hex(material.join(":").as_bytes())
        .chars()
        .take(IDEMPOTENCY_KEY_LENGTH)
        .collect()
}

fn send_request(endpoint: &str, request: &DaemonRequest) -> Result<DaemonResponse, String> {
    axon_daemon::infrastructure::ipc::JsonlIpcClient::connect(endpoint)
        .and_then(|mut client| client.send(request))
        .map_err(|error| format!("Goal 工具调用 Daemon 失败：{error}"))
}

/// 回执正文使用结构化 JSON，主 turn 据此向用户解释结果。
fn summarize(body: &Value) -> String {
    serde_json::to_string(body).unwrap_or_else(|_| "Goal 工具已提交".to_owned())
}

fn daemon_error_message(response: &DaemonResponse) -> String {
    response
        .body
        .get("error")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| "Daemon 拒绝 Goal 工具命令".to_owned())
}

#[cfg(test)]
mod tests {
    use super::{IDEMPOTENCY_KEY_LENGTH, derive_idempotency_key};
    use axon_agent_runtime::GoalToolRuntime;
    use axon_protocol::GoalToolCommand;

    #[test]
    fn goal_tool_names_are_classified_for_main_turn_dispatch() {
        for name in [
            "builtin.goal_create",
            "builtin.goal_list",
            "builtin.goal_get",
            "builtin.goal_start",
            "builtin.goal_update",
            "builtin.goal_cancel",
        ] {
            assert_eq!(
                crate::tool_orchestrator::classify_tool(name),
                crate::tool_orchestrator::ToolKind::GoalTool,
                "{name} 必须分类为 Goal 工具"
            );
        }
        assert_eq!(
            crate::tool_orchestrator::ToolKind::Unsupported,
            crate::tool_orchestrator::classify_tool("builtin.goal_unknown")
        );
        assert_eq!(
            crate::tool_orchestrator::ToolKind::Unsupported,
            crate::tool_orchestrator::classify_tool("builtin.goal_archive")
        );
    }

    /// CLI host 面不持有逐命令知识：Goal 工具一律透传给 runtime 的单一解析实现。
    #[test]
    fn goal_tool_commands_are_delegated_to_the_single_parser() {
        let cases = [
            (
                "builtin.goal_create",
                serde_json::json!({
                    "scope": "current_workspace",
                    "title": "整理发布说明",
                    "objective": "输出可验收的发布说明",
                    "steps": ["汇总变更"],
                    "acceptance": ["发布说明通过审阅"],
                }),
            ),
            (
                "builtin.goal_list",
                serde_json::json!({"scope": "current_workspace", "limit": 5}),
            ),
            ("builtin.goal_get", serde_json::json!({"goalId": "goal-1"})),
            (
                "builtin.goal_start",
                serde_json::json!({"goalId": "goal-1", "expectedRevision": 3}),
            ),
            (
                "builtin.goal_update",
                serde_json::json!({"goalId": "goal-1", "expectedRevision": 3, "nextStep": "复核"}),
            ),
            (
                "builtin.goal_cancel",
                serde_json::json!({"goalId": "goal-1", "expectedRevision": 3, "reason": "用户取消"}),
            ),
        ];
        for (name, input) in cases {
            let command = GoalToolRuntime::parse_command(name, &input)
                .unwrap_or_else(|error| panic!("{name} 必须经单一解析实现成功：{error:?}"));
            let expected = match name {
                "builtin.goal_create" => matches!(command, GoalToolCommand::Create(_)),
                "builtin.goal_list" => matches!(command, GoalToolCommand::List(_)),
                "builtin.goal_get" => matches!(command, GoalToolCommand::Get(_)),
                "builtin.goal_start" => matches!(command, GoalToolCommand::Start(_)),
                "builtin.goal_update" => matches!(command, GoalToolCommand::Update(_)),
                _ => matches!(command, GoalToolCommand::Cancel(_)),
            };
            assert!(expected, "{name} 命令变体与工具名不一致");
        }

        assert!(GoalToolRuntime::parse_command("builtin.unknown", &serde_json::json!({})).is_err());
    }

    #[test]
    fn idempotency_key_is_stable_and_call_scoped() {
        let first = derive_idempotency_key(&["thread-1", "7", "call-1"]);
        assert_eq!(first, derive_idempotency_key(&["thread-1", "7", "call-1"]));
        assert_eq!(first.len(), IDEMPOTENCY_KEY_LENGTH);
        assert_ne!(first, derive_idempotency_key(&["thread-1", "7", "call-2"]));
    }
}
