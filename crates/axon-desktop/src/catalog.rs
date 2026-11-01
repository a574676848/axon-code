//! 供应商档案与模型目录的一体化增删改。
//!
//! 一个「供应商档案」等于一条供应商配置加上它名下的全部模型条目。
//! 界面上一张卡片对应一个档案，保存时整卡一次提交，因此这里的所有写入
//! 都是单次原子落盘，绝不会出现供应商已改而模型半新的中间态。
//!
//! 写入一律经网关：后台没有供应商档案专用路由，因此用配置读写换等价语义，
//! 先 `GET /api/v1/config` 取权威全文，在内存里整卡改一处，再以文本模式
//! `PUT /api/v1/config` 整份写回，落盘前由后台重新校验。这样同一份代码在
//! 云端档案下同样成立：桌面端不再解析本机 `config.toml`。
//!
//! 安全边界：本模块只接受 `api_key_env`（环境变量名），
//! 结构化参数里不存在 `api_key` 明文字段，密钥绝不穿越 IPC。

use std::path::Path;

use axon_config::{ConfigLoader, ModelConfig, ProviderConfig, add_provider, to_toml_string};

use crate::config_gateway::{self, ConfigUpdate};
use crate::ipc::{ConfigReadResponse, ConfigWriteResponse};

/// 供应商写入入参。
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
pub struct ProviderInput {
    /// 供应商标识名称。
    pub name: String,
    /// 是否启用。
    pub enabled: bool,
    /// 协议类型。
    pub protocol: String,
    /// API 基础地址。
    pub base_url: String,
    /// API Key 环境变量引用名。
    pub api_key_env: Option<String>,
    /// 底层传输链路。
    pub wire_transport: String,
    /// WebSocket 地址。
    pub websocket_url: Option<String>,
    /// 是否启用请求压缩。
    pub enable_request_compression: bool,
    /// 调度优先级。
    pub priority: i32,
    /// 调度权重。
    pub weight: u32,
}

impl Default for ProviderInput {
    fn default() -> Self {
        let base = ProviderConfig::new(String::new(), String::new(), String::new());
        Self {
            name: base.name,
            enabled: base.enabled,
            protocol: base.protocol,
            base_url: base.base_url,
            api_key_env: base.api_key_env,
            wire_transport: base.wire_transport,
            websocket_url: base.websocket_url,
            enable_request_compression: base.enable_request_compression,
            priority: base.priority,
            weight: base.weight,
        }
    }
}

/// 模型写入入参。
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
pub struct ModelInput {
    /// 模型名称（配置里的路由键）。
    pub name: String,
    /// 真实模型 ID。
    pub id: String,
    /// 关联供应商。
    pub provider: String,
    /// 上下文窗口。
    pub context_window: u64,
    /// 最大输出 token。
    pub max_output_tokens: Option<u64>,
    /// 是否支持视觉。
    pub supports_vision: bool,
    /// 是否支持 embedding。
    pub supports_embedding: bool,
    /// embedding 维度。
    pub embedding_dimension: usize,
    /// 是否要求历史助手消息逐条回传推理内容。
    pub requires_reasoning_content_on_assistant_messages: bool,
}

impl Default for ModelInput {
    fn default() -> Self {
        let base = ModelConfig::default();
        Self {
            name: base.name,
            id: base.id,
            provider: base.provider,
            context_window: base.context_window,
            max_output_tokens: base.max_output_tokens,
            supports_vision: base.supports_vision,
            supports_embedding: base.supports_embedding,
            embedding_dimension: base.embedding_dimension,
            requires_reasoning_content_on_assistant_messages: base
                .requires_reasoning_content_on_assistant_messages,
        }
    }
}

/// 校验并原子落盘。
///
/// 只服务按路径写入的离线场合（自检与单测）：界面命令一律经网关，由后台
/// 校验后落盘。
fn persist(path: &Path, config: &axon_config::AxonConfig) -> Result<ConfigWriteResponse, String> {
    config
        .validate()
        .map_err(|error| format!("配置校验未通过：{error:?}"))?;
    ConfigLoader::save_to_path(path, config)
        .map_err(|error| format!("原子写配置失败：{error:?}"))?;
    Ok(ConfigWriteResponse {
        written: true,
        message: "已保存".to_string(),
        backup_created: true,
    })
}

/// 由入参构建供应商配置。
fn build_provider(input: &ProviderInput) -> Result<ProviderConfig, String> {
    let name = input.name.trim();
    if name.is_empty() {
        return Err("供应商名称不能为空".to_string());
    }
    if input.base_url.trim().is_empty() {
        return Err("供应商接口地址不能为空".to_string());
    }

    let mut provider = ProviderConfig::new(name, input.protocol.trim(), input.base_url.trim());
    provider.enabled = input.enabled;
    provider.api_key_env = input
        .api_key_env
        .clone()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    if !input.wire_transport.trim().is_empty() {
        provider.wire_transport = input.wire_transport.trim().to_string();
    }
    provider.websocket_url = input
        .websocket_url
        .clone()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    provider.enable_request_compression = input.enable_request_compression;
    provider.priority = input.priority;
    provider.weight = input.weight;
    Ok(provider)
}

