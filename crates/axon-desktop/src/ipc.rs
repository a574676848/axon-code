use std::collections::HashMap;
use std::fs;
use std::path::Path;

use axon_config::{
    AxonConfig, ConfigLoader, ConfigValue, ConfigValueKind, config_key_specs, get_config_value,
    provider_credential, set_config_value, to_toml_string,
};
use serde::{Deserialize, Serialize};

/// 前端键规格传输模型。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigKeySpecDto {
    /// 配置键点分路径。
    pub key: String,
    /// 键类型。
    pub kind: String,
    /// 是否允许用户编辑。
    pub editable: bool,
    /// 说明文本。
    pub description: String,
    /// 所属 setup 模块。
    pub setup_module: String,
    /// 默认值文本。
    pub default_value: String,
    /// 可选项。
    pub options: Vec<String>,
    /// 依赖前置键。
    pub depends_on: Vec<String>,
}

/// 供应商配置模型。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderDto {
    /// 供应商标识名称。
    pub name: String,
    /// 是否启用。
    pub enabled: bool,
    /// 通信协议。
    pub protocol: String,
    /// API 端点基础路径。
    pub base_url: String,
    /// API Key 环境变量引用名。
    pub api_key_env: Option<String>,
    /// 该引用当前是否能解析到真实凭据。
    pub api_key_configured: bool,
    /// 底层网络协议传输。
    pub wire_transport: String,
    /// 调度优先级。
    pub priority: i32,
    /// 调度权重。
    pub weight: u32,
}

/// 模型条目配置模型。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelDto {
    /// 模型名称。
    pub name: String,
    /// 真实模型 ID。
    pub id: String,
    /// 关联供应商。
    pub provider: String,
    /// 上下文视口大小。
    pub context_window: u64,
    /// 最大输出 Token。
    pub max_output_tokens: Option<u64>,
    /// 是否支持视觉多模态。
    pub supports_vision: bool,
    /// 是否支持向量检索。
    pub supports_embedding: bool,
    /// 向量维度；仅 supports_embedding 为 true 时有效。
    pub embedding_dimension: usize,
}

/// 配置读取响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigReadResponse {
    /// 扁平配置键值字典。
    pub flat_values: HashMap<String, serde_json::Value>,
    /// 权威 TOML 规格文本。
    pub toml_content: String,
    /// 仅展示的环境变量名清单（绝不透出密钥明文）。
    pub env_vars: Vec<String>,
    /// 配置键规格定义。
    pub specs: Vec<ConfigKeySpecDto>,
    /// 配置文件权威路径。
    pub config_path: String,
    /// 已注册供应商列表。
    pub providers: Vec<ProviderDto>,
    /// 已注册模型列表。
    pub models: Vec<ModelDto>,
    /// 供应商协议候选值，与配置校验保持单一事实源。
    pub provider_protocols: Vec<String>,
    /// 供应商链路候选值。
    pub provider_wire_transports: Vec<String>,
}

/// 配置校验响应（纯内存，不落盘）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigValidateResponse {
    /// 校验是否完全通过。
    pub valid: bool,
    /// 错误信息列表。
    pub errors: Vec<String>,
    /// 警告信息列表。
    pub warnings: Vec<String>,
    /// 变更或校验摘要。
    pub diff_summary: Vec<String>,
}

/// 配置写入响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigWriteResponse {
    /// 是否实际执行了写入（内容未变时为 false）。
    pub written: bool,
    /// 状态说明消息。
    pub message: String,
    /// 是否建立了安全备份。
    pub backup_created: bool,
}

/// 将系统 ConfigValue 转为 JSON Value。
fn convert_config_value_to_json(val: ConfigValue) -> serde_json::Value {
    match val {
        ConfigValue::Bool(b) => serde_json::Value::Bool(b),
        ConfigValue::Unsigned(u) => serde_json::Value::Number(u.into()),
        ConfigValue::String(s) => serde_json::Value::String(s),
        ConfigValue::StringList(list) => {
            serde_json::Value::Array(list.into_iter().map(serde_json::Value::String).collect())
        }
    }
}

