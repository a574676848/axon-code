//! 子代理工具参数解析与名称规范化。

use axon_domain::{AxonError, AxonResult, ToolCallInput};

use crate::agent_control_runtime::AGENT_CONTROL_REASON_INVALID_PATH;
use crate::agent_subagent_fork_context::SubagentForkTurns;
use crate::agent_tools::AGENT_TOOL_NAMESPACE;
use crate::builtin_subagent_tool_spec::{
    DELEGATE_TO_FAST_TOOL_NAME, LIST_AGENTS_TOOL_NAME, SEND_MESSAGE_TOOL_NAME,
    SPAWN_AGENT_TOOL_NAME, SUBAGENT_TOOL_MESSAGE_FIELD, SUBAGENT_TOOL_PATH_PREFIX_FIELD,
    SUBAGENT_TOOL_RUN_IN_BACKGROUND_FIELD, SUBAGENT_TOOL_TARGET_FIELD, SUBAGENT_TOOL_TASK_FIELD,
};

use super::SUBAGENT_TOOL_REASON_UNKNOWN_FIELD;

pub(super) fn parse_arguments(input: &ToolCallInput) -> AxonResult<serde_json::Value> {
    input
        .as_json()
        .cloned()
        .ok_or_else(|| AxonError::tool_invalid_arguments("子代理工具参数必须是 JSON object"))
}

pub(super) fn validate_allowed_fields(
    arguments: &serde_json::Value,
    allowed: &[&str],
) -> AxonResult<()> {
    let Some(object) = arguments.as_object() else {
        return Err(AxonError::tool_invalid_arguments(
            "子代理工具参数必须是 JSON object",
        ));
    };
    if object.keys().all(|key| allowed.contains(&key.as_str())) {
        return Ok(());
    }
    Err(AxonError::tool_invalid_arguments(
        SUBAGENT_TOOL_REASON_UNKNOWN_FIELD,
    ))
}

pub(super) fn required_string(arguments: &serde_json::Value, field: &str) -> AxonResult<String> {
    let value = optional_string(arguments, field).ok_or(AxonError::tool_invalid_arguments(
        "子代理工具参数缺少必填字段",
    ))?;
    if value.trim().is_empty() {
        return Err(AxonError::tool_invalid_arguments(
            "子代理工具字符串参数不能为空",
        ));
    }
    Ok(value)
}

pub(super) fn required_target_reference(arguments: &serde_json::Value) -> AxonResult<String> {
    optional_string(arguments, SUBAGENT_TOOL_TARGET_FIELD).ok_or(AxonError::tool_invalid_arguments(
        "子代理工具参数缺少 target 字段",
    ))
}

pub(super) fn required_task_name(arguments: &serde_json::Value) -> AxonResult<String> {
    let value = arguments
        .get(crate::builtin_subagent_tool_spec::SUBAGENT_TOOL_TASK_NAME_FIELD)
        .and_then(serde_json::Value::as_str)
        .ok_or(AxonError::tool_invalid_arguments(
            "子代理工具参数缺少必填 task_name 字段",
        ))?;
    if value.is_empty()
        || !value
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_')
    {
        return Err(AxonError::tool_invalid_arguments(
            "子代理 task_name 必须仅包含小写字母、数字、连字符或下划线",
        ));
    }
    Ok(value.to_owned())
}

