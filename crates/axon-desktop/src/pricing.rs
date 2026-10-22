//! 模型定价：桌面端读写价格规则的薄封装。
//!
//! 与用量统计同源，一律经网关 `/api/v1/observability/pricing/*`：
//! 规则落在 `usage/usage.sqlite3` 的 `pricing_rules` 表，桌面端不直连数据库，
//! 也不在本地重算计费口径。
//!
//! 网关的保存接口只接受「新建」语义（规则 ID 由服务端按时间生成），
//! 因此「编辑」在界面上是**先建后删**两步，由调用方按顺序发起：新建失败时
//! 旧规则原样保留，中间不存在「这条模型没有价格」的窗口。

use serde_json::Value;

use crate::gateway;
use crate::usage::observability_endpoint;

/// 列出全部价格规则，按服务端的四级全序返回。
#[tauri::command]
pub fn pricing_rules() -> Result<Value, String> {
    gateway::get_json(&observability_endpoint("/pricing/rules", &[]))
}

/// 新建一条价格规则；返回服务端生成的规则 ID。
#[tauri::command]
pub fn pricing_save_rule(payload: Value) -> Result<Value, String> {
    gateway::post_json(&observability_endpoint("/pricing/rules", &[]), &payload)
}

/// 读取每日自动同步任务的状态。
#[tauri::command]
pub fn pricing_auto_sync() -> Result<Value, String> {
    gateway::get_json(&observability_endpoint("/pricing/auto-sync", &[]))
}

/// 打开或关闭每日自动同步；打开时由后端创建或启用系统 Cron 任务。
#[tauri::command]
pub fn pricing_set_auto_sync(enabled: bool) -> Result<Value, String> {
    gateway::post_json(
        &observability_endpoint("/pricing/auto-sync", &[]),
        &serde_json::json!({ "enabled": enabled }),
    )
}

/// 按 ID 删除一条价格规则。
#[tauri::command]
pub fn pricing_delete_rule(rule_id: String) -> Result<Value, String> {
    gateway::delete_json(&observability_endpoint(
        &format!("/pricing/rules/{}", encode_segment(&rule_id)),
        &[],
    ))
}

/// 从 models.dev 同步费率；服务端有 60 秒手动同步防抖。
#[tauri::command]
pub fn pricing_sync() -> Result<Value, String> {
    gateway::post_json(&observability_endpoint("/pricing/sync", &[]), &Value::Null)
}

/// 按当前规则重算已落库的历史成本。
#[tauri::command]
pub fn pricing_recalculate() -> Result<Value, String> {
    gateway::post_json(
        &observability_endpoint("/pricing/recalculate", &[]),
        &Value::Null,
    )
}

/// 把一段文本编码成可以安全放进路径的片段。
///
/// 规则 ID 由服务端生成（形如 `pr_1792569904652`），但删除入口是路径参数，
/// 这里仍按通用规则转义：调用方传入什么都不会拼出一条越界的路径。
fn encode_segment(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_id_is_escaped_before_entering_the_path() {
        assert_eq!(encode_segment("pr_1792569904652"), "pr_1792569904652");
        assert_eq!(encode_segment("a/b?c"), "a%2Fb%3Fc");
    }
}
