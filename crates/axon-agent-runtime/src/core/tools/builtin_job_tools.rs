//! 后台作业工具的 schema 与定义。
//!
//! 这两个工具是「后台长任务」这条通道的入口。并发策略按 PAR-1 区分：
//! `job_output` 是只读快照，`ParallelSafe`；`job_kill` 改运行态，`Exclusive`。
//!
//! 生命周期所有权在 `shell_command` 返回 `job_id` 的那一刻移交给作业系统：
//! 作业不再绑定发起它的 tool call，只能通过 `job_kill` 或取消标记终止。

use axon_domain::{
    ToolDefinition, ToolId, ToolOutputPolicy, ToolPermissionPolicy, ToolSchema, ToolSchemaField,
    ToolSchemaFieldKind,
};

/// 作业输出工具名。
pub const JOB_OUTPUT_TOOL_NAME: &str = "job_output";
/// 作业输出工具 ID。
pub const JOB_OUTPUT_TOOL_ID: ToolId = ToolId(38);
/// 作业终止工具名。
pub const JOB_KILL_TOOL_NAME: &str = "job_kill";
/// 作业终止工具 ID。
pub const JOB_KILL_TOOL_ID: ToolId = ToolId(39);

/// `job_id` 字段名。
pub const JOB_TOOL_JOB_ID_FIELD: &str = "job_id";
/// `yield_time_ms` 字段名；与 `shell_command` 共用同名同义字段。
pub const JOB_TOOL_YIELD_TIME_FIELD: &str = "yield_time_ms";
/// `max_output_tokens` 字段名；与 `shell_command` 共用同名同义字段。
pub const JOB_TOOL_MAX_OUTPUT_TOKENS_FIELD: &str = "max_output_tokens";
/// `yield_time_ms` 的最大上限（毫秒，10 秒）：严禁前台长时间等待轮询。
pub const JOB_TOOL_MAX_YIELD_TIME_MS: u64 = 10_000;

const JOB_OUTPUT_TOOL_SCHEMA_VERSION: u32 = 1;
const JOB_KILL_TOOL_SCHEMA_VERSION: u32 = 1;

/// `job_id` 字段的统一说明：两条产出通道（Shell 后台作业、后台子代理）共用同一注册表。
const JOB_TOOL_JOB_ID_FIELD_DESCRIPTION: &str = "启动后台任务时回执给出的编号，后台子代理的 jobId 也走这里。照抄回执里的数字就行：不要自己编、不要改动，也不要把别的任务的编号拿来用。这个编号只给 job_output 和 job_kill 用，不能传给 write_stdin 或 send_message；反过来，会话 ID 也不能传给这两个工具。";

/// 返回 `job_output` 工具定义。
pub(crate) fn job_output_tool_definition() -> ToolDefinition {
    let mut schema = ToolSchema::object(
        JOB_OUTPUT_TOOL_SCHEMA_VERSION,
        [JOB_TOOL_JOB_ID_FIELD],
        vec![
            ToolSchemaField::new(
                JOB_TOOL_JOB_ID_FIELD,
                ToolSchemaFieldKind::NonBlankString,
                JOB_TOOL_JOB_ID_FIELD_DESCRIPTION,
            ),
            ToolSchemaField::new(
                JOB_TOOL_YIELD_TIME_FIELD,
                ToolSchemaFieldKind::Integer,
                "可选，最多 10000（10 秒）。传了会在这里稍等一下，看这段时间里有没有新输出或任务是否结束；不传就立刻返回当前已有的内容。任务跑完会自动通知你，所以平时不必靠这个参数去等。",
            ),
            ToolSchemaField::new(
                JOB_TOOL_MAX_OUTPUT_TOKENS_FIELD,
                ToolSchemaFieldKind::Integer,
                "可选本次回执输出预算，单位 token；默认 2000。",
            ),
        ],
    );
    schema.input_schema["properties"][JOB_TOOL_YIELD_TIME_FIELD]["minimum"] = serde_json::json!(0);
    schema.input_schema["properties"][JOB_TOOL_YIELD_TIME_FIELD]["maximum"] =
        serde_json::json!(JOB_TOOL_MAX_YIELD_TIME_MS);
    schema.input_schema["properties"][JOB_TOOL_MAX_OUTPUT_TOKENS_FIELD]["minimum"] =
        serde_json::json!(1);
    schema.input_schema["examples"] = serde_json::json!([
        { "job_id": "1" },
        { "job_id": "1", "yield_time_ms": 10000 }
    ]);
    ToolDefinition {
        id: JOB_OUTPUT_TOOL_ID,
        namespace: crate::AGENT_TOOL_NAMESPACE.to_owned(),
        name: JOB_OUTPUT_TOOL_NAME.to_owned(),
        description: "看一个后台任务跑到哪了：它到目前为止的输出，以及是否还在跑。要停掉它请用 job_kill。启动回执里的 job_id 照抄即可。任务跑完会自动通知你，不需要守着反复来问；只在需要看进度或取结果时调用。每次返回的是「上次读取之后新增的那一段」，不是从头到尾的全文；内容太多时只给摘要并标明完整输出在哪里。任务结束后短时间内仍可读取，再久会提示已经查不到，那时需要重新执行一次。".to_owned(),
        schema,
        enabled: true,
        concurrency_policy: axon_domain::ToolConcurrencyPolicy::ParallelSafe,
        output_policy: ToolOutputPolicy::RefOnOverflow,
        permission_policy: ToolPermissionPolicy::CallEvaluated,
    }
}