/// 加载配置相邻 `.env` 中的凭据。
///
/// 运行时会话启动时会加载该文件；桌面宿主必须做同样的事，
/// 否则界面上会把已经配好的密钥误报成「未配置」。
fn ensure_credentials_loaded(config_path: &Path) {
    let Some(parent) = config_path.parent() else {
        return;
    };
    let _ = axon_config::load_provider_credentials_from_env_file(&parent.join(".env"));
}

/// 执行真实配置读取。
pub fn execute_config_read_at(config_path: &Path) -> Result<ConfigReadResponse, String> {
    ensure_credentials_loaded(config_path);
    let loaded =
        ConfigLoader::load_from_path(config_path).map_err(|e| format!("加载配置失败: {e:?}"))?;

    let toml_content =
        to_toml_string(&loaded.config).map_err(|e| format!("序列化配置失败: {e:?}"))?;

    let mut flat_values = HashMap::new();
    let specs_raw = config_key_specs();
    let mut specs = Vec::with_capacity(specs_raw.len());

    for s in specs_raw {
        if let Some(val) = get_config_value(&loaded.config, s.key) {
            flat_values.insert(s.key.to_string(), convert_config_value_to_json(val));
        }

        let kind_str = match s.kind {
            ConfigValueKind::Bool => "bool",
            ConfigValueKind::Unsigned => "unsigned",
            ConfigValueKind::String => "string",
            ConfigValueKind::StringList => "string_list",
        };

        specs.push(ConfigKeySpecDto {
            key: s.key.to_string(),
            kind: kind_str.to_string(),
            editable: s.editable,
            description: s.description.to_string(),
            setup_module: s.setup_module.to_string(),
            default_value: s.default_value.to_string(),
            options: s.options.iter().map(|&opt| opt.to_string()).collect(),
            depends_on: s.depends_on.iter().map(|&dep| dep.to_string()).collect(),
        });
    }

    // 收集安全环境变量名清单（仅透出变量名，绝不透传密钥明文）
    let known_env_keys = [
        "AXON_RUNTIME_ROOT",
        "AXON_CONFIG_PATH",
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "GLM_API_KEY",
        "DEEPSEEK_API_KEY",
    ];
    let env_vars = known_env_keys
        .iter()
        .filter(|&&k| std::env::var(k).is_ok())
        .map(|&k| k.to_string())
        .collect();

    let providers = loaded
        .config
        .providers
        .iter()
        .map(|p| ProviderDto {
            name: p.name.clone(),
            enabled: p.enabled,
            protocol: p.protocol.clone(),
            base_url: p.base_url.clone(),
            api_key_env: p.api_key_env.clone(),
            api_key_configured: p
                .api_key_env
                .as_deref()
                .is_some_and(|name| provider_credential(name).is_some()),
            wire_transport: p.wire_transport.clone(),
            priority: p.priority,
            weight: p.weight,
        })
        .collect();

    let models = loaded
        .config
        .models
        .iter()
        .map(|m| ModelDto {
            name: m.name.clone(),
            id: m.id.clone(),
            provider: m.provider.clone(),
            context_window: m.context_window,
            max_output_tokens: m.max_output_tokens,
            supports_vision: m.supports_vision,
            supports_embedding: m.supports_embedding,
            embedding_dimension: m.embedding_dimension,
        })
        .collect();

    Ok(ConfigReadResponse {
        flat_values,
        toml_content,
        env_vars,
        specs,
        config_path: config_path.to_string_lossy().to_string(),
        providers,
        models,
        provider_protocols: vec![
            axon_config::PROVIDER_PROTOCOL_OPENAI.to_string(),
            axon_config::PROVIDER_PROTOCOL_OPENAI_CHAT.to_string(),
            axon_config::PROVIDER_PROTOCOL_ANTHROPIC.to_string(),
            axon_config::PROVIDER_PROTOCOL_DASHSCOPE.to_string(),
            axon_config::PROVIDER_PROTOCOL_VOLCENGINE_ARK_MULTIMODAL.to_string(),
        ],
        provider_wire_transports: vec![
            axon_config::PROVIDER_WIRE_TRANSPORT_HTTP.to_string(),
            axon_config::PROVIDER_WIRE_TRANSPORT_WEBSOCKET.to_string(),
        ],
    })
}

