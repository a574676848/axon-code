//! 人格状态只读透传。
//!
//! 桌面端不直连运行态数据库：人格状态一律经网关读取，本模块只做一次薄透传。

use serde_json::Value;

use crate::gateway;

/// 读取已持久化的人格状态摘要。
///
/// # Errors
///
/// 网关不可达、未授权或响应不是合法 JSON 时返回中文诊断。
#[tauri::command]
pub fn persona_summary() -> Result<Value, String> {
    gateway::get_json("/persona/summary")
}

/// 读取形象资产（PNG 的 Base64 视图）。
///
/// 只经网关读取，桌面端不直接访问运行态文件；标识为空时由调用方选择默认形象。
///
/// # Errors
///
/// 网关不可达、未授权或形象尚未放置时返回中文诊断。
#[tauri::command]
pub fn persona_portrait() -> Result<Value, String> {
    gateway::get_json("/persona/portrait/art-halfbody.png")
}
