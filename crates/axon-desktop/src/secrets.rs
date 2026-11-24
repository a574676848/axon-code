//! 密钥的可视化管理：经网关读写后台服务的密钥文件。
//!
//! 桌面端不再碰本机 `.env`：状态查询走 `GET /api/v1/secrets/status`，写入与清除
//! 走 `PUT /api/v1/secrets`（`value` 为空表示清除）。回执只有「变量名 + 是否已
//! 配置」两位状态，密钥明文既不回传界面，也不会停在桌面端进程内。
//!
//! 安全边界：明文只在上行请求体里出现一次；错误文案与日志一律不含明文，本模块
//! 也不写任何日志。对外文案在送出前再过一道脱敏，把明文出现过的痕迹抹掉。

use serde_json::Value;

use crate::config_gateway::{self, SECRETS_PATH, SECRETS_STATUS_PATH};

/// 明文若出现在对外文案里，用它替换。
const REDACTED: &str = "******";

/// 校验环境变量名：只允许字母、数字与下划线，且不以数字开头。
fn validate_key(key: &str) -> Result<&str, String> {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        return Err("环境变量名不能为空".to_string());
    }
    let mut chars = trimmed.chars();
    let first_ok = chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_');
    if !first_ok || !chars.all(|item| item.is_ascii_alphanumeric() || item == '_') {
        return Err("环境变量名只能包含字母、数字与下划线，且不能以数字开头".to_string());
    }
    Ok(trimmed)
}

/// 状态查询路径。
///
/// 环境变量名只含字母、数字与下划线，没有需要百分号编码的字符，因此查询串按字面
/// 拼接，不引入编码层。
fn status_path(name: &str) -> String {
    format!("{SECRETS_STATUS_PATH}?name={name}")
}

/// 写入请求体：明文只应出现在这一处。
fn write_payload(name: &str, value: &str) -> Value {
    serde_json::json!({ "name": name, "value": value })
}

/// 取回执里的配置状态位。
fn configured_of(receipt: &Value) -> Result<bool, String> {
    config_gateway::bool_field(receipt, "configured")
}

/// 把明文从对外文案里抹掉。
///
/// 后台回执理论上不含密钥值，但这一位是安全红线，不能依赖上游：只要明文出现在
/// 将要上屏或写日志的字符串里，一律替换成占位符。
fn redact(message: String, secret: &str) -> String {
    if secret.is_empty() {
        return message;
    }
    message.replace(secret, REDACTED)
}

/// Tauri IPC 命令：查询某个环境变量是否已经配置密钥。
///
/// # Errors
///
/// 变量名不合法、网关不可达、凭据不匹配或后台服务版本过旧。
#[tauri::command(async)]
pub fn secret_status_of(key: String) -> Result<bool, String> {
    let key = validate_key(&key)?.to_string();
    let receipt = config_gateway::get_json(&status_path(&key))?;
    configured_of(&receipt)
}

/// Tauri IPC 命令：写入或更新一个密钥。
///
/// # Errors
///
/// 变量名不合法、密钥为空、网关不可达、凭据不匹配或后台服务版本过旧。
#[tauri::command(async)]
pub fn secret_set(key: String, value: String) -> Result<(), String> {
    let key = validate_key(&key)?.to_string();
    if value.trim().is_empty() {
        return Err("密钥值不能为空".to_string());
    }
    let receipt = config_gateway::put_json(SECRETS_PATH, &write_payload(&key, &value))
        .map_err(|message| redact(message, &value))?;
    // 回执必须带回配置状态位：缺了它说明这一份没有按契约落盘，不能当作写成功。
    configured_of(&receipt).map(|_| ())
}

/// Tauri IPC 命令：清除一个密钥。
///
/// 清除是幂等的：本来就不存在时后台同样回「未配置」，因此这里不报错。
///
/// # Errors
///
/// 变量名不合法、网关不可达、凭据不匹配或后台服务版本过旧。
#[tauri::command(async)]
pub fn secret_clear(key: String) -> Result<(), String> {
    let key = validate_key(&key)?.to_string();
    let receipt = config_gateway::put_json(SECRETS_PATH, &write_payload(&key, ""))?;
    configured_of(&receipt).map(|_| ())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{REDACTED, redact, status_path, validate_key, write_payload};

    /// 一个一眼能认出来的哨兵串：只要它出现在错误文案里就说明明文泄漏了。
    const SENTINEL: &str = "sk-sentinel-plaintext-never-on-screen-0123456789";

    #[test]
    fn env_key_validation_rejects_unsafe_names() {
        assert_eq!(
            validate_key(" OPENAI_API_KEY ").expect("合法名"),
            "OPENAI_API_KEY"
        );
        assert_eq!(validate_key("_KEY1").expect("合法名"), "_KEY1");
        assert!(validate_key("").is_err());
        assert!(validate_key("1KEY").is_err());
        assert!(validate_key("BAD-KEY").is_err());
        assert!(validate_key("BAD KEY").is_err());
        assert!(validate_key("坏键").is_err());
    }

    #[test]
    fn status_and_write_shapes_match_the_gateway_contract() {
        assert_eq!(
            status_path("OPENAI_API_KEY"),
            "/api/v1/secrets/status?name=OPENAI_API_KEY"
        );
        assert_eq!(
            write_payload("OPENAI_API_KEY", SENTINEL),
            json!({ "name": "OPENAI_API_KEY", "value": SENTINEL }),
            "写入请求体只有 name 与 value 两键"
        );
    }

    #[test]
    fn plaintext_appears_in_the_request_body_only() {
        // 明文只允许出现在上行请求体里，且只出现一次。
        let payload = serde_json::to_string(&write_payload("OPENAI_API_KEY", SENTINEL))
            .expect("请求体必须可序列化");
        assert_eq!(
            payload.matches(SENTINEL).count(),
            1,
            "明文在请求体里只应出现一次：{payload}"
        );

        // 失败文案来自状态码与服务端原因，绝不带请求体。
        for (status, body) in [
            (404u16, ""),
            (401, r#"{"error":"unauthorized"}"#),
            (
                500,
                r#"{"error":"write_failed","message":"写入密钥文件失败"}"#,
            ),
            (413, ""),
        ] {
            let message = crate::config_gateway::failure_message(status, body);
            assert!(
                !message.contains(SENTINEL),
                "错误文案不得含明文（HTTP {status}）：{message}"
            );
        }
    }

    #[test]
    fn redaction_removes_the_plaintext_from_any_outward_message() {
        // 万一上游把明文回显进原因里，这一层负责把它抹掉。
        let leaked = format!("写入密钥失败：服务端回显 {SENTINEL}");
        let safe = redact(leaked, SENTINEL);
        assert!(!safe.contains(SENTINEL), "{safe}");
        assert!(safe.contains(REDACTED), "{safe}");
        // 没有明文可抹时原样透出，避免把有用信息也一起盖掉。
        assert_eq!(redact("写入密钥失败".to_string(), SENTINEL), "写入密钥失败");
    }
}