/// 执行权威内存校验（纯内存，不落盘）。
pub fn execute_config_validate(toml_content: &str) -> ConfigValidateResponse {
    match ConfigLoader::load_from_str(toml_content) {
        Ok(_) => ConfigValidateResponse {
            valid: true,
            errors: Vec::new(),
            warnings: Vec::new(),
            diff_summary: vec!["权威内存校验通过 (语法与结构完整)".to_string()],
        },
        Err(err) => ConfigValidateResponse {
            valid: false,
            errors: vec![format!("{err:?}")],
            warnings: Vec::new(),
            diff_summary: Vec::new(),
        },
    }
}

/// 备份后直接写入配置（复用 ConfigLoader::save_to_path）。
pub fn execute_config_write_at(
    config_path: &Path,
    toml_content: &str,
) -> Result<ConfigWriteResponse, String> {
    // 1. 内存中先行严格校验
    let loaded = ConfigLoader::load_from_str(toml_content)
        .map_err(|e| format!("配置校验未通过，禁止落盘: {e:?}"))?;

    let canonical_toml =
        to_toml_string(&loaded.config).map_err(|e| format!("规范化序列化失败: {e:?}"))?;

    // 2. 检查现有文件是否完全一致
    let existing_match = fs::read(config_path)
        .ok()
        .is_some_and(|existing| existing == canonical_toml.as_bytes());

    if existing_match {
        return Ok(ConfigWriteResponse {
            written: false,
            message: "配置内容未发生变化，已跳过写入".to_string(),
            backup_created: false,
        });
    }

    // 3. 父目录按需创建
    if let Some(parent) = config_path.parent()
        && !parent.exists()
    {
        fs::create_dir_all(parent).map_err(|e| format!("创建配置目录失败: {e:?}"))?;
    }

    // 4. 调用权威配置保存
    ConfigLoader::save_to_path(config_path, &loaded.config)
        .map_err(|e| format!("写入配置失败: {e:?}"))?;

    Ok(ConfigWriteResponse {
        written: true,
        message: "配置已备份并写入".to_string(),
        backup_created: config_path.exists(),
    })
}

/// 解析当前用户的配置文件路径。
fn user_config_path() -> Result<std::path::PathBuf, String> {
    ConfigLoader::default_user_config_path()
        .map_err(|error| format!("获取用户配置路径失败: {error:?}"))
}

/// 在指定路径上批量应用配置键值，备份后直接写入原文件。
pub fn execute_apply_values_at(
    path: &Path,
    values: &HashMap<String, String>,
) -> Result<ConfigWriteResponse, String> {
    if values.is_empty() {
        return Ok(ConfigWriteResponse {
            written: false,
            message: "没有需要应用的变更".to_string(),
            backup_created: false,
        });
    }

    let mut loaded =
        ConfigLoader::load_from_path(path).map_err(|error| format!("加载配置失败: {error:?}"))?;

    let mut keys: Vec<&String> = values.keys().collect();
    keys.sort();
    for key in keys {
        let raw = values.get(key).cloned().unwrap_or_default();
        set_config_value(&mut loaded.config, key, &raw)
            .map_err(|error| format!("配置项 {key} 取值非法：{error:?}"))?;
    }

    ConfigLoader::save_to_path(path, &loaded.config)
        .map_err(|error| format!("写入配置失败: {error:?}"))?;
    Ok(ConfigWriteResponse {
        written: true,
        message: "已保存".to_string(),
        backup_created: true,
    })
}

/// 在指定路径上把单个配置项恢复为内置默认值。
pub fn execute_reset_key_at(path: &Path, key: &str) -> Result<ConfigWriteResponse, String> {
    let default_value = get_config_value(&AxonConfig::default(), key)
        .ok_or_else(|| format!("未知配置键：{key}"))?;

    let mut loaded =
        ConfigLoader::load_from_path(path).map_err(|error| format!("加载配置失败: {error:?}"))?;
    set_config_value(&mut loaded.config, key, &default_value.to_string())
        .map_err(|error| format!("重置配置项失败：{error:?}"))?;
    ConfigLoader::save_to_path(path, &loaded.config)
        .map_err(|error| format!("写入配置失败: {error:?}"))?;
    Ok(ConfigWriteResponse {
        written: true,
        message: "已恢复默认值".to_string(),
        backup_created: true,
    })
}