pub(super) fn optional_string(arguments: &serde_json::Value, field: &str) -> Option<String> {
    arguments
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

/// `send_message` 的解析结果。
pub(super) struct SendMessageArgs {
    /// 目标代理引用（相对或 canonical 路径）。
    pub(super) target: String,
    /// 待投递的消息正文。
    pub(super) message: String,
}

/// 解析 `send_message` 参数。
///
/// 两个协作工具（`send_message`、`list_agents`）的参数解析都收敛到本文件，
/// 分派层不再各自内联校验，因此同一非法输入在两个工具上得到同构错误。
pub(super) fn parse_send_message_args(
    arguments: &serde_json::Value,
) -> AxonResult<SendMessageArgs> {
    validate_allowed_fields(
        arguments,
        &[
            SUBAGENT_TOOL_TARGET_FIELD,
            SUBAGENT_TOOL_MESSAGE_FIELD,
            crate::TOOL_INTENT_FIELD,
        ],
    )?;
    Ok(SendMessageArgs {
        target: required_target_reference(arguments)?,
        message: required_string(arguments, SUBAGENT_TOOL_MESSAGE_FIELD)?,
    })
}

/// `list_agents` 的解析结果。
pub(super) struct ListAgentsArgs {
    /// 可选路径前缀；`None` 表示不限定目标、列出当前可见的整棵代理树。
    pub(super) path_prefix: Option<String>,
}

/// 解析 `list_agents` 参数。
pub(super) fn parse_list_agents_args(arguments: &serde_json::Value) -> AxonResult<ListAgentsArgs> {
    validate_allowed_fields(arguments, &[SUBAGENT_TOOL_PATH_PREFIX_FIELD])?;
    Ok(ListAgentsArgs {
        path_prefix: optional_string(arguments, SUBAGENT_TOOL_PATH_PREFIX_FIELD),
    })
}

/// `delegate_to_fast` 的解析结果。
pub(super) struct DelegateArgs {
    /// 自包含的委托任务。
    pub(super) task: String,
    /// 是否后台执行；缺省为后台（合同 §5.1.7 的默认双模式）。
    pub(super) run_in_background: bool,
}

/// 解析 `delegate_to_fast` 参数。
///
/// `run_in_background` 省略即为后台：默认后台是合同 §5.1.7 的语义选择，
/// 同步等待是模型按「下一步动作是否依赖结果」显式选择的例外。
pub(super) fn parse_delegate_args(arguments: &serde_json::Value) -> AxonResult<DelegateArgs> {
    validate_allowed_fields(
        arguments,
        &[
            SUBAGENT_TOOL_TASK_FIELD,
            SUBAGENT_TOOL_RUN_IN_BACKGROUND_FIELD,
        ],
    )?;
    if let Some(value) = arguments.get(SUBAGENT_TOOL_RUN_IN_BACKGROUND_FIELD)
        && !value.is_boolean()
    {
        return Err(AxonError::tool_invalid_arguments(
            "子代理 run_in_background 必须是布尔值",
        ));
    }
    Ok(DelegateArgs {
        task: required_string(arguments, SUBAGENT_TOOL_TASK_FIELD)?,
        run_in_background: arguments
            .get(SUBAGENT_TOOL_RUN_IN_BACKGROUND_FIELD)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true),
    })
}

pub(super) fn validate_fork_turns(value: Option<&str>) -> AxonResult<()> {
    SubagentForkTurns::parse(value)
        .map(|_| ())
        .map_err(|_| AxonError::tool_invalid_arguments(AGENT_CONTROL_REASON_INVALID_PATH))
}

pub(super) fn subagent_tool_name(name: &str) -> Option<&'static str> {
    let canonical = canonical_subagent_tool_name(name);
    match canonical {
        SPAWN_AGENT_TOOL_NAME => Some(SPAWN_AGENT_TOOL_NAME),
        SEND_MESSAGE_TOOL_NAME => Some(SEND_MESSAGE_TOOL_NAME),
        LIST_AGENTS_TOOL_NAME => Some(LIST_AGENTS_TOOL_NAME),
        DELEGATE_TO_FAST_TOOL_NAME => Some(DELEGATE_TO_FAST_TOOL_NAME),
        _ => None,
    }
}

fn canonical_subagent_tool_name(name: &str) -> &str {
    name.strip_prefix(&format!("{AGENT_TOOL_NAMESPACE}."))
        .unwrap_or(name)
}
