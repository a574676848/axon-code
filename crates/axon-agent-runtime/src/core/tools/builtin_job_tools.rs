//! 后台作业工具的 schema 与定义。
//!
//! 这两个工具是「后台长任务」这条通道的入口。它们与 `write_stdin` 的关键差别是
//! **并发策略**：作业工具是 `ParallelSafe`，因此含它们的批次不会被强制串行。
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

const JOB_OUTPUT_TOOL_SCHEMA_VERSION: u32 = 1;
const JOB_KILL_TOOL_SCHEMA_VERSION: u32 = 1;

/// `job_id` 字段的统一说明：两条产出通道（Shell 后台作业、后台子代理）共用同一注册表。
const JOB_TOOL_JOB_ID_FIELD_DESCRIPTION: &str = "回执给出的作业 ID：来自 shell_command 以 run_in_background: true 启动后的回执，或来自后台子代理（spawn_agent）回执的 jobId。必须逐字复制最近一次回执中的 job_id；禁止臆造、改写或跨 Agent run 复用。job_id 是临时 Job 的唯一生命周期句柄，不能传给 write_stdin 或 send_message，session_id 与 AgentPath 也不能传给本工具。";

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
                "可选等待增量输出时长，单位毫秒；省略时立即返回本批输出。作业仍在运行且已在等待窗口内有新输出、进入终态或窗口结束时返回。",
            ),
            ToolSchemaField::new(
                JOB_TOOL_MAX_OUTPUT_TOKENS_FIELD,
                ToolSchemaFieldKind::Integer,
                "可选本次回执输出预算，单位 token；默认 2000。",
            ),
        ],
    );
    schema.input_schema["properties"][JOB_TOOL_MAX_OUTPUT_TOKENS_FIELD]["minimum"] =
        serde_json::json!(1);
    schema.input_schema["examples"] = serde_json::json!([
        { "job_id": "1" },
        { "job_id": "1", "yield_time_ms": 30000 }
    ]);
    ToolDefinition {
        id: JOB_OUTPUT_TOOL_ID,
        namespace: crate::AGENT_TOOL_NAMESPACE.to_owned(),
        name: JOB_OUTPUT_TOOL_NAME.to_owned(),
        description: "读取临时 Job（Shell 后台作业或后台子代理）的增量输出与状态。job_id 必须逐字复制最近一次回执中的 job_id，不能臆造或跨作业复用；job_id 是生命周期通道的句柄，与 session_id（write_stdin 的交互式 Shell 会话）、AgentPath（子代理协作寻址）属于不同通道，不可互传：job_id 不能传给 write_stdin，session_id 也不能传给本工具。回执返回本批新增的 stdout/stderr、status 与 exit_code；status 为 Running 只表示作业仍在运行，不表示已成功，Timeout 与 Unresponsive 分别表示墙钟超时与空闲超时。输出超出预算时按 full_output_available 决定是读取完整归档，还是缩小范围后重新执行。作业终态后按保留窗口回收，此后读取会提示已超出保留窗口。只想等待时传 yield_time_ms。".to_owned(),
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
        description: "终止一个临时 Job（Shell 后台作业或后台子代理）。job_id 必须逐字复制最近一次回执中的 job_id；job_id 与 session_id、AgentPath 属于不同通道，不可互传。对后台子代理执行完整中断语义：中止当前 turn、关闭会话、投影终态并触发完成通知。作业已进入终态时返回提示而非重复终止。终止后仍可用 job_output 读取已产生的输出与终态。".to_owned(),
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
