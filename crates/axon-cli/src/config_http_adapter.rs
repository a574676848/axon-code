//! 配置类 HTTP 适配器：配置读取、键规格与配置写入。
//!
//! 三条路由与桌面端 `config_read` / `config_write` / `config_apply_values` /
//! `config_reset_key` / `config_validate` 同构：载荷字段名逐字沿用桌面端结构体
//! （`flat_values`、`toml_content`、`env_vars`、`specs`、`config_path`、`providers`、
//! `models`、`provider_protocols`、`provider_wire_transports`），因此同一份界面代码
//! 在直连宿主与经网关两条通道上拿到的是同一形状。
//!
//! 写入一律复用 `axon_config`：`ConfigLoader::load_from_str` 做权威校验，
//! `write_with_backup` 落盘（先备份再覆写），不另写第二套 TOML 写入。
//!
//! `PUT /api/v1/config` 是联合模式，按 `mode` 分派：
//!
//! - `text`：提交 TOML 全文，服务端校验后落盘；
//! - `values`：等价桌面端 `config_apply_values`，逐键应用后落盘；
//! - `resetKey`：等价桌面端 `config_reset_key`，把单个键恢复为内置默认值；
//! - `validate`：只校验不落盘，返回桌面端校验结论。
//!
//! 鉴权不在这里做：网关的 `custom()` 分派对 `/api/v1/*` 无条件执行鉴权，适配器
//! 拿不到豁免权。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use axon_config::{
    AxonConfig, ConfigKeySpec, ConfigLoader, ConfigValue, ConfigValueKind, config_key_specs,
    get_config_value, set_config_value, to_toml_string, write_with_backup,
};
use axon_daemon::{GatewayHttpResponse, GatewayHttpRouteHandler};
use serde_json::{Map, Value, json};

/// 配置读写路由。
const CONFIG_PATH: &str = "/api/v1/config";
/// 配置键规格路由。
const CONFIG_SCHEMA_PATH: &str = "/api/v1/config/schema";
/// 方法不匹配的错误码，与网关自身的 405 口径同名。
const METHOD_NOT_ALLOWED: &str = "method_not_allowed";
/// 请求体不是合法 JSON 的错误码。
const INVALID_JSON: &str = "invalid_json";
/// 未知 `mode` 的错误码。
const INVALID_MODE: &str = "invalid_mode";
/// 缺少 `tomlContent` 的错误码。
const MISSING_TOML_CONTENT: &str = "missing_toml_content";
/// 缺少 `values` 的错误码。
const MISSING_VALUES: &str = "missing_values";
/// `values` 里出现非字符串取值的错误码。
const INVALID_VALUES: &str = "invalid_values";
/// 缺少 `key` 的错误码。
const MISSING_KEY: &str = "missing_key";
/// 配置校验不通过的错误码。
const CONFIG_INVALID: &str = "config_invalid";
/// 未知配置键的错误码。
const UNKNOWN_KEY: &str = "unknown_key";
/// 配置落盘失败的错误码。
const WRITE_FAILED: &str = "write_failed";
/// 配置读取失败的错误码。
const READ_FAILED: &str = "read_failed";

/// 密钥文件相对配置文件目录的名称。
const ENV_FILE_NAME: &str = ".env";

/// 仅展示的环境变量名清单：只透出名字，绝不透出取值。
const DISPLAYED_ENV_KEYS: [&str; 6] = [
    "AXON_RUNTIME_ROOT",
    "AXON_CONFIG_PATH",
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "GLM_API_KEY",
    "DEEPSEEK_API_KEY",
];

/// 供应商协议候选值：与配置校验保持单一事实源。
const PROVIDER_PROTOCOLS: [&str; 5] = [
    axon_config::PROVIDER_PROTOCOL_OPENAI,
    axon_config::PROVIDER_PROTOCOL_OPENAI_CHAT,
    axon_config::PROVIDER_PROTOCOL_ANTHROPIC,
    axon_config::PROVIDER_PROTOCOL_DASHSCOPE,
    axon_config::PROVIDER_PROTOCOL_VOLCENGINE_ARK_MULTIMODAL,
];

/// 供应商链路候选值。
const PROVIDER_WIRE_TRANSPORTS: [&str; 2] = [
    axon_config::PROVIDER_WIRE_TRANSPORT_HTTP,
    axon_config::PROVIDER_WIRE_TRANSPORT_WEBSOCKET,
];

