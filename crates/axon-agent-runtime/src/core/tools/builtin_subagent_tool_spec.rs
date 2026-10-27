//! 子代理协作工具定义。

use axon_domain::{
    ToolConcurrencyPolicy, ToolDefinition, ToolId, ToolOutputPolicy, ToolPermissionPolicy,
    ToolSchema, ToolSchemaField, ToolSchemaFieldKind,
};
use serde_json::json;

use crate::agent_tools::AGENT_TOOL_NAMESPACE;
use crate::{TOOL_INTENT_FIELD, TOOL_INTENT_FIELD_DESCRIPTION};

/// `spawn_agent` 工具 ID。
pub const SPAWN_AGENT_TOOL_ID: ToolId = ToolId(40);
/// `send_message` 工具 ID。
pub const SEND_MESSAGE_TOOL_ID: ToolId = ToolId(41);
/// `list_agents` 工具 ID。
pub const LIST_AGENTS_TOOL_ID: ToolId = ToolId(45);
/// `delegate_to_fast` 工具 ID。
pub const DELEGATE_TO_FAST_TOOL_ID: ToolId = ToolId(48);

/// 创建子代理工具名。
pub const SPAWN_AGENT_TOOL_NAME: &str = "spawn_agent";
/// 发送消息工具名。
pub const SEND_MESSAGE_TOOL_NAME: &str = "send_message";
/// 列出代理工具名。
pub const LIST_AGENTS_TOOL_NAME: &str = "list_agents";
/// 快速模型委托工具名。
pub const DELEGATE_TO_FAST_TOOL_NAME: &str = "delegate_to_fast";

/// 子代理工具 schema 版本。
pub const SUBAGENT_TOOL_SCHEMA_VERSION: u32 = 2;

/// 任务名字段。
pub const SUBAGENT_TOOL_TASK_NAME_FIELD: &str = "task_name";
/// 消息字段。
pub const SUBAGENT_TOOL_MESSAGE_FIELD: &str = "message";
/// fork 上下文字段。
pub const SUBAGENT_TOOL_FORK_TURNS_FIELD: &str = "fork_turns";
/// 目标代理字段。
pub const SUBAGENT_TOOL_TARGET_FIELD: &str = "target";
/// 列表前缀字段。
pub const SUBAGENT_TOOL_PATH_PREFIX_FIELD: &str = "path_prefix";
/// 显式委托任务字段。
pub const SUBAGENT_TOOL_TASK_FIELD: &str = "task";
/// 是否后台执行字段。
pub const SUBAGENT_TOOL_RUN_IN_BACKGROUND_FIELD: &str = "run_in_background";
/// 创建子代理说明。
const SPAWN_AGENT_TOOL_DESCRIPTION: &str = "创建可持续协作的通用子代理；把包含目标、范围、证据、交付与验证的完整首轮任务写入 task，task_name 只作为稳定任务名并仅使用小写字母、数字、'-' 或 '_'。子代理看不到本次对话，task 必须是一份完整独立的 prompt。默认后台执行：立即返回 jobId，完成时运行时把通知回流到下一轮；可用 builtin.job_output / builtin.job_kill 按 jobId 读取结果或终止该子代理。";
/// 快速委托说明。
const DELEGATE_TO_FAST_TOOL_DESCRIPTION: &str = "把一个自包含任务委派给在自身上下文里独立工作的快速子代理，用于卸载聚焦而独立的工作（调研、有边界的实现、分析），不消耗父对话上下文。返回结果而不是中间步骤。子代理看不到本次对话，task 必须是完整独立的 prompt。多个互不依赖的委派应在同一条消息里一次发出并行执行；有先后依赖的必须串行。写仓库时每个子代理的写集必须互不相交，同一文件禁止并发送改。默认后台执行：立即返回 jobId，完成时运行时把结果经通知回流到下一轮，用 builtin.job_output 按 jobId 读取；仅当下一步动作依赖该结果时才传 run_in_background: false 同步等待。";
/// 委托任务字段说明。
const DELEGATE_TO_FAST_TASK_FIELD_DESCRIPTION: &str = "自包含的委托任务：写明目标、范围、证据要求与交付格式。子代理看不到本次对话，这里必须是一份完整独立的 prompt。";
/// 后台执行字段说明。
const DELEGATE_TO_FAST_RUN_IN_BACKGROUND_DESCRIPTION: &str = "是否后台执行。默认 true：立即返回 jobId，完成后经通知回流结果，期间你可以继续做不依赖它的事。仅当下一步动作依赖该结果时才传 false 同步等待并直接拿到回执。";

