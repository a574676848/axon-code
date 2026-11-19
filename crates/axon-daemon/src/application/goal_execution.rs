//! 后台 Goal Execution 的自包含 payload 构造。
//!
//! Worker 没有主 turn 历史，payload 必须自带 prompt 与渠道上下文，否则恢复后
//! 只能解析出空输入。prompt 以 Goal 契约为唯一执行依据，避免后台重复创建 Goal；
//! 正文模板在 `axon-prompts`（`templates/agent/goal_execution.md`），这里只做契约
//! 字段到模板输入的映射。

use axon_domain::GoalWritePolicy;
use serde_json::Value;

/// 用 Goal 契约字段渲染后台执行 prompt。
///
/// 写权限随目标持久化：只读目标每次执行（含重新启动）都必须拿到只读口径，
/// 不能因为重试或重启退回允许改动。
#[must_use]
pub(super) fn prompt(
    title: &str,
    objective: &str,
    steps: &[String],
    acceptance: &[String],
    write_policy: GoalWritePolicy,
) -> String {
    axon_prompts::render_goal_execution(&axon_prompts::GoalExecutionTemplateInput {
        title: title.to_owned(),
        objective: objective.to_owned(),
        steps: steps.to_vec(),
        acceptance: acceptance.to_vec(),
        scope: match write_policy {
            GoalWritePolicy::Allow => axon_prompts::GoalExecutionScope::AllowWrites,
            GoalWritePolicy::ReadOnly => axon_prompts::GoalExecutionScope::ReadOnly,
        },
    })
}

/// 组装后台 Execution 的持久 payload。
#[must_use]
pub(super) fn payload(prompt: &str, transport_context: Value) -> String {
    serde_json::json!({
        "prompt": prompt,
        "transportContext": transport_context,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::{payload, prompt};
    use axon_domain::GoalWritePolicy;

    fn contract() -> (&'static str, &'static str, Vec<String>, Vec<String>) {
        (
            "标题",
            "目标",
            vec!["第一步".to_owned()],
            vec!["验收".to_owned()],
        )
    }

    #[test]
    fn payload_carries_prompt_and_transport_context() {
        let (title, objective, steps, acceptance) = contract();
        let prompt = prompt(
            title,
            objective,
            &steps,
            &acceptance,
            GoalWritePolicy::Allow,
        );
        assert!(prompt.contains("标题：标题"));
        assert!(prompt.contains("- 第一步"));
        let payload = payload(&prompt, serde_json::json!({"channel": "tui"}));
        let decoded: serde_json::Value =
            serde_json::from_str(&payload).expect("payload 必须是合法 JSON");
        assert_eq!(decoded["prompt"], serde_json::json!(prompt));
        assert_eq!(decoded["transportContext"]["channel"], "tui");
    }

    /// 只读目标必须拿到只读口径；允许目标必须拿到改动范围约束。
    #[test]
    fn write_policy_selects_the_execution_scope_text() {
        let (title, objective, steps, acceptance) = contract();
        let read_only = prompt(
            title,
            objective,
            &steps,
            &acceptance,
            GoalWritePolicy::ReadOnly,
        );
        let allow = prompt(
            title,
            objective,
            &steps,
            &acceptance,
            GoalWritePolicy::Allow,
        );

        assert!(read_only.contains("本次执行为只读"));
        assert!(!allow.contains("本次执行为只读"));
        assert!(allow.contains("不要顺手重构"));
    }
}
