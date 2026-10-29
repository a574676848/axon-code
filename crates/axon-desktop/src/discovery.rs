//! 上游供应商模型列表发现。
//!
//! 只做一件事：向供应商的模型列表端点发一次只读 GET，把返回的候选模型
//! 原样交给界面勾选。候选永远不会直接写入配置，写盘仍由用户确认后经
//! [`crate::catalog`] 的原子路径完成。
//!
//! 密钥边界：本模块只接受环境变量名，密钥由
//! [`axon_config::provider_credential`] 在进程内解析，绝不穿过 IPC，
//! 也不会出现在任何返回值或错误信息里。

use std::time::Duration;

use axon_config::{ConfigLoader, provider_credential};
use serde::{Deserialize, Serialize};

/// 单次探测的超时时间，避免界面被不可达端点长时间挂住。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// 模型列表路径段。
const MODELS_PATH: &str = "models";

/// 协议常量：Anthropic 使用独立的鉴权头与版本头。
const PROTOCOL_ANTHROPIC: &str = "anthropic";

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

/// 拼接模型列表端点：基地址末尾的斜杠会被忽略。
fn models_endpoint(base_url: &str) -> String {
    let trimmed = base_url.trim().trim_end_matches('/');
    format!("{trimmed}/{MODELS_PATH}")
}

/// 从任意 JSON 形态中取出候选模型数组。
fn candidate_array(payload: &serde_json::Value) -> Option<&Vec<serde_json::Value>> {
    if let Some(array) = payload.as_array() {
        return Some(array);
    }
    for key in ["data", "models", "result"] {
        if let Some(array) = payload.get(key).and_then(serde_json::Value::as_array) {
            return Some(array);
        }
        if let Some(array) = payload
            .get(key)
            .and_then(|nested| nested.get("models"))
            .and_then(serde_json::Value::as_array)
        {
            return Some(array);
        }
    }
    None
}

/// 从条目中读取第一个可用的字符串字段。
fn first_string(entry: &serde_json::Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        entry
            .get(*key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
    })
}

/// 从条目中读取第一个可用的正整数或数字字符串字段。
fn first_number(entry: &serde_json::Value, keys: &[&str]) -> Option<u64> {
    keys.iter().find_map(|key| {
        let value = entry.get(*key)?;
        value.as_u64().or_else(|| {
            value
                .as_str()
                .and_then(|text| text.trim().parse::<u64>().ok())
        })
    })
}

/// 把响应体解析为候选模型列表。
fn parse_candidates(payload: &serde_json::Value) -> Vec<DiscoveredModelDto> {
    let Some(array) = candidate_array(payload) else {
        return Vec::new();
    };

    let mut models: Vec<DiscoveredModelDto> = array
        .iter()
        .filter_map(|entry| {
            let id = first_string(entry, &["id", "name", "model", "model_name"])?;
            Some(DiscoveredModelDto {
                id,
                name: first_string(entry, &["display_name", "displayName", "title"]),
                context_window: first_number(
                    entry,
                    &[
                        "context_window",
                        "contextWindow",
                        "context_length",
                        "max_context_length",
                        "inputTokenLimit",
                    ],
                ),
                max_output_tokens: first_number(
                    entry,
                    &[
                        "max_output_tokens",
                        "maxTokens",
                        "max_output",
                        "outputTokenLimit",
                    ],
                ),
            })
        })
        .collect();

    models.sort_by(|left, right| left.id.cmp(&right.id));
    models.dedup_by(|left, right| left.id == right.id);
    models
}

/// 向端点发起一次只读探测。
pub fn execute_discover_models(
    base_url: &str,
    protocol: &str,
    api_key_env: Option<&str>,
) -> Result<DiscoverModelsResponse, String> {
    let trimmed_base = base_url.trim();
    if trimmed_base.is_empty() {
        return Err("供应商未配置接口地址，无法获取模型列表".to_string());
    }
    let endpoint = models_endpoint(trimmed_base);
    if !endpoint.starts_with("http://") && !endpoint.starts_with("https://") {
        return Err("接口地址必须以 http:// 或 https:// 开头".to_string());
    }

    let credential = api_key_env
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .and_then(provider_credential);

    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(REQUEST_TIMEOUT))
        .build()
        .new_agent();

    let mut request = agent.get(&endpoint).header("accept", "application/json");
    if let Some(key) = credential.as_deref() {
        if protocol.trim() == PROTOCOL_ANTHROPIC {
            request = request
                .header("x-api-key", key)
                .header("anthropic-version", "2023-06-01");
        } else {
            request = request.header("authorization", &format!("Bearer {key}"));
        }
    }

    let mut response = request
        .call()
        .map_err(|error| format!("请求模型列表失败：{error}"))?;

    if response.status().as_u16() >= 400 {
        return Err(format!(
            "上游返回状态码 {}，请检查接口地址与密钥配置",
            response.status().as_u16()
        ));
    }

    let body = response
        .body_mut()
        .read_to_string()
        .map_err(|error| format!("读取模型列表响应失败：{error}"))?;

    let payload: serde_json::Value =
        serde_json::from_str(&body).map_err(|_| "上游返回的内容不是合法 JSON".to_string())?;

    let models = parse_candidates(&payload);
    if models.is_empty() {
        return Err("上游返回的模型列表为空".to_string());
    }

    Ok(DiscoverModelsResponse { endpoint, models })
}

