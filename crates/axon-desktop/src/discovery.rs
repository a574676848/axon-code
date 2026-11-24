//! 上游供应商模型列表发现。
//!
//! 探测本身在后台服务里执行，桌面端只做一次转发：后台向供应商的模型列表端点
//! 发一次只读 GET，把返回的候选模型原样交给界面勾选。候选永远不会直接写入
//! 配置，写盘仍由用户确认后经网关的配置写入完成。
//!
//! 密钥边界：本模块只接受环境变量名（`apiKeyEnv`），密钥由后台在自己的进程内
//! 解析，绝不穿越请求体，也不会出现在任何返回值或错误信息里。

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config_gateway::{self, PROVIDERS_DISCOVER_PATH};

/// 探测到的候选模型。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredModelDto {
    /// 上游模型标识。
    pub id: String,
    /// 可读名称，上游未提供时为空。
    pub name: Option<String>,
    /// 上游声明的上下文窗口，未声明时为空。
    pub context_window: Option<u64>,
    /// 上游声明的最大输出 token，未声明时为空。
    pub max_output_tokens: Option<u64>,
}

/// 探测结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoverModelsResponse {
    /// 实际请求的端点地址。
    pub endpoint: String,
    /// 候选模型列表，按标识升序去重。
    pub models: Vec<DiscoveredModelDto>,
}

/// 探测请求体：字段名是命令参数的 camelCase 形式，与后台适配器逐字一致。
///
/// 缺省字段一律给 `null`：后台按 `Option` 解析，`null` 与缺省同义，
/// 因此新建供应商（档案还没落盘）时地址与协议也能照常透传。
fn discover_payload(
    provider: Option<String>,
    base_url: Option<String>,
    protocol: Option<String>,
    api_key_env: Option<String>,
) -> Value {
    json!({
        "provider": provider,
        "baseUrl": base_url,
        "protocol": protocol,
        "apiKeyEnv": api_key_env,
    })
}

/// Tauri IPC 命令：获取供应商可用的模型列表。
///
/// 传入已登记供应商名称即可；也可以直接给出地址与协议，用于供应商尚未保存时的
/// 试探测。档案缺项由后台补齐，桌面端不再自己读配置。
///
/// # Errors
///
/// 网关不可达、凭据不匹配、后台服务版本过旧、供应商未登记且未给地址，
/// 或上游探测失败。
#[tauri::command(async)]
pub fn discover_models(
    provider: Option<String>,
    base_url: Option<String>,
    protocol: Option<String>,
    api_key_env: Option<String>,
) -> Result<DiscoverModelsResponse, String> {
    let payload = discover_payload(provider, base_url, protocol, api_key_env);
    let receipt = config_gateway::post_json(PROVIDERS_DISCOVER_PATH, &payload)?;
    config_gateway::decode(receipt)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::discover_payload;

    #[test]
    fn request_body_uses_the_gateway_camel_case_field_names() {
        let payload = discover_payload(
            Some("demo-provider".to_string()),
            Some("https://example.com/v1".to_string()),
            Some("openai".to_string()),
            Some("DEMO_API_KEY".to_string()),
        );
        assert_eq!(
            payload,
            json!({
                "provider": "demo-provider",
                "baseUrl": "https://example.com/v1",
                "protocol": "openai",
                "apiKeyEnv": "DEMO_API_KEY",
            }),
            "请求体字段名必须与后台适配器逐字一致"
        );
        assert!(payload.get("api_key_env").is_none(), "不得混用蛇形字段名");
    }

    #[test]
    fn missing_fields_are_sent_as_null_for_a_new_provider() {
        // 新建供应商时档案还不存在，界面可能只填了地址：缺项给 null，
        // 后台按 Option 解析成缺省值，由后台去补档案里的地址与密钥变量名。
        let payload = discover_payload(Some("brand-new".to_string()), None, None, None);
        assert_eq!(payload["provider"], "brand-new");
        assert!(payload["baseUrl"].is_null());
        assert!(payload["protocol"].is_null());
        assert!(payload["apiKeyEnv"].is_null());
    }
}