/// `text` 模式写入成功后的说明。
const MESSAGE_WRITTEN: &str = "配置已备份并写入";
/// 内容未变时的说明。
const MESSAGE_UNCHANGED: &str = "配置内容未发生变化，已跳过写入";
/// 批量应用成功后的说明。
const MESSAGE_APPLIED: &str = "已保存";
/// 恢复默认值成功后的说明。
const MESSAGE_RESET: &str = "已恢复默认值";
/// 没有待应用变更时的说明。
const MESSAGE_NO_CHANGES: &str = "没有需要应用的变更";
/// 内存校验通过时的摘要。
const SUMMARY_VALID: &str = "权威内存校验通过 (语法与结构完整)";

/// 配置 HTTP 适配器。
///
/// 适配器持有**权威配置文件路径**：读取、写入与 `.env` 定位都从它派生，
/// 不再从环境推断，因此守护进程用 `--config` 指定的自定义路径同样成立。
pub(crate) struct ConfigHttpAdapter {
    /// 配置文件路径。
    config_path: PathBuf,
}

impl ConfigHttpAdapter {
    /// 以配置文件路径构造。
    pub(crate) fn new(config_path: impl Into<PathBuf>) -> Self {
        Self {
            config_path: config_path.into(),
        }
    }

    /// 读取配置并投影成桌面端 `config_read` 的同构载荷。
    fn read(&self) -> GatewayHttpResponse {
        self.ensure_credentials_loaded();
        let loaded = match ConfigLoader::load_from_path(&self.config_path) {
            Ok(loaded) => loaded,
            Err(error) => {
                return error_with_message(500, READ_FAILED, format!("加载配置失败：{error:?}"));
            }
        };
        let toml_content = match to_toml_string(&loaded.config) {
            Ok(text) => text,
            Err(error) => {
                return error_with_message(500, READ_FAILED, format!("序列化配置失败：{error:?}"));
            }
        };

        let mut flat_values = Map::new();
        for spec in config_key_specs() {
            if let Some(value) = get_config_value(&loaded.config, spec.key) {
                flat_values.insert(spec.key.to_owned(), config_value_to_json(value));
            }
        }

        let providers = loaded
            .config
            .providers
            .iter()
            .map(|provider| {
                json!({
                    "name": provider.name,
                    "enabled": provider.enabled,
                    "protocol": provider.protocol,
                    "base_url": provider.base_url,
                    "api_key_env": provider.api_key_env,
                    "api_key_configured": provider
                        .api_key_env
                        .as_deref()
                        .is_some_and(|name| axon_config::provider_credential(name).is_some()),
                    "wire_transport": provider.wire_transport,
                    "priority": provider.priority,
                    "weight": provider.weight,
                })
            })
            .collect::<Vec<_>>();

        let models = loaded
            .config
            .models
            .iter()
            .map(|model| {
                json!({
                    "name": model.name,
                    "id": model.id,
                    "provider": model.provider,
                    "context_window": model.context_window,
                    "max_output_tokens": model.max_output_tokens,
                    "supports_vision": model.supports_vision,
                    "supports_embedding": model.supports_embedding,
                    "embedding_dimension": model.embedding_dimension,
                    "requires_reasoning_content_on_assistant_messages":
                        model.requires_reasoning_content_on_assistant_messages,
                })
            })
            .collect::<Vec<_>>();

        let env_vars = DISPLAYED_ENV_KEYS
            .iter()
            .filter(|name| std::env::var(name).is_ok())
            .map(|name| (*name).to_owned())
            .collect::<Vec<_>>();

        GatewayHttpResponse::json(
            200,
            &json!({
                "flat_values": Value::Object(flat_values),
                "toml_content": toml_content,
                "env_vars": env_vars,
                "specs": specs_payload(),
                "config_path": self.config_path.to_string_lossy(),
                "providers": providers,
                "models": models,
                "provider_protocols": PROVIDER_PROTOCOLS,
                "provider_wire_transports": PROVIDER_WIRE_TRANSPORTS,
            }),
        )
    }

    /// 联合模式写入：按 `mode` 分派。
    fn update(&self, body: &[u8]) -> GatewayHttpResponse {
        let Ok(request) = serde_json::from_slice::<Value>(body) else {
            return error(400, INVALID_JSON);
        };
        let Some(mode) = request.get("mode").and_then(Value::as_str) else {
            return error(400, INVALID_MODE);
        };
        match mode.trim() {
            "text" => match text_field(&request, "tomlContent") {
                Some(toml_content) => self.write_text(toml_content),
                None => error(400, MISSING_TOML_CONTENT),
            },
            "values" => self.apply_values(&request),
            "resetKey" => match text_field(&request, "key") {
                Some(key) => self.reset_key(key),
                None => error(400, MISSING_KEY),
            },
            "validate" => match text_field(&request, "tomlContent") {
                Some(toml_content) => {
                    GatewayHttpResponse::json(200, &validate_payload(toml_content))
                }
                None => error(400, MISSING_TOML_CONTENT),
            },
            _ => error(400, INVALID_MODE),
        }
    }