/// 从已登记供应商解析出探测所需的连接信息。
fn resolve_provider_target(provider: &str) -> Result<(String, String, Option<String>), String> {
    let path = ConfigLoader::default_user_config_path()
        .map_err(|error| format!("获取用户配置路径失败：{error:?}"))?;
    let loaded =
        ConfigLoader::load_from_path(&path).map_err(|error| format!("加载配置失败：{error:?}"))?;
    let entry = loaded
        .config
        .providers
        .iter()
        .find(|candidate| candidate.name == provider)
        .ok_or_else(|| format!("未找到供应商：{provider}"))?;
    Ok((
        entry.base_url.clone(),
        entry.protocol.clone(),
        entry.api_key_env.clone(),
    ))
}

/// Tauri IPC 命令：获取供应商可用的模型列表。
///
/// 传入已登记供应商名称即可；也可以直接给出地址与协议，
/// 用于供应商尚未保存时的试探测。
#[tauri::command(async)]
pub fn discover_models(
    provider: Option<String>,
    base_url: Option<String>,
    protocol: Option<String>,
    api_key_env: Option<String>,
) -> Result<DiscoverModelsResponse, String> {
    let mut resolved_base = base_url.unwrap_or_default();
    let mut resolved_protocol = protocol.unwrap_or_default();
    let mut resolved_key = api_key_env;

    // 与配置读取一致：先加载相邻 .env，否则已配置的密钥会被当成缺失。
    if let Ok(path) = ConfigLoader::default_user_config_path()
        && let Some(parent) = path.parent()
    {
        let _ = axon_config::load_provider_credentials_from_env_file(&parent.join(".env"));
    }

    if let Some(name) = provider
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        /*
         * 已登记档案只用来补齐缺项：新建供应商时名字还没写进配置，
         * 界面已经把地址与协议填好了，此时不该因为「查不到这个档案」直接失败。
         * 只有既查不到档案、界面又没给地址时，才是真的无法探测。
         */
        let registered = resolve_provider_target(name).ok();
        if registered.is_none() && resolved_base.trim().is_empty() {
            return Err(format!("未找到供应商：{name}"));
        }
        let (base, protocol, key) =
            merge_target((resolved_base, resolved_protocol, resolved_key), registered);
        resolved_base = base;
        resolved_protocol = protocol;
        resolved_key = key;
    }

    execute_discover_models(&resolved_base, &resolved_protocol, resolved_key.as_deref())
}

/// 合并界面直接给出的探测参数与已登记档案：直接给出的优先，档案只补缺项。
fn merge_target(
    requested: (String, String, Option<String>),
    registered: Option<(String, String, Option<String>)>,
) -> (String, String, Option<String>) {
    let (mut base, mut protocol, mut key) = requested;
    if let Some((entry_base, entry_protocol, entry_key)) = registered {
        if base.trim().is_empty() {
            base = entry_base;
        }
        if protocol.trim().is_empty() {
            protocol = entry_protocol;
        }
        if key.is_none() {
            key = entry_key;
        }
    }
    (base, protocol, key)
}

#[cfg(test)]
mod tests {
    use super::merge_target;

    fn entry() -> (String, String, Option<String>) {
        (
            "https://api.example.com/v1".to_string(),
            "openai-chatcompletions".to_string(),
            Some("EXAMPLE_API_KEY".to_string()),
        )
    }

    #[test]
    fn unregistered_provider_keeps_the_values_typed_in_the_form() {
        // 新建供应商：档案还不存在，界面给的地址与协议必须原样用于探测。
        let merged = merge_target(
            (
                "https://tokenrhythm.studio/v1".to_string(),
                "openai-chatcompletions".to_string(),
                Some("RHYTHM_API_KEY".to_string()),
            ),
            None,
        );
        assert_eq!(
            merged,
            (
                "https://tokenrhythm.studio/v1".to_string(),
                "openai-chatcompletions".to_string(),
                Some("RHYTHM_API_KEY".to_string())
            )
        );
    }

    #[test]
    fn registered_provider_only_fills_the_missing_fields() {
        // 已登记档案：界面没填的用档案补，界面填了的以界面为准。
        let merged = merge_target((String::new(), String::new(), None), Some(entry()));
        assert_eq!(merged, entry());

        let overridden = merge_target(
            (
                "https://override.example.com/v1".to_string(),
                String::new(),
                Some("OVERRIDE_KEY".to_string()),
            ),
            Some(entry()),
        );
        assert_eq!(
            overridden,
            (
                "https://override.example.com/v1".to_string(),
                "openai-chatcompletions".to_string(),
                Some("OVERRIDE_KEY".to_string())
            )
        );
    }
}