/// 返回 `job_kill` 工具定义。
pub(crate) fn job_kill_tool_definition() -> ToolDefinition {
    let mut schema = ToolSchema::object(
        JOB_KILL_TOOL_SCHEMA_VERSION,
        [JOB_TOOL_JOB_ID_FIELD],
        vec![ToolSchemaField::new(
            JOB_TOOL_JOB_ID_FIELD,
            ToolSchemaFieldKind::NonBlankString,
            JOB_TOOL_JOB_ID_FIELD_DESCRIPTION,
        )],
    );
    schema.input_schema["examples"] = serde_json::json!([{ "job_id": "1" }]);
    ToolDefinition {
        id: JOB_KILL_TOOL_ID,
        namespace: crate::AGENT_TOOL_NAMESPACE.to_owned(),
        name: JOB_KILL_TOOL_NAME.to_owned(),
        description: "停掉一个后台任务，包括后台子代理。job_id 照抄启动回执里的编号。已经结束的任务会提示无需重复停止。停掉子代理等同于中断它：它会立刻停下，不再继续做。停掉之后仍可以用 job_output 查看它已经产生的输出和结局。".to_owned(),
        schema,
        enabled: true,
        concurrency_policy: axon_domain::ToolConcurrencyPolicy::Exclusive,
        output_policy: ToolOutputPolicy::BoundedSummary,
        permission_policy: ToolPermissionPolicy::CallEvaluated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `job_output` 只读快照可并行；`job_kill` 改运行态，必须独占。
    #[test]
    fn job_tools_split_read_and_mutation() {
        assert_eq!(
            job_output_tool_definition().concurrency_policy,
            axon_domain::ToolConcurrencyPolicy::ParallelSafe
        );
        assert_eq!(
            job_kill_tool_definition().concurrency_policy,
            axon_domain::ToolConcurrencyPolicy::Exclusive
        );
    }

    /// 两个工具的 ID 与名字不得重复或与既有工具冲突。
    #[test]
    fn job_tool_identities_are_distinct() {
        let output = job_output_tool_definition();
        let kill = job_kill_tool_definition();

        assert_ne!(output.id, kill.id, "两个作业工具的 ID 必须不同");
        assert_ne!(output.name, kill.name, "两个作业工具的名字必须不同");
        assert_eq!(output.id, JOB_OUTPUT_TOOL_ID);
        assert_eq!(kill.id, JOB_KILL_TOOL_ID);
    }

    /// `job_id` 必须是必填字段；漏填时应在 schema 层被拒绝。
    #[test]
    fn job_id_is_required_by_both_tools() {
        for definition in [job_output_tool_definition(), job_kill_tool_definition()] {
            let required = definition.schema.input_schema["required"]
                .as_array()
                .unwrap_or_else(|| panic!("{} 必须有 required 列表", definition.name));
            assert!(
                required.iter().any(|field| field == JOB_TOOL_JOB_ID_FIELD),
                "{} 必须把 job_id 列为必填",
                definition.name
            );
        }
    }

    /// 工具描述必须点明 `job_id` 与 `session_id` 不可互传，
    /// 否则模型可能把作业 ID 拿去 write_stdin，形成双通道。
    #[test]
    fn job_output_description_forbids_cross_channel_ids() {
        let description = job_output_tool_definition().description;

        assert!(description.contains("write_stdin"));
        assert!(description.contains("session_id"));
    }

    /// 终止工具同属作业通道，必须给出同样的通道隔离说明。
    #[test]
    fn job_kill_description_forbids_cross_channel_ids() {
        let description = job_kill_tool_definition().description;

        assert!(description.contains("session_id"));
        assert!(description.contains("不可互传"));
    }
}
