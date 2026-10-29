//! 供应商档案与模型目录的一体化增删改。
//!
//! 一个「供应商档案」等于一条供应商配置加上它名下的全部模型条目。
//! 界面上一张卡片对应一个档案，保存时整卡一次提交，因此这里的所有写入
//! 都是单次原子落盘，绝不会出现供应商已改而模型半新的中间态。
//!
//! 落盘一律复用 `ConfigLoader::save_to_path`，因此仍然走
//! `axon_config::atomic_write::replace_file_atomically` 的原子替换路径，
//! 并在替换前保留时间戳备份。
//!
//! 安全边界：本模块只接受 `api_key_env`（环境变量名），
//! 结构化参数里不存在 `api_key` 明文字段，密钥绝不穿越 IPC。

use std::path::Path;

use axon_config::{ConfigLoader, ModelConfig, ProviderConfig, add_provider};

use crate::ipc::ConfigWriteResponse;

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

/// 解析当前用户配置文件路径。
fn config_path() -> Result<std::path::PathBuf, String> {
    ConfigLoader::default_user_config_path()
        .map_err(|error| format!("获取用户配置路径失败：{error:?}"))
}

/// 校验并原子落盘。
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

    let mut loaded =
        ConfigLoader::load_from_path(path).map_err(|error| format!("加载配置失败：{error:?}"))?;

    let target = previous_name.clone().unwrap_or_else(|| final_name.clone());
    match loaded
        .config
        .providers
        .iter()
        .position(|existing| existing.name == target)
    {
        Some(index) => {
            let duplicated = loaded
                .config
                .providers
                .iter()
                .enumerate()
                .any(|(other, entry)| other != index && entry.name == final_name);
            if duplicated {
                return Err("provider.name 不能重复".to_string());
            }
            loaded.config.providers[index] = provider;
        }
        None => {
            add_provider(&mut loaded.config, provider)
                .map_err(|error| format!("新增供应商失败：{error:?}"))?;
        }
    }

    // 改名后原模型需要跟随到新名称，否则会留下悬空引用。
    if let Some(previous) = previous_name.as_deref()
        && previous != final_name
    {
        for model in &mut loaded.config.models {
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
    loaded
        .config
        .models
        .retain(|model| !(model.provider == final_name && !keep.contains(&model.name)));
    for model in built_models {
        match loaded
            .config
            .models
            .iter()
            .position(|existing| existing.name == model.name)
        {
            Some(index) => loaded.config.models[index] = model,
            None => loaded.config.models.push(model),
        }
    }

    reconcile_model_slots(&mut loaded.config);
    persist(path, &loaded.config)
}

/// 在指定路径上移除一个供应商档案，连同它名下的全部模型。
pub fn provider_profile_remove_at(path: &Path, name: &str) -> Result<ConfigWriteResponse, String> {
    let mut loaded =
        ConfigLoader::load_from_path(path).map_err(|error| format!("加载配置失败：{error:?}"))?;
    let trimmed = name.trim();
    let before = loaded.config.providers.len();
    loaded
        .config
        .providers
        .retain(|entry| entry.name != trimmed);
    if loaded.config.providers.len() == before {
        return Err("供应商不存在".to_string());
    }
    loaded
        .config
        .models
        .retain(|model| model.provider != trimmed);
    reconcile_model_slots(&mut loaded.config);
    persist(path, &loaded.config)
}

/// Tauri IPC 命令：整体保存一个供应商档案。
#[tauri::command(async)]
pub fn provider_profile_upsert(
    input: ProviderInput,
    models: Vec<ModelInput>,
    original_name: Option<String>,
) -> Result<ConfigWriteResponse, String> {
    provider_profile_upsert_at(&config_path()?, input, models, original_name)
}

/// Tauri IPC 命令：移除一个供应商档案及其全部模型。
#[tauri::command(async)]
pub fn provider_profile_remove(name: String) -> Result<ConfigWriteResponse, String> {
    provider_profile_remove_at(&config_path()?, &name)
}