/// 由入参构建模型配置。
fn build_model(input: &ModelInput, provider_name: &str) -> Result<ModelConfig, String> {
    let name = input.name.trim();
    if name.is_empty() {
        return Err("模型名称不能为空".to_string());
    }
    let id = input.id.trim();
    if id.is_empty() {
        return Err(format!("模型 {name} 缺少模型 ID"));
    }

    Ok(ModelConfig {
        name: name.to_string(),
        id: id.to_string(),
        provider: provider_name.to_string(),
        context_window: input.context_window,
        max_output_tokens: input.max_output_tokens,
        supports_vision: input.supports_vision,
        supports_embedding: input.supports_embedding,
        embedding_dimension: input.embedding_dimension,
        requires_reasoning_content_on_assistant_messages: input
            .requires_reasoning_content_on_assistant_messages,
    })
}

/// 模型槽位与模型目录保持自洽。
///
/// 配置校验要求：只要模型目录非空，`agent.default_model` 与 `agent.fast_model`
/// 都必须非空且引用一个已配置模型。因此在新增、改名或删除模型之后，
/// 必须把失效的槽位重新绑定，否则用户将无法保存任何模型变更。
fn reconcile_model_slots(config: &mut axon_config::AxonConfig) {
    if config.models.is_empty() {
        config.agent.default_model = String::new();
        config.agent.fast_model = String::new();
        return;
    }

    let fallback = config.models[0].name.clone();
    let references_existing = |config: &axon_config::AxonConfig, value: &str| {
        let trimmed = value.trim();
        !trimmed.is_empty()
            && config
                .models
                .iter()
                .any(|model| model.name == trimmed || model.id == trimmed)
    };

    if !references_existing(config, &config.agent.default_model) {
        config.agent.default_model = fallback;
    }
    let default_name = config.agent.default_model.clone();
    if !references_existing(config, &config.agent.fast_model) {
        config.agent.fast_model = default_name;
    }
}

/// 在指定路径上整体保存一个供应商档案。
///
/// 入参里的模型列表是该供应商名下的完整集合：不在列表中的旧模型会被移除，
/// 列表中的新增或修改会就地生效，供应商改名时其模型的归属同步跟随。
pub fn provider_profile_upsert_at(
    path: &Path,
    input: ProviderInput,
    models: Vec<ModelInput>,
    original_name: Option<String>,
) -> Result<ConfigWriteResponse, String> {
    let mut loaded =
        ConfigLoader::load_from_path(path).map_err(|error| format!("加载配置失败：{error:?}"))?;
    upsert_into(&mut loaded.config, input, models, original_name)?;
    persist(path, &loaded.config)
}

/// 在指定路径上移除一个供应商档案，连同它名下的全部模型。
pub fn provider_profile_remove_at(path: &Path, name: &str) -> Result<ConfigWriteResponse, String> {
    let mut loaded =
        ConfigLoader::load_from_path(path).map_err(|error| format!("加载配置失败：{error:?}"))?;
    remove_from(&mut loaded.config, name)?;
    persist(path, &loaded.config)
}

/// 在内存里整体写入一个供应商档案；落盘由调用方负责。
///
/// 校验放在最前面：整卡里任何一处不合法，都不会留下半新半旧的中间态。
fn upsert_into(
    config: &mut axon_config::AxonConfig,
    input: ProviderInput,
    models: Vec<ModelInput>,
    original_name: Option<String>,
) -> Result<(), String> {
    let provider = build_provider(&input)?;
    let final_name = provider.name.clone();
    let previous_name = original_name
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    let mut built_models = Vec::with_capacity(models.len());
    for entry in &models {
        built_models.push(build_model(entry, &final_name)?);
    }
    let mut names: Vec<&str> = built_models
        .iter()
        .map(|model| model.name.as_str())
        .collect();
    names.sort_unstable();
    let unique_count = {
        names.dedup();
        names.len()
    };
    if unique_count != built_models.len() {
        return Err("同一供应商下的模型名称不能重复".to_string());
    }

    let target = previous_name.clone().unwrap_or_else(|| final_name.clone());
    match config
        .providers
        .iter()
        .position(|existing| existing.name == target)
    {
        Some(index) => {
            let duplicated = config
                .providers
                .iter()
                .enumerate()
                .any(|(other, entry)| other != index && entry.name == final_name);
            if duplicated {
                return Err("provider.name 不能重复".to_string());
            }
            config.providers[index] = provider;
        }
        None => {
            add_provider(config, provider).map_err(|error| format!("新增供应商失败：{error:?}"))?;
        }
    }

    // 改名后原模型需要跟随到新名称，否则会留下悬空引用。
    if let Some(previous) = previous_name.as_deref()
        && previous != final_name
    {
        for model in &mut config.models {
            if model.provider == previous {
                model.provider = final_name.clone();
            }
        }
    }

    // 归属本供应商的旧模型：以本次提交的集合为准做替换。
    let keep: Vec<String> = built_models
        .iter()
        .map(|model| model.name.clone())
        .collect();
    config
        .models
        .retain(|model| !(model.provider == final_name && !keep.contains(&model.name)));
    for model in built_models {
        match config
            .models
            .iter()
            .position(|existing| existing.name == model.name)
        {
            Some(index) => config.models[index] = model,
            None => config.models.push(model),
        }
    }

    reconcile_model_slots(config);
    Ok(())
}