/// 返回全部子代理工具定义。
///
/// 目标态是四个协作工具：`spawn_agent`（保留在执行注册表但不向模型暴露）、
/// `delegate_to_fast`、`send_message`、`list_agents`。等待与终止不再由子代理工具
/// 承担：生命周期操作一律走 `job_output` / `job_kill` 的 `job_id` 通道。
#[must_use]
pub fn subagent_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        spawn_agent_tool_definition(),
        send_message_tool_definition(),
        list_agents_tool_definition(),
        delegate_to_fast_tool_definition(),
    ]
}

/// `spawn_agent` 工具定义。
#[must_use]
pub fn spawn_agent_tool_definition() -> ToolDefinition {
    ToolDefinition {
        id: SPAWN_AGENT_TOOL_ID,
        namespace: AGENT_TOOL_NAMESPACE.to_owned(),
        name: SPAWN_AGENT_TOOL_NAME.to_owned(),
        description: SPAWN_AGENT_TOOL_DESCRIPTION.to_owned(),
        schema: ToolSchema::new(
            SUBAGENT_TOOL_SCHEMA_VERSION,
            json!({
                "type": "object",
                "properties": {
                    SUBAGENT_TOOL_TASK_NAME_FIELD: {
                        "type": "string",
                        "pattern": "^[a-z0-9_-]+$",
                        "description": "稳定任务名；仅使用小写字母、数字、连字符或下划线。"
                    },
                    SUBAGENT_TOOL_TASK_FIELD: {
                        "type": "string",
                        "pattern": "\\S",
                        "description": "子代理首轮的完整任务说明。"
                    },
                    SUBAGENT_TOOL_FORK_TURNS_FIELD: {
                        "description": "复制给子代理的最近父会话轮次；省略或 none 表示不复制，all 表示全部，正整数字符串表示最近 N 轮。",
                        "anyOf": [
                            { "type": "string", "enum": ["none", "all"] },
                            { "type": "string", "pattern": "^[1-9][0-9]*$" }
                        ]
                    }
                },
                "required": [SUBAGENT_TOOL_TASK_NAME_FIELD, SUBAGENT_TOOL_TASK_FIELD],
                "additionalProperties": false
            }),
        ),
        // 内部协作能力：spawn_agent 保留在执行注册表，但不向模型暴露；
        // 创建子代理统一由 delegate_to_fast 与内部编排驱动。
        enabled: false,
        concurrency_policy: ToolConcurrencyPolicy::ParallelSafe,
        output_policy: ToolOutputPolicy::BoundedSummary,
        permission_policy: ToolPermissionPolicy::ReadOnly,
    }
}

/// `send_message` 工具定义。
#[must_use]
pub fn send_message_tool_definition() -> ToolDefinition {
    message_tool_definition(
        SEND_MESSAGE_TOOL_ID,
        SEND_MESSAGE_TOOL_NAME,
        "需要补充上下文但不要求子代理立即开始新任务时，向现有子代理发送一条消息。",
    )
}

/// 协作工具目标字段的统一说明：root 与 self 都不可寻址。
const SUBAGENT_TOOL_COLLABORATION_TARGET_DESCRIPTION: &str = "目标子代理的任务路径（canonical `/root/...`）或相对任务名。不可寻址 root（主代理自身），也不可寻址当前代理自己；两者都会被稳定拒绝。";

