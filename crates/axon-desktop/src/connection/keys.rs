//! 密钥管理卡片的数据面：经网关 `/api/v1/keys` 列出、签发与吊销 Gateway 多 Key。
//!
//! 三条边界：
//!
//! 1. **身份由网关判**。管理面只接受超管凭据，普通 Key 一律 403；桌面端不自己判断
//!    「当前档案是不是超管」，只按回执分档给文案（403 是权限问题，401 是凭据问题）。
//! 2. **明文只在签发回执里**。列表回执只有标签、两个时间与指纹前 6 位，本模块不回传、
//!    也不缓存明文；签发回执照原样交给界面，显示一次即丢。
//! 3. **不落后端一份模型**。字段名与网关逐字对齐（`createdAtMs` / `lastUsedAtMs`），
//!    这里只做反序列化与错误分档，不重新命名。

use serde::{Deserialize, Serialize};

use crate::gateway;

/// 管理面路径；网关客户端负责补 `/api/v1` 前缀。
const KEYS_PATH: &str = "/keys";
/// 403 的中文说明。
///
/// 普通 Key 的凭据本身有效，缺的是权限级别，因此不能沿用网关那套「凭据缺失或不匹配」
/// 的说法：那句话会让人去改凭据，而真正要做的是换成全局 token。
const FORBIDDEN_MESSAGE: &str = "需要超管凭据：密钥管理只接受全局 token，当前档案用的是普通 Key";

/// 一把 Key 的展示形态；不含明文与摘要。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KeyView {
    /// Key 标识。
    pub id: String,
    /// Key 标签。
    pub label: String,
    /// 创建时间（毫秒）。
    pub created_at_ms: u64,
    /// 最后一次用于鉴权的时间（毫秒）；从未使用为 `None`。
    pub last_used_at_ms: Option<u64>,
    /// 指纹前 6 位十六进制字符。
    pub fingerprint: String,
}

/// 签发回执：明文只在这一份回执里出现一次。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IssuedKeyView {
    /// Key 标识。
    pub id: String,
    /// Key 标签。
    pub label: String,
    /// 明文；离开这份回执后无法再查看。
    pub plaintext: String,
    /// 创建时间（毫秒）。
    pub created_at_ms: u64,
}

/// 列表载荷。
#[derive(Debug, Deserialize)]
struct KeyListPayload {
    /// Key 清单。
    keys: Vec<KeyView>,
}

/// 列出全部未吊销的 Key。
///
/// # Errors
///
/// 网关不可达、非 2xx 回执（403 归为「需要超管凭据」），或回执形状不符合契约。
#[tauri::command]
pub fn connection_keys_list() -> Result<Vec<KeyView>, String> {
    let (status, body) = gateway::get_raw(KEYS_PATH)?;
    let value = decode(status, &body)?;
    serde_json::from_value::<KeyListPayload>(value)
        .map(|payload| payload.keys)
        .map_err(|error| format!("密钥列表格式无法识别：{error}"))
}

/// 签发一把新 Key：明文只在回执里出现一次。
///
/// # Errors
///
/// 标签为空白、网关不可达、非 2xx 回执（403 归为「需要超管凭据」），或回执形状不符。
#[tauri::command]
pub fn connection_keys_create(label: String) -> Result<IssuedKeyView, String> {
    let label = label.trim();
    if label.is_empty() {
        return Err("请填写 Key 的用途标签：标签用于在审计里认出这把 Key".to_owned());
    }
    let (status, body) = gateway::post_raw(KEYS_PATH, &serde_json::json!({ "label": label }))?;
    let value = decode(status, &body)?;
    serde_json::from_value(value).map_err(|error| format!("签发回执格式无法识别：{error}"))
}

/// 吊销一把 Key：对下一次鉴权立即生效。
///
/// # Errors
///
/// 标识为空或含非法字符、网关不可达、非 2xx 回执（403 归为「需要超管凭据」），
/// 或回执没有确认吊销。
#[tauri::command]
pub fn connection_keys_revoke(id: String) -> Result<(), String> {
    let id = id.trim();
    if id.is_empty() {
        return Err("Key 标识不能为空".to_owned());
    }
    // 路径参数只放行最保守的字符集：标识由后端生成，形如 UUID，只含字母、数字与连字符。
    // 放行其它字符等于让调用方往路径里塞 `..` 或查询串。
    if !id.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-') {
        return Err(format!("Key 标识非法：{id}"));
    }
    let (status, body) = gateway::delete_raw(&format!("{KEYS_PATH}/{id}"))?;
    let value = decode(status, &body)?;
    match value.get("revoked").and_then(serde_json::Value::as_bool) {
        Some(true) => Ok(()),
        _ => Err("网关未确认吊销结果".to_owned()),
    }
}

/// 按状态码分档解码：403 单独归为权限问题，其余交给网关的统一口径。
fn decode(status: u16, body: &str) -> Result<serde_json::Value, String> {
    if status == 403 {
        return Err(FORBIDDEN_MESSAGE.to_owned());
    }
    gateway::decode_json(status, body)
}

#[cfg(test)]
mod tests {
    use super::{FORBIDDEN_MESSAGE, decode};

    #[test]
    fn forbidden_is_reported_as_missing_super_admin_not_as_a_bad_credential() {
        let message = decode(403, r#"{"error":"forbidden"}"#).expect_err("403 必须失败");
        assert_eq!(message, FORBIDDEN_MESSAGE);
        assert!(message.contains("超管"), "{message}");
    }

    #[test]
    fn unauthorized_keeps_the_credential_advice() {
        // 401 是凭据真的不对（过期、写错），与 403 的修复动作不同，不能并成一句话。
        let message = decode(401, r#"{"error":"unauthorized"}"#).expect_err("401 必须失败");
        assert!(message.contains("凭据"), "{message}");
        assert!(!message.contains("超管"), "{message}");
    }
}