    /// `text` 模式：校验 TOML 全文后整份落盘。
    fn write_text(&self, toml_content: &str) -> GatewayHttpResponse {
        let loaded = match ConfigLoader::load_from_str(toml_content) {
            Ok(loaded) => loaded,
            Err(error) => {
                return error_with_message(
                    400,
                    CONFIG_INVALID,
                    format!("配置校验未通过，禁止落盘：{error:?}"),
                );
            }
        };
        let canonical = match to_toml_string(&loaded.config) {
            Ok(text) => text,
            Err(error) => {
                return error_with_message(
                    400,
                    CONFIG_INVALID,
                    format!("规范化序列化失败：{error:?}"),
                );
            }
        };
        if is_unchanged(Path::new(&self.config_path), &canonical) {
            return GatewayHttpResponse::json(
                200,
                &json!({
                    "written": false,
                    "message": MESSAGE_UNCHANGED,
                    "backup_created": false,
                }),
            );
        }
        self.persist(&canonical, MESSAGE_WRITTEN)
    }

    /// `values` 模式：逐键应用后整份落盘，语义等价桌面端 `config_apply_values`。
    fn apply_values(&self, request: &Value) -> GatewayHttpResponse {
        let Some(values) = request.get("values").and_then(Value::as_object) else {
            return error(400, MISSING_VALUES);
        };
        let mut pairs = BTreeMap::<String, String>::new();
        for (key, value) in values {
            let Some(raw) = value.as_str() else {
                return error_with_message(
                    400,
                    INVALID_VALUES,
                    format!("配置项 {key} 的取值必须是字符串"),
                );
            };
            pairs.insert(key.clone(), raw.to_owned());
        }
        if pairs.is_empty() {
            return GatewayHttpResponse::json(
                200,
                &json!({
                    "written": false,
                    "message": MESSAGE_NO_CHANGES,
                    "backup_created": false,
                }),
            );
        }
        let mut loaded = match ConfigLoader::load_from_path(&self.config_path) {
            Ok(loaded) => loaded,
            Err(error) => {
                return error_with_message(400, CONFIG_INVALID, format!("加载配置失败：{error:?}"));
            }
        };
        // 按键名升序应用：与桌面端一致，避免同一批变更因哈希顺序产生不同结论。
        for (key, raw) in &pairs {
            if let Err(error) = set_config_value(&mut loaded.config, key, raw) {
                return error_with_message(
                    400,
                    CONFIG_INVALID,
                    format!("配置项 {key} 取值非法：{error:?}"),
                );
            }
        }
        let canonical = match to_toml_string(&loaded.config) {
            Ok(text) => text,
            Err(error) => {
                return error_with_message(
                    400,
                    CONFIG_INVALID,
                    format!("规范化序列化失败：{error:?}"),
                );
            }
        };
        self.persist(&canonical, MESSAGE_APPLIED)
    }

    /// `resetKey` 模式：把单个键恢复为内置默认值，语义等价桌面端 `config_reset_key`。
    fn reset_key(&self, key: &str) -> GatewayHttpResponse {
        let Some(default_value) = get_config_value(&AxonConfig::default(), key) else {
            return error_with_message(400, UNKNOWN_KEY, format!("未知配置键：{key}"));
        };
        let mut loaded = match ConfigLoader::load_from_path(&self.config_path) {
            Ok(loaded) => loaded,
            Err(error) => {
                return error_with_message(400, CONFIG_INVALID, format!("加载配置失败：{error:?}"));
            }
        };
        if let Err(error) = set_config_value(&mut loaded.config, key, &default_value.to_string()) {
            return error_with_message(400, CONFIG_INVALID, format!("重置配置项失败：{error:?}"));
        }
        let canonical = match to_toml_string(&loaded.config) {
            Ok(text) => text,
            Err(error) => {
                return error_with_message(
                    400,
                    CONFIG_INVALID,
                    format!("规范化序列化失败：{error:?}"),
                );
            }
        };
        self.persist(&canonical, MESSAGE_RESET)
    }

    /// 落盘：先备份再覆写，回执里如实报告是否生成了备份。
    fn persist(&self, canonical: &str, message: &str) -> GatewayHttpResponse {
        match write_with_backup(&self.config_path, canonical.as_bytes()) {
            Ok(backup) => GatewayHttpResponse::json(
                200,
                &json!({
                    "written": true,
                    "message": message,
                    "backup_created": backup.is_some(),
                }),
            ),
            Err(error) => error_with_message(500, WRITE_FAILED, format!("写入配置失败：{error:?}")),
        }
    }