/// 在指定路径上对一批待应用变更做纯内存试算校验，不落盘。
pub fn execute_validate_values_at(
    path: &Path,
    values: &HashMap<String, String>,
) -> ConfigValidateResponse {
    let mut loaded = match ConfigLoader::load_from_path(path) {
        Ok(value) => value,
        Err(error) => {
            return ConfigValidateResponse {
                valid: false,
                errors: vec![format!("加载配置失败: {error:?}")],
                warnings: Vec::new(),
                diff_summary: Vec::new(),
            };
        }
    };

    let mut keys: Vec<&String> = values.keys().collect();
    keys.sort();
    let mut errors = Vec::new();
    for key in &keys {
        let raw = values.get(*key).cloned().unwrap_or_default();
        if let Err(error) = set_config_value(&mut loaded.config, key, &raw) {
            errors.push(format!("{key}：{error:?}"));
        }
    }

    ConfigValidateResponse {
        valid: errors.is_empty(),
        errors,
        warnings: Vec::new(),
        diff_summary: if keys.is_empty() {
            vec!["没有待应用的变更".to_string()]
        } else {
            vec![format!("待应用 {} 项配置变更", keys.len())]
        },
    }
}

/// Tauri IPC 命令：读取基础配置。
#[tauri::command]
pub fn config_read() -> Result<ConfigReadResponse, String> {
    execute_config_read_at(&user_config_path()?)
}

/// Tauri IPC 命令：校验配置（纯内存，不落盘）。
#[tauri::command]
pub fn config_validate(toml_content: String) -> ConfigValidateResponse {
    execute_config_validate(&toml_content)
}

/// Tauri IPC 命令：备份后写入配置。
#[tauri::command]
pub fn config_write(toml_content: String) -> Result<ConfigWriteResponse, String> {
    execute_config_write_at(&user_config_path()?, &toml_content)
}

/// Tauri IPC 命令：批量应用配置键值并落盘。
#[tauri::command]
pub fn config_apply_values(values: HashMap<String, String>) -> Result<ConfigWriteResponse, String> {
    execute_apply_values_at(&user_config_path()?, &values)
}

/// Tauri IPC 命令：对一批待应用变更做纯内存试算校验，不落盘。
#[tauri::command]
pub fn config_validate_values(values: HashMap<String, String>) -> ConfigValidateResponse {
    match user_config_path() {
        Ok(path) => execute_validate_values_at(&path, &values),
        Err(error) => ConfigValidateResponse {
            valid: false,
            errors: vec![error],
            warnings: Vec::new(),
            diff_summary: Vec::new(),
        },
    }
}

/// Tauri IPC 命令：把单个配置项恢复为内置默认值。
#[tauri::command]
pub fn config_reset_key(key: String) -> Result<ConfigWriteResponse, String> {
    execute_reset_key_at(&user_config_path()?, &key)
}

/// Tauri IPC 命令：在系统默认文本编辑器中打开配置文件。
#[tauri::command]
pub fn open_config_file() -> Result<(), String> {
    let path = user_config_path()?;
    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        let _ = std::fs::create_dir_all(parent);
    }
    if !path.exists() {
        let _ = std::fs::write(&path, "");
    }
    crate::process::open_path(&path).map_err(|error| format!("无法打开配置文件：{error}"))
}

/// 朗读音频：base64 编码的 WAV 与 MIME 类型。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceAudioPayload {
    /// base64 编码的 WAV 字节。
    pub audio_base64: String,
    /// MIME 类型，固定 `audio/wav`。
    pub mime_type: String,
}

/// 合成一段朗读音频，由 Rust 侧带网关凭据发起请求。
///
/// # Errors
///
/// 参数为空、网关不可达、语音运行时未就绪或合成失败。
#[tauri::command]
pub fn voice_tts(text: String, preset_id: Option<String>) -> Result<VoiceAudioPayload, String> {
    use base64::Engine as _;
    let text = text.trim();
    if text.is_empty() {
        return Err("没有可朗读的内容".to_owned());
    }
    let bytes =
        crate::gateway::post_voice_tts(text, preset_id.as_deref()).map_err(
            |error| match error {
                crate::gateway::VoiceTtsError::Unavailable(reason) => {
                    format!("voice_unavailable: {reason}")
                }
                crate::gateway::VoiceTtsError::Failed(reason) => reason,
            },
        )?;
    Ok(VoiceAudioPayload {
        audio_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
        mime_type: "audio/wav".to_owned(),
    })
}
