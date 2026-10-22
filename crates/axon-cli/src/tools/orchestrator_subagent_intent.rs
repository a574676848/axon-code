//! 同一父 turn 的子代理 spawn 意图幂等。

use axon_agent_runtime::{
    AgentPath, ProviderToolCall, SPAWN_AGENT_TOOL_NAME, SubagentSpawnIntentClaim,
    SubagentToolExecutionReceipt,
};
use axon_crypto::sha256_hex;

use crate::tool_orchestrator::ToolOrchestratorContext;

pub(super) enum SpawnIntentExecution {
    Owner(String),
    Duplicate(axon_agent_runtime::SubagentSpawnIntentWaiter),
}

pub(super) fn claim_spawn_intent(
    context: &ToolOrchestratorContext,
    tool_call: &ProviderToolCall,
    parent_path: &AgentPath,
) -> Option<SpawnIntentExecution> {
    let fingerprint = spawn_intent_fingerprint(tool_call)?;
    let claim = context
        .subagent_sessions
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .claim_spawn_intent(parent_path.as_str(), &fingerprint);
    Some(match claim {
        SubagentSpawnIntentClaim::Owner => SpawnIntentExecution::Owner(fingerprint),
        SubagentSpawnIntentClaim::Duplicate(waiter) => SpawnIntentExecution::Duplicate(waiter),
    })
}

pub(super) fn complete_spawn_intent(
    context: &ToolOrchestratorContext,
    parent_path: &AgentPath,
    fingerprint: Option<&str>,
    result: Result<SubagentToolExecutionReceipt, String>,
) {
    let Some(fingerprint) = fingerprint else {
        return;
    };
    context
        .subagent_sessions
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .complete_spawn_intent(parent_path.as_str(), fingerprint, result);
}

pub(crate) fn spawn_intent_fingerprint(tool_call: &ProviderToolCall) -> Option<String> {
    let model_intent = match tool_call.dispatch_name() {
        SPAWN_AGENT_TOOL_NAME | "builtin.spawn_agent" => "inherit",
        axon_agent_runtime::DELEGATE_TO_FAST_TOOL_NAME | "builtin.delegate_to_fast" => "fast",
        _ => return None,
    };
    let arguments = tool_call.input.as_json()?;
    let task = first_text(arguments, &["task"])?;
    let task_name = first_text(arguments, &["task_name"]).unwrap_or(model_intent);
    let fork_turns = first_text(arguments, &["fork_turns"]).unwrap_or("none");
    let normalized = serde_json::json!({
        "task_name": normalize_task_name(task_name),
        "task": normalize_intent_text(task),
        "fork_turns": normalize_intent_text(fork_turns),
        "model_intent": model_intent,
    });
    Some(sha256_hex(normalized.to_string().as_bytes()))
}

fn first_text<'a>(value: &'a serde_json::Value, fields: &[&str]) -> Option<&'a str> {
    fields
        .iter()
        .find_map(|field| value.get(*field).and_then(serde_json::Value::as_str))
}

fn normalize_task_name(value: &str) -> String {
    let normalized = normalize_intent_text(value);
    let Some((prefix, suffix)) = normalized.rsplit_once('_') else {
        return normalized;
    };
    if suffix.chars().all(|ch| ch.is_ascii_digit()) {
        prefix.to_owned()
    } else {
        normalized
    }
}

fn normalize_intent_text(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_merges_generated_numeric_suffix_but_keeps_distinct_tasks() {
        let call = |task_name: &str, task: &str| {
            ProviderToolCall::new(
                format!("call-{task_name}"),
                SPAWN_AGENT_TOOL_NAME,
                serde_json::json!({
                    "task_name": task_name,
                    "task": task,
                    "fork_turns": "all"
                })
                .to_string(),
            )
        };

        assert_eq!(
            spawn_intent_fingerprint(&call("review", "检查 API")),
            spawn_intent_fingerprint(&call("review_2", "  检查   API "))
        );
        assert_ne!(
            spawn_intent_fingerprint(&call("review", "检查 API")),
            spawn_intent_fingerprint(&call("security", "检查 API"))
        );
    }
}
