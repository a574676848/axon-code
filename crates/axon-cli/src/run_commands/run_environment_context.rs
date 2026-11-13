//! 本轮动态环境上下文的组装与字段提取。
//!
//! 会话车道标识与全局通知会话只作为事实字段注入，不在提示词里写死任何命令用法：
//! 用法由技能与文档解释，命令面变化时这里不需要跟着改。
//!
//! 子代理模型目录与思考强度档位同样只作为事实注入：把它们放进环境上下文，
//! 模型才可能在 `spawn_agent` 上显式选型；清单与工具实际接受的值域由同一个
//! 来源生成，避免通告一套、执行另一套。

use chrono::Local;
use std::time::{SystemTime, UNIX_EPOCH};

/// 组装本轮动态环境上下文。
///
/// 会话车道标识与全局通知会话只作为**事实字段**注入，不在提示词里写死任何命令用法：
/// 用法由技能与文档解释，命令面变化时这里不需要跟着改。
///
/// `delegatable_models` 为空表示本轮不注入模型目录：子代理身份不能创建子代理，
/// 因此不需要这份选型事实。
pub(super) fn cli_environment_context(
    transport_context: Option<&serde_json::Value>,
    notification_chat_id: Option<&str>,
    delegatable_models: &[String],
) -> axon_agent_runtime::EnvironmentContext {
    let field = |key: &str| {
        transport_context
            .and_then(|value| value.get(key))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let now = SystemTime::now();
    let utc_now_ms = now.duration_since(UNIX_EPOCH).map_or(0, |duration| {
        duration.as_millis().try_into().unwrap_or(u64::MAX)
    });
    let local_now: chrono::DateTime<Local> = now.into();
    let timezone_name = local_now.format("%Z").to_string();
    axon_agent_runtime::EnvironmentContext {
        utc_now_ms,
        local_time: Some(local_now.format("%Y-%m-%d %H:%M:%S").to_string()),
        timezone_name: (!timezone_name.trim().is_empty()).then_some(timezone_name),
        local_offset_minutes: Some(local_now.offset().local_minus_utc() / 60),
        platform: std::env::consts::OS.to_owned(),
        architecture: std::env::consts::ARCH.to_owned(),
        shell: Some(axon_agent_runtime::default_shell_program()),
        terminal: first_environment_value(&["TERM_PROGRAM", "TERM"]),
        session_channel: field("channel"),
        session_destination: field("destination"),
        session_workspace: field("workspaceId"),
        notification_chat_id: notification_chat_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned),
        delegatable_models: delegatable_models.to_vec(),
        reasoning_effort_tiers: axon_config::REASONING_EFFORT_VALUES
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
    }
}

fn first_environment_value(names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        std::env::var(name)
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    })
}