/// `list_agents` 工具定义。
#[must_use]
pub fn list_agents_tool_definition() -> ToolDefinition {
    tool_definition(
        LIST_AGENTS_TOOL_ID,
        LIST_AGENTS_TOOL_NAME,
        "需要查看当前有哪些子代理及其状态时，列出任务路径、状态和摘要。",
        ToolConcurrencyPolicy::ParallelSafe,
        Vec::new(),
        vec![string_field(
            SUBAGENT_TOOL_PATH_PREFIX_FIELD,
            "可选任务路径前缀，语义与 send_message 的 target 相同（root 与自己都会被拒绝）；省略时列出当前可见的全部子代理。",
        )],
    )
}

/// `delegate_to_fast` 工具定义。
#[must_use]
pub fn delegate_to_fast_tool_definition() -> ToolDefinition {
    tool_definition(
        DELEGATE_TO_FAST_TOOL_ID,
        DELEGATE_TO_FAST_TOOL_NAME,
        DELEGATE_TO_FAST_TOOL_DESCRIPTION,
        ToolConcurrencyPolicy::ParallelSafe,
        vec![SUBAGENT_TOOL_TASK_FIELD],
        vec![
            string_field(
                SUBAGENT_TOOL_TASK_FIELD,
                DELEGATE_TO_FAST_TASK_FIELD_DESCRIPTION,
            ),
            boolean_field(
                SUBAGENT_TOOL_RUN_IN_BACKGROUND_FIELD,
                DELEGATE_TO_FAST_RUN_IN_BACKGROUND_DESCRIPTION,
            ),
        ],
    )
}

fn tool_definition(
    id: ToolId,
    name: &str,
    description: &str,
    concurrency_policy: ToolConcurrencyPolicy,
    required_fields: Vec<&str>,
    fields: Vec<ToolSchemaField>,
) -> ToolDefinition {
    let mut schema = ToolSchema::object(SUBAGENT_TOOL_SCHEMA_VERSION, required_fields, fields);
    if let Some(properties) = schema
        .input_schema
        .get_mut("properties")
        .and_then(serde_json::Value::as_object_mut)
    {
        for property in properties.values_mut() {
            if property.get("type").and_then(serde_json::Value::as_str) == Some("integer") {
                property["minimum"] = json!(0);
            }
        }
    }
    ToolDefinition {
        id,
        namespace: AGENT_TOOL_NAMESPACE.to_owned(),
        name: name.to_owned(),
        description: description.to_owned(),
        schema,
        enabled: true,
        concurrency_policy,
        output_policy: ToolOutputPolicy::BoundedSummary,
        permission_policy: ToolPermissionPolicy::ReadOnly,
    }
}

fn message_tool_definition(id: ToolId, name: &str, description: &str) -> ToolDefinition {
    tool_definition(
        id,
        name,
        description,
        ToolConcurrencyPolicy::Exclusive,
        // intent 与其余内置工具同契约：折叠态只显示这一句话，原始消息留在展开态。
        vec![
            SUBAGENT_TOOL_TARGET_FIELD,
            SUBAGENT_TOOL_MESSAGE_FIELD,
            TOOL_INTENT_FIELD,
        ],
        vec![
            string_field(
                SUBAGENT_TOOL_TARGET_FIELD,
                SUBAGENT_TOOL_COLLABORATION_TARGET_DESCRIPTION,
            ),
            string_field(
                SUBAGENT_TOOL_MESSAGE_FIELD,
                "要追加到目标邮箱的消息或任务。",
            ),
            string_field(TOOL_INTENT_FIELD, TOOL_INTENT_FIELD_DESCRIPTION),
        ],
    )
}

fn string_field(name: &str, description: &str) -> ToolSchemaField {
    ToolSchemaField::new(name, ToolSchemaFieldKind::NonBlankString, description)
}

fn boolean_field(name: &str, description: &str) -> ToolSchemaField {
    ToolSchemaField::new(name, ToolSchemaFieldKind::Boolean, description)
}
