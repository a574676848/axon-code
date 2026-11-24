//! 密钥状态 HTTP 适配器：查询是否已配置、写入或清除配置相邻 `.env` 里的密钥。
//!
//! 与桌面端 `secrets.rs` 的 `secret_status_of` / `secret_set` / `secret_clear` 等价：
//! 复用配置系统既有的 `.env` 补丁原语 `axon_config::setup::render_env_from`
//! （保留文件里的注释与原有顺序，不重新生成整份文件），落盘走
//! `axon_config::write_with_backup`（先时间戳备份再直接覆写），与 `config.toml`
//! 的写入策略一致。
//!
//! 安全边界：读取路径只回传「已配置 / 未配置」这一位状态；密钥明文既不回传界面，
//! 也不出现在任何响应与日志里。因此本模块的所有回执只有 `name` 与 `configured`
//! 两个字段。

use std::fs;
use std::path::PathBuf;

use axon_config::setup::{EnvKeyChange, SETUP_SECRET_PRESENT, render_env_from, secret_status};
use axon_config::{set_provider_credential, write_with_backup};
use axon_daemon::{GatewayHttpResponse, GatewayHttpRouteHandler};
use serde_json::{Value, json};

/// 密钥写入路由。
const SECRETS_PATH: &str = "/api/v1/secrets";
/// 密钥状态路由。
const SECRETS_STATUS_PATH: &str = "/api/v1/secrets/status";
/// 密钥文件相对配置文件目录的名称。
const ENV_FILE_NAME: &str = ".env";
/// 方法不匹配的错误码，与网关自身的 405 口径同名。
const METHOD_NOT_ALLOWED: &str = "method_not_allowed";
/// 请求体不是合法 JSON 的错误码。
const INVALID_JSON: &str = "invalid_json";
/// 缺少 `name` 的错误码。
const MISSING_NAME: &str = "missing_name";
/// 变量名非法的错误码。
const INVALID_NAME: &str = "invalid_name";
/// 缺少 `value` 字段的错误码。
const MISSING_VALUE: &str = "missing_value";
/// 取值非法的错误码。
const INVALID_VALUE: &str = "invalid_value";
/// 无法定位配置目录的错误码。
const SECRETS_UNAVAILABLE: &str = "secrets_unavailable";
/// 写入密钥文件失败的错误码。
const WRITE_FAILED: &str = "write_failed";

/// 密钥状态与写入适配器。
///
/// 适配器持有权威配置文件路径：`.env` 一律取配置文件所在目录，
/// 与桌面端「配置相邻 .env」的口径完全一致。
pub(crate) struct SecretsHttpAdapter {
    /// 配置文件路径。
    config_path: PathBuf,
}

impl SecretsHttpAdapter {
    /// 以配置文件路径构造。
    pub(crate) fn new(config_path: impl Into<PathBuf>) -> Self {
        Self {
            config_path: config_path.into(),
        }
    }

    /// `GET /api/v1/secrets/status?name=<变量名>`。
    fn status(&self, query: Option<&str>) -> GatewayHttpResponse {
        let Some(raw_name) = query_value(query, "name") else {
            return error(400, MISSING_NAME);
        };
        let name = raw_name.trim();
        if name.is_empty() {
            return error(400, MISSING_NAME);
        }
        let name = match validate_key(name) {
            Ok(name) => name,
            Err(message) => return error_with_message(400, INVALID_NAME, message),
        };
        let path = match self.env_file_path() {
            Ok(path) => path,
            Err(message) => return error_with_message(500, SECRETS_UNAVAILABLE, message),
        };
        let configured = secret_status(&path, name) == SETUP_SECRET_PRESENT;
        GatewayHttpResponse::json(200, &json!({ "name": name, "configured": configured }))
    }

    /// `PUT /api/v1/secrets`：体为 `{"name":"...","value":"..."}`，空 `value` 表示清除。
    fn set(&self, body: &[u8]) -> GatewayHttpResponse {
        let Ok(request) = serde_json::from_slice::<Value>(body) else {
            return error(400, INVALID_JSON);
        };
        let Some(raw_name) = request.get("name").and_then(Value::as_str) else {
            return error(400, MISSING_NAME);
        };
        let name = match validate_key(raw_name) {
            Ok(name) => name.to_owned(),
            Err(message) => return error_with_message(400, INVALID_NAME, message),
        };
        let Some(value) = request.get("value").and_then(Value::as_str) else {
            return error(400, MISSING_VALUE);
        };
        // 换行会把后续内容挤成新的一行键值对，从而写坏整份 `.env`，因此直接拒绝。
        if value.contains('\n') || value.contains('\r') {
            return error_with_message(400, INVALID_VALUE, "密钥值不能包含换行符".to_owned());
        }
        match self.apply(&name, value) {
            Ok(configured) => {
                GatewayHttpResponse::json(200, &json!({ "name": name, "configured": configured }))
            }
            Err(message) => error_with_message(500, WRITE_FAILED, message),
        }
    }