    /// 加载配置相邻 `.env` 中的凭据。
    ///
    /// 运行时会话启动时会加载该文件；网关侧必须做同样的事，否则界面会把已经配好的
    /// 密钥误报成「未配置」。
    fn ensure_credentials_loaded(&self) {
        let Some(parent) = self.config_path.parent() else {
            return;
        };
        let _ = axon_config::load_provider_credentials_from_env_file(&parent.join(ENV_FILE_NAME));
    }
}

impl GatewayHttpRouteHandler for ConfigHttpAdapter {
    fn handle_request(
        &self,
        method: &str,
        path: &str,
        _query: Option<&str>,
        body: &[u8],
    ) -> Option<GatewayHttpResponse> {
        // 精确比较路径：本适配器只拥有这两条路由，未命中一律交回组合器，
        // 不得返回统一 404 把后续适配器截断。
        if path == CONFIG_SCHEMA_PATH {
            return Some(if method == "GET" {
                GatewayHttpResponse::json(200, &json!({ "specs": specs_payload() }))
            } else {
                method_not_allowed()
            });
        }
        if path != CONFIG_PATH {
            return None;
        }
        Some(match method {
            "GET" => self.read(),
            "PUT" => self.update(body),
            _ => method_not_allowed(),
        })
    }
}

/// 键规格载荷：字段名与桌面端 `ConfigKeySpecDto` 逐字一致。
fn specs_payload() -> Vec<Value> {
    config_key_specs()
        .iter()
        .map(spec_payload)
        .collect::<Vec<_>>()
}

fn spec_payload(spec: &ConfigKeySpec) -> Value {
    json!({
        "key": spec.key,
        "kind": kind_token(spec.kind),
        "editable": spec.editable,
        "description": spec.description,
        "setup_module": spec.setup_module,
        "default_value": spec.default_value,
        "options": spec.options.iter().map(|option| (*option).to_owned()).collect::<Vec<_>>(),
        "depends_on": spec
            .depends_on
            .iter()
            .map(|dependency| (*dependency).to_owned())
            .collect::<Vec<_>>(),
    })
}

/// 键类型取值，与桌面端 `ConfigKeySpecDto.kind` 的字符串逐字一致。
fn kind_token(kind: ConfigValueKind) -> &'static str {
    match kind {
        ConfigValueKind::Bool => "bool",
        ConfigValueKind::Unsigned => "unsigned",
        ConfigValueKind::String => "string",
        ConfigValueKind::StringList => "string_list",
    }
}

/// 配置值转 JSON：与桌面端 `convert_config_value_to_json` 一一对应。
fn config_value_to_json(value: ConfigValue) -> Value {
    match value {
        ConfigValue::Bool(value) => Value::Bool(value),
        ConfigValue::Unsigned(value) => Value::from(value),
        ConfigValue::String(value) => Value::String(value),
        ConfigValue::StringList(values) => {
            Value::Array(values.into_iter().map(Value::String).collect())
        }
    }
}

/// 纯内存校验载荷：字段名与桌面端 `ConfigValidateResponse` 逐字一致。
fn validate_payload(toml_content: &str) -> Value {
    match ConfigLoader::load_from_str(toml_content) {
        Ok(_) => json!({
            "valid": true,
            "errors": [],
            "warnings": [],
            "diff_summary": [SUMMARY_VALID],
        }),
        Err(error) => json!({
            "valid": false,
            "errors": [format!("{error:?}")],
            "warnings": [],
            "diff_summary": [],
        }),
    }
}

/// 磁盘上的配置是否已经等于待写入内容。
fn is_unchanged(path: &Path, canonical: &str) -> bool {
    std::fs::read(path).is_ok_and(|existing| existing == canonical.as_bytes())
}

/// 取请求体里的字符串字段；缺省与空串同义。
fn text_field<'a>(request: &'a Value, key: &str) -> Option<&'a str> {
    request.get(key).and_then(Value::as_str)
}

/// 只有错误码的响应：形状与合同里的 `{"error":"..."}` 逐字一致。
fn error(status: u16, code: &str) -> GatewayHttpResponse {
    GatewayHttpResponse::json(status, &json!({ "error": code }))
}

/// 带中文原因的响应：错误码保持稳定，原因供界面直接展示。
fn error_with_message(status: u16, code: &str, message: impl Into<String>) -> GatewayHttpResponse {
    GatewayHttpResponse::json(status, &json!({ "error": code, "message": message.into() }))
}

/// 方法不匹配响应：路径属于本适配器，但该方法不被支持。
fn method_not_allowed() -> GatewayHttpResponse {
    error(405, METHOD_NOT_ALLOWED)
}

#[cfg(test)]
#[path = "config_http_adapter_tests.rs"]
mod config_http_adapter_tests;
