//! 工具入参解析与时间戳：notes / history 两个命名空间共用。
//!
//! 单独成文件是为了不让 `orchestrator_notes_history.rs` 继续长到行数上限，而不是
//! 为了复用；但这两件事本来就该只有一份实现：两个命名空间的执行器都要先过同一套
//! schema 校验，复制一份就等于「哪些字段允许显式传 null」的规则各写一遍。

use axon_agent_runtime::ProviderToolCall;

use super::now_ms;

/// 去掉 `builtin.` 前缀后的裸工具名。
pub(super) fn bare_tool_name(tool_name: &str) -> String {
    tool_name
        .strip_prefix("builtin.")
        .unwrap_or(tool_name)
        .to_owned()
}

/// 按 schema 解析工具入参，失败时把前两行原因与补参指引一起给出。
pub(super) fn parse_input<T>(tool_call: &ProviderToolCall, guidance: &str) -> Result<T, String>
where
    T: for<'de> serde::Deserialize<'de>,
{
    let Some(value) = tool_call.input.as_json() else {
        return Err(format!("工具入参必须是 JSON 对象。{guidance}"));
    };
    reject_explicit_nulls(value, guidance)?;
    serde_json::from_value(value.clone()).map_err(|error| {
        format!(
            "工具入参不符合 schema：{}。{guidance}",
            error
                .to_string()
                .lines()
                .take(2)
                .collect::<Vec<_>>()
                .join(" ")
        )
    })
}

/// 拒绝非 nullable 字段显式传 null；nullable 字段的原样保留。
pub(super) fn reject_explicit_nulls(
    value: &serde_json::Value,
    guidance: &str,
) -> Result<(), String> {
    let Some(fields) = value.as_object() else {
        return Ok(());
    };
    if fields.iter().any(|(field, value)| {
        value.is_null()
            && !matches!(
                field.as_str(),
                "prefix"
                    | "start_line"
                    | "stop_line"
                    | "path_prefix"
                    | "role"
                    | "tool_name"
                    | "tool_namespace"
                    | "window_id"
            )
    }) {
        return Err(format!("schema 不允许非 nullable 字段传 null。{guidance}"));
    }
    Ok(())
}

/// 当前 Unix 毫秒；超出 i64 的时钟直接夹到上限。
pub(super) fn unix_now_ms() -> i64 {
    i64::try_from(now_ms()).unwrap_or(i64::MAX)
}
