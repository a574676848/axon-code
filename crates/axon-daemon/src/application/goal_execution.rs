//! 后台 Goal Execution 的自包含 payload 构造。
//!
//! Worker 没有主 turn 历史，payload 必须自带 prompt 与渠道上下文，否则恢复后
//! 只能解析出空输入。prompt 以 Goal 契约为唯一执行依据，避免后台重复创建 Goal。

use serde_json::Value;

/// 后台 Execution 的执行指令模板。
const GOAL_EXECUTION_PROMPT_TEMPLATE: &str = "\
执行已承诺的 Goal，直接完成工作并给出结论；不要重新创建 Goal，也不要仅复述计划。
最终回复直接写完整执行结果，包含关键输出、错误和下一步；不要填写工具调用 ID、archiveRef、session_id 等内部标识。
需要后续阶段时增加 continuation: {\"stage\":\"尚未执行的阶段标识\",\"prompt\":\"自包含下一阶段指令，包含验收条件和本报告格式\"}。
后续阶段必须有本阶段的真实成功工具回执作为进展；不要重复已执行阶段或更改任务受众。summary 应包含完整交付结果，不能只说已完成。

标题：{title}
目标：{objective}
执行步骤：
{steps}
验收标准：
{acceptance}";

/// 用 Goal 契约字段渲染后台执行 prompt。
#[must_use]
pub(super) fn prompt(
    title: &str,
    objective: &str,
    steps: &[String],
    acceptance: &[String],
) -> String {
    GOAL_EXECUTION_PROMPT_TEMPLATE
        .replace("{title}", title)
        .replace("{objective}", objective)
        .replace("{steps}", &bullet_list(steps))
        .replace("{acceptance}", &bullet_list(acceptance))
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

fn bullet_list(items: &[String]) -> String {
    items
        .iter()
        .map(|item| format!("- {item}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::{payload, prompt};

    #[test]
    fn payload_carries_prompt_and_transport_context() {
        let prompt = prompt("标题", "目标", &["第一步".to_owned()], &["验收".to_owned()]);
        assert!(prompt.contains("标题：标题"));
        assert!(prompt.contains("- 第一步"));
        let payload = payload(&prompt, serde_json::json!({"channel": "tui"}));
        let decoded: serde_json::Value =
            serde_json::from_str(&payload).expect("payload 必须是合法 JSON");
        assert_eq!(decoded["prompt"], serde_json::json!(prompt));
        assert_eq!(decoded["transportContext"]["channel"], "tui");
    }
}