    /// 写入或清除一个密钥，返回写入后的配置状态。
    ///
    /// 明文只在本函数内停留：既不进回执，也不进任何错误文案。
    fn apply(&self, name: &str, value: &str) -> Result<bool, String> {
        let path = self.env_file_path()?;
        let raw = fs::read(&path).ok();
        if value.trim().is_empty() {
            if let Some(rendered) =
                render_env_from(raw.as_deref(), &[EnvKeyChange::Clear(name.to_owned())])
            {
                write_with_backup(&path, &rendered)
                    .map_err(|error| format!("写入密钥文件失败：{error:?}"))?;
            }
            // 进程内凭据仓库必须与磁盘一致：清除之后旧值不能继续存活。
            let _ = axon_config::load_provider_credentials_from_env_file(&path);
            // 本来就不存在时同样视为清除成功：语义是幂等的「确保没有这个密钥」。
            return Ok(false);
        }
        if let Some(rendered) = render_env_from(
            raw.as_deref(),
            &[EnvKeyChange::Set(name.to_owned(), value.to_owned())],
        ) {
            write_with_backup(&path, &rendered)
                .map_err(|error| format!("写入密钥文件失败：{error:?}"))?;
        }
        // 同步进程内缓存，界面无需重启即可看到「已配置」。
        set_provider_credential(name, value);
        let _ = axon_config::load_provider_credentials_from_env_file(&path);
        Ok(true)
    }

    /// 配置相邻 `.env` 的路径。
    fn env_file_path(&self) -> Result<PathBuf, String> {
        let parent = self
            .config_path
            .parent()
            .ok_or_else(|| "无法定位配置目录".to_owned())?;
        Ok(parent.join(ENV_FILE_NAME))
    }
}

impl GatewayHttpRouteHandler for SecretsHttpAdapter {
    fn handle_request(
        &self,
        method: &str,
        path: &str,
        query: Option<&str>,
        body: &[u8],
    ) -> Option<GatewayHttpResponse> {
        // 两条路由都精确匹配：未命中一律交回组合器，不得返回统一 404 截断后续适配器。
        if path == SECRETS_STATUS_PATH {
            return Some(if method == "GET" {
                self.status(query)
            } else {
                method_not_allowed()
            });
        }
        if path != SECRETS_PATH {
            return None;
        }
        Some(match method {
            "PUT" => self.set(body),
            _ => method_not_allowed(),
        })
    }
}

/// 校验环境变量名：只允许字母、数字与下划线，且不以数字开头。
fn validate_key(key: &str) -> Result<&str, String> {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        return Err("环境变量名不能为空".to_owned());
    }
    let mut chars = trimmed.chars();
    let first_ok = chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_');
    if !first_ok || !chars.all(|item| item.is_ascii_alphanumeric() || item == '_') {
        return Err("环境变量名只能包含字母、数字与下划线，且不能以数字开头".to_owned());
    }
    Ok(trimmed)
}

/// 读取单个查询参数；空值与缺省同义。
///
/// 环境变量名只含字母、数字与下划线，不存在需要百分号解码的字符，
/// 因此查询串按字面取值，不引入解码层。
fn query_value(query: Option<&str>, key: &str) -> Option<String> {
    query?.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (name == key).then(|| value.to_owned())
    })
}

/// 只有错误码的响应。
fn error(status: u16, code: &str) -> GatewayHttpResponse {
    GatewayHttpResponse::json(status, &json!({ "error": code }))
}

/// 带中文原因的响应。
fn error_with_message(status: u16, code: &str, message: String) -> GatewayHttpResponse {
    GatewayHttpResponse::json(status, &json!({ "error": code, "message": message }))
}

/// 方法不匹配响应。
fn method_not_allowed() -> GatewayHttpResponse {
    error(405, METHOD_NOT_ALLOWED)
}

#[cfg(test)]
#[path = "secrets_http_adapter_tests.rs"]
mod secrets_http_adapter_tests;