/// 在内存里移除一个供应商档案，连同它名下的全部模型；落盘由调用方负责。
fn remove_from(config: &mut axon_config::AxonConfig, name: &str) -> Result<(), String> {
    let trimmed = name.trim();
    let before = config.providers.len();
    config.providers.retain(|entry| entry.name != trimmed);
    if config.providers.len() == before {
        return Err("供应商不存在".to_string());
    }
    config.models.retain(|model| model.provider != trimmed);
    reconcile_model_slots(config);
    Ok(())
}

/// 取权威配置全文、在内存里改一处、整份写回后台。
///
/// 读改写都在网关侧：桌面端只做一次转发式的编辑，落盘与校验都由后台完成。
fn edit_config(
    edit: impl FnOnce(&mut axon_config::AxonConfig) -> Result<(), String>,
) -> Result<ConfigWriteResponse, String> {
    let receipt = config_gateway::get_json(config_gateway::CONFIG_PATH)?;
    let read: ConfigReadResponse = config_gateway::decode(receipt)?;
    let mut loaded = ConfigLoader::load_from_str(&read.toml_content)
        .map_err(|error| format!("后台配置无法解析：{error:?}"))?;
    edit(&mut loaded.config)?;
    let toml_content =
        to_toml_string(&loaded.config).map_err(|error| format!("规范化序列化失败：{error:?}"))?;
    let receipt = (ConfigUpdate::Text { toml_content }).send()?;
    config_gateway::decode(receipt)
}

/// Tauri IPC 命令：整体保存一个供应商档案。
///
/// # Errors
///
/// 入参不合法、网关不可达或后台服务版本过旧。
#[tauri::command(async)]
pub fn provider_profile_upsert(
    input: ProviderInput,
    models: Vec<ModelInput>,
    original_name: Option<String>,
) -> Result<ConfigWriteResponse, String> {
    edit_config(move |config| upsert_into(config, input, models, original_name))
}

/// Tauri IPC 命令：移除一个供应商档案及其全部模型。
///
/// # Errors
///
/// 供应商不存在、网关不可达或后台服务版本过旧。
#[tauri::command(async)]
pub fn provider_profile_remove(name: String) -> Result<ConfigWriteResponse, String> {
    edit_config(|config| remove_from(config, &name))
}

#[cfg(test)]
mod tests {
    use super::{ModelInput, ProviderInput, remove_from, upsert_into};

    /// 一个待保存的供应商档案，字段取值与界面提交的同形。
    fn profile() -> ProviderInput {
        ProviderInput {
            name: "demo-provider".to_string(),
            enabled: true,
            protocol: "openai".to_string(),
            base_url: "https://example.com/v1".to_string(),
            api_key_env: Some("DEMO_API_KEY".to_string()),
            wire_transport: "http".to_string(),
            priority: 10,
            weight: 1,
            ..ProviderInput::default()
        }
    }

    /// 一个待保存的模型条目。
    fn model() -> ModelInput {
        ModelInput {
            name: "demo-a".to_string(),
            id: "demo-a-v1".to_string(),
            provider: "demo-provider".to_string(),
            context_window: 64000,
            ..ModelInput::default()
        }
    }

    #[test]
    fn gateway_text_round_trip_keeps_profile_models_and_slots() {
        // 网关路径把改动写成整份 TOML 交给后台，因此拼出来的文本必须仍可解析，
        // 并且供应商、模型与模型槽位一个都不少。
        let mut config = axon_config::AxonConfig::default();
        upsert_into(&mut config, profile(), vec![model()], None).expect("整卡写入必须成功");

        let text = axon_config::to_toml_string(&config).expect("必须能序列化成权威 TOML");
        let reloaded =
            axon_config::ConfigLoader::load_from_str(&text).expect("拼出的全文必须仍可解析");
        assert_eq!(reloaded.config.providers.len(), 1);
        assert_eq!(reloaded.config.models.len(), 1);
        assert_eq!(reloaded.config.providers[0].name, "demo-provider");
        assert_eq!(
            reloaded.config.agent.default_model, "demo-a",
            "模型目录非空时默认模型槽位必须绑到已配置模型"
        );
    }

    #[test]
    fn removing_an_unknown_profile_is_an_error() {
        let mut config = axon_config::AxonConfig::default();
        assert!(remove_from(&mut config, "不存在的档案").is_err());
    }

    #[test]
    fn removing_the_only_profile_clears_the_model_slots() {
        let mut config = axon_config::AxonConfig::default();
        upsert_into(&mut config, profile(), vec![model()], None).expect("整卡写入必须成功");
        remove_from(&mut config, "demo-provider").expect("移除已有档案必须成功");
        assert!(config.providers.is_empty());
        assert!(config.models.is_empty());
        assert!(config.agent.default_model.is_empty());
    }
}
