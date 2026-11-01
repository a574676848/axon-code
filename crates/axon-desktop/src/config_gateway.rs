//! 配置类能力经网关的共用客户端。
//!
//! 阶段 2 起，桌面端不再读写本机的 `config.toml`、`.env`、`.mcp.json`、
//! `SOUL.md`、`AGENTS.md` 与技能目录，全部改为经网关 HTTP 读写。本模块是这批
//! 调用共用的一层：拼请求体、把回执反序列化成既有 DTO、把非 2xx 的状态码翻成
//! 可执行的中文结论。
//!
//! 端点、凭据与 TLS 信任一律复用既有实现，不另建一套：地址取
//! [`crate::gateway::endpoint_at`]（与 `get_raw` 同源），凭据取
//! [`crate::gateway::api_token`]，传输层取
//! [`crate::connection::endpoint::http_agent`]（平台根证书与全局超时都在那里）。
//! 这里之所以自带一次请求组装，是因为既有动词层只有 GET / POST / DELETE，
//! 而配置写入是 PUT。
//!
//! 两条降级路径都在这里定文案：
//!
//! - 路由缺失（404）：如实说「网关不含这条接口」，不翻成版本结论；
//! - 未连接态：连不上、证书失败、401 与 403 各给一句可展示的原因，界面据此走
//!   既有失败路径，不 panic、不返回伪造数据。

use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};

use crate::connection::endpoint;

/// 配置读取与写入路由。
pub(crate) const CONFIG_PATH: &str = "/api/v1/config";
/// 密钥状态路由；变量名以 `name` 查询参数给出。
pub(crate) const SECRETS_STATUS_PATH: &str = "/api/v1/secrets/status";
/// 密钥写入路由；`value` 为空表示清除。
pub(crate) const SECRETS_PATH: &str = "/api/v1/secrets";
/// 上游模型发现路由。
pub(crate) const PROVIDERS_DISCOVER_PATH: &str = "/api/v1/providers/discover";
/// MCP 配置路由。
pub(crate) const MCP_CONFIG_PATH: &str = "/api/v1/mcp/config";
/// 技能清单路由。
pub(crate) const SKILLS_PATH: &str = "/api/v1/skills";
/// 文档路由前缀，后接 `soul` 或 `agents`。
pub(crate) const DOCUMENTS_PATH_PREFIX: &str = "/api/v1/documents/";

/// 凭据被拒的结论：这是未连接态的一种，界面据此走未连接路径。
const CREDENTIAL_REJECTED: &str =
    "未连接：网关凭据缺失或不匹配，请在「设置 › 连接」里重新填写当前档案的令牌";
/// 回执结构不符时的结论。
const UNEXPECTED_BODY: &str = "后台服务回执不是预期的 JSON 结构";

/// 带正文的动词：配置类写入只有这两种。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BodyMethod {
    /// 模型发现。
    Post,
    /// 配置、密钥、MCP 与文档的写入。
    Put,
}

/// GET 一个配置类接口，返回解析后的 JSON。
///
/// 传输层失败沿用 [`crate::gateway::get_raw`] 的中文口径，不再二次包装。
///
/// # Errors
///
/// 网关不可达、凭据不匹配、后台服务版本过旧或回执不是合法 JSON。
pub(crate) fn get_json(path: &str) -> Result<Value, String> {
    // 本模块的路由常量都是完整路径（自带 `/api/v1`），因此必须走不补前缀的那条。
    let (status, body) = crate::gateway::get_raw_at(path)?;
    parse_reply(status, &body)
}

/// POST 一个配置类接口（上游模型发现）。
///
/// # Errors
///
/// 同 [`get_json`]。
pub(crate) fn post_json(path: &str, payload: &Value) -> Result<Value, String> {
    send_json(BodyMethod::Post, path, payload)
}

/// PUT 一个配置类接口（配置、密钥、MCP 与文档的写入）。
///
/// # Errors
///
/// 同 [`get_json`]。
pub(crate) fn put_json(path: &str, payload: &Value) -> Result<Value, String> {
    send_json(BodyMethod::Put, path, payload)
}

/// 把回执反序列化成既有 DTO。
///
/// 后台回执字段与既有类型逐字一致，因此直接反序列化，不另建一套平行类型；
/// 结构对不上时补一句中文前缀，界面不必读 serde 的英文报错。
///
/// # Errors
///
/// 回执缺少字段或字段类型不符。
pub(crate) fn decode<T: DeserializeOwned>(receipt: Value) -> Result<T, String> {
    serde_json::from_value(receipt).map_err(|error| format!("{UNEXPECTED_BODY}：{error}"))
}

/// 取回执里的一层字符串字段。
///
/// 用于回执只带一个正文或名字的场合（文档全文、MCP 原文），缺字段即视为结构
/// 不符，不把空串当成功。
///
/// # Errors
///
/// 回执缺少该字段，或该字段不是字符串。
pub(crate) fn string_field(receipt: &Value, key: &str) -> Result<String, String> {
    receipt
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("{UNEXPECTED_BODY}：缺少 {key}"))
}

/// 取回执里的一层布尔字段。
///
/// # Errors
///
/// 回执缺少该字段，或该字段不是布尔值。
pub(crate) fn bool_field(receipt: &Value, key: &str) -> Result<bool, String> {
    receipt
        .get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("{UNEXPECTED_BODY}：缺少 {key}"))
}

/// 带正文的请求：组装、发送、按状态码归类。
fn send_json(method: BodyMethod, path: &str, payload: &Value) -> Result<Value, String> {
    let url = crate::gateway::endpoint_at(path)?;
    let agent = endpoint::http_agent(endpoint::REQUEST_TIMEOUT);
    let request = match method {
        BodyMethod::Post => agent.post(&url),
        BodyMethod::Put => agent.put(&url),
    };
    let mut request = request
        .header("accept", "application/json")
        .header("content-type", "application/json");
    if let Some(token) = crate::gateway::api_token() {
        request = request.header("authorization", &format!("Bearer {token}"));
    }
    let (status, body) = match request.send(payload.to_string()) {
        Ok(response) => {
            let status = response.status().as_u16();
            let body = response
                .into_body()
                .read_to_string()
                .map_err(|error| format!("读取网关响应失败：{error}"))?;
            (status, body)
        }
        Err(error) => return Err(transport_failure(&error)),
    };
    parse_reply(status, &body)
}

/// 2xx 解析 JSON，其余按状态码归类。
fn parse_reply(status: u16, body: &str) -> Result<Value, String> {
    if !(200..300).contains(&status) {
        return Err(failure_message(status, body));
    }
    if body.trim().is_empty() {
        // 写操作的成功回执可能没有正文；交给 decode 判结构，这里不当成错误。
        return Ok(Value::Null);
    }
    serde_json::from_str(body).map_err(|_| UNEXPECTED_BODY.to_string())
}

/// 状态码到中文结论的映射。
///
/// 项目未上线，后端与桌面端同版本发布，因此 404 只意味着「这条路由不在」：按普通
/// 失败报出来，让地址、前缀或部署问题自己说话，不再翻成「后台服务版本过旧」。
/// 401 是未连接态的凭据问题，403 是权限问题，二者修复动作不同，因此分开。
/// 其余 4xx / 5xx 保留后台服务给出的原因，那是唯一能说明现场的一句话。
#[must_use]
pub(crate) fn failure_message(status: u16, body: &str) -> String {
    let reason = server_reason(body);
    match status {
        404 => join_reason("网关不含这条接口（HTTP 404）", &reason),
        401 => CREDENTIAL_REJECTED.to_string(),
        403 => join_reason("后台服务拒绝了这次操作：当前凭据没有该权限", &reason),
        411 => "请求缺少长度声明，后台服务拒绝接收".to_string(),
        413 => "内容过大，后台服务拒绝接收".to_string(),
        400 | 415 | 422 => join_reason("后台服务拒绝了这条请求", &reason),
        429 => "网关繁忙，请稍后重试".to_string(),
        500..=599 => join_reason("后台服务处理失败", &reason),
        _ => format!("网关返回状态码 {status}"),
    }
}

/// 取后台服务给的原因：优先 `message`，其次 `error` 错误码。
fn server_reason(body: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return String::new();
    };
    for key in ["message", "error"] {
        if let Some(text) = value.get(key).and_then(Value::as_str)
            && !text.trim().is_empty()
        {
            return text.trim().to_string();
        }
    }
    String::new()
}

/// 把基础结论与服务端原因拼成一句话。
fn join_reason(base: &str, reason: &str) -> String {
    if reason.is_empty() {
        base.to_string()
    } else {
        format!("{base}：{reason}")
    }
}

/// 传输层失败的结论：与探活同一套口径，界面读到的是同一类中文原因。
fn transport_failure(error: &ureq::Error) -> String {
    match error {
        ureq::Error::Tls(reason) => format!("证书有问题：{reason}"),
        ureq::Error::TlsRequired => "证书有问题：该地址要求 TLS，但地址写的是明文 http".to_string(),
        ureq::Error::Timeout(_) | ureq::Error::BodyStalled => {
            "连不上：网关没有在预算内应答，后台服务可能正忙".to_string()
        }
        ureq::Error::HostNotFound => "连不上：域名解析失败".to_string(),
        ureq::Error::ConnectionFailed => "连不上：连接被拒绝".to_string(),
        ureq::Error::Io(reason) if looks_like_certificate_failure(&reason.to_string()) => {
            format!("证书有问题：{reason}")
        }
        other => format!("连不上：{other}"),
    }
}

/// 判断一段错误文本是不是证书校验失败。
///
/// rustls 的握手错误会经由 `io::Error` 上抛，`ureq::Error::Tls` 只覆盖一部分情形；
/// 这里只兜住最常见的一类，完整分类仍归探活层。
fn looks_like_certificate_failure(text: &str) -> bool {
    let lowered = text.to_ascii_lowercase();
    lowered.contains("certificate") || lowered.contains("tls")
}

/// `PUT /api/v1/config` 的联合模式。
///
/// 四种形态与既有桌面端语义一一对应：`Text` 等价 `config_write`，
/// `Values` 等价 `config_apply_values`，`ResetKey` 等价 `config_reset_key`，
/// `Validate` 等价 `config_validate`（只校验不落盘）。
#[derive(Debug, Clone)]
pub(crate) enum ConfigUpdate {
    /// 整份 TOML 文本。
    Text {
        /// 待落盘的配置全文。
        toml_content: String,
    },
    /// 一批键值变更。
    Values {
        /// 待应用的键值；键名由调用方按升序整理，与后台的逐键顺序一致。
        values: BTreeMap<String, String>,
    },
    /// 单个键恢复内置默认值。
    ResetKey {
        /// 待恢复的配置键。
        key: String,
    },
    /// 只校验不落盘。
    Validate {
        /// 待校验的配置全文。
        toml_content: String,
    },
}

impl ConfigUpdate {
    /// 联合模式的请求体；字段名与后台适配器逐字一致。
    #[must_use]
    pub(crate) fn payload(&self) -> Value {
        match self {
            Self::Text { toml_content } => {
                json!({ "mode": "text", "tomlContent": toml_content })
            }
            Self::Values { values } => {
                let mut object = Map::new();
                for (key, value) in values {
                    object.insert(key.clone(), Value::String(value.clone()));
                }
                json!({ "mode": "values", "values": Value::Object(object) })
            }
            Self::ResetKey { key } => json!({ "mode": "resetKey", "key": key }),
            Self::Validate { toml_content } => {
                json!({ "mode": "validate", "tomlContent": toml_content })
            }
        }
    }

    /// 提交给后台并把回执交回调用方。
    ///
    /// # Errors
    ///
    /// 同 [`put_json`]。
    pub(crate) fn send(&self) -> Result<Value, String> {
        put_json(CONFIG_PATH, &self.payload())
    }
}

/// 拼出「把一批待应用的值应用到权威配置全文」之后的候选全文。
///
/// 后台的联合模式没有「按值校验」这一形态，因此这条路径用两次调用换等价语义：
/// 先 `GET /api/v1/config` 取权威全文，再在内存里应用这批值，最后由调用方交给
/// `validate` 模式判定，判定权仍在后台。取不到权威全文时如实失败，绝不猜。
///
/// # Errors
///
/// 网关失败、回执结构不符或某个键的取值不合法；错误逐条列出，便于界面一次
/// 展示全部问题。
pub(crate) fn compose_candidate_text(
    values: &BTreeMap<String, String>,
) -> Result<String, Vec<String>> {
    let receipt = get_json(CONFIG_PATH).map_err(|error| vec![error])?;
    let toml_content = receipt
        .get("toml_content")
        .and_then(Value::as_str)
        .ok_or_else(|| vec![UNEXPECTED_BODY.to_string()])?;
    apply_values_to_text(toml_content, values)
}

/// 在配置全文上应用一批键值，返回候选全文。
///
/// 与旧实现同一个原语（`set_config_value`），只是输入从文件换成了后台回执里的
/// 全文；键名按升序应用，与后台逐键顺序一致。
fn apply_values_to_text(
    toml_content: &str,
    values: &BTreeMap<String, String>,
) -> Result<String, Vec<String>> {
    let mut loaded = axon_config::ConfigLoader::load_from_str(toml_content)
        .map_err(|error| vec![format!("后台配置无法解析：{error:?}")])?;
    if values.is_empty() {
        return Ok(toml_content.to_string());
    }
    let mut errors = Vec::new();
    for (key, raw) in values {
        if let Err(error) = axon_config::set_config_value(&mut loaded.config, key, raw) {
            errors.push(format!("{key}：{error:?}"));
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    axon_config::to_toml_string(&loaded.config)
        .map_err(|error| vec![format!("规范化序列化失败：{error:?}")])
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::{ConfigUpdate, apply_values_to_text, failure_message};

    /// 一个合法的、可被解析的最小配置全文。
    const TOML: &str = "[agent]\nlog_level = \"info\"\n";

    #[test]
    fn text_mode_carries_the_full_toml_and_nothing_else() {
        let payload = ConfigUpdate::Text {
            toml_content: TOML.to_string(),
        }
        .payload();
        assert_eq!(
            payload,
            json!({ "mode": "text", "tomlContent": TOML }),
            "文本模式的请求体必须是 mode 与 tomlContent 两键"
        );
        assert_eq!(payload.as_object().map(serde_json::Map::len), Some(2));
    }

    #[test]
    fn values_mode_sends_a_string_map_only() {
        let mut values = BTreeMap::new();
        values.insert("agent.log_level".to_string(), "debug".to_string());
        values.insert("agent.reasoning_effort".to_string(), "low".to_string());
        let payload = ConfigUpdate::Values { values }.payload();
        assert_eq!(
            payload,
            json!({
                "mode": "values",
                "values": { "agent.log_level": "debug", "agent.reasoning_effort": "low" }
            }),
            "按值写入只带 mode 与 values 两键，取值一律是字符串"
        );
        assert!(payload.get("tomlContent").is_none());
    }

    #[test]
    fn reset_key_mode_carries_the_key() {
        let payload = ConfigUpdate::ResetKey {
            key: "agent.max_subagent_depth".to_string(),
        }
        .payload();
        assert_eq!(
            payload,
            json!({ "mode": "resetKey", "key": "agent.max_subagent_depth" }),
            "恢复默认值的请求体必须逐字给出键名"
        );
        assert!(payload.get("values").is_none());
    }

    #[test]
    fn validate_mode_never_carries_a_write_target() {
        let payload = ConfigUpdate::Validate {
            toml_content: TOML.to_string(),
        }
        .payload();
        assert_eq!(
            payload,
            json!({ "mode": "validate", "tomlContent": TOML }),
            "校验模式与文本模式形状相同，靠 mode 区分是否落盘"
        );
        assert_eq!(payload["mode"], "validate");
    }

    #[test]
    fn missing_route_is_reported_as_a_plain_404() {
        // 项目未上线，404 只说明这条路由不在：如实说，不翻成版本结论。
        let bare = failure_message(404, "");
        assert!(bare.contains("404"), "{bare}");
        assert!(!bare.contains("版本过旧"), "{bare}");
        let with_reason = failure_message(404, r#"{"error":"NOT_FOUND"}"#);
        assert!(with_reason.contains("NOT_FOUND"), "{with_reason}");
    }

    #[test]
    fn rejected_credentials_are_reported_as_disconnected() {
        let message = failure_message(401, r#"{"error":"unauthorized"}"#);
        assert!(message.starts_with("未连接"), "{message}");
        assert!(message.contains("凭据"), "{message}");

        let forbidden = failure_message(403, r#"{"error":"forbidden"}"#);
        assert!(!forbidden.contains("未连接"), "{forbidden}");
        assert!(forbidden.contains("权限"), "{forbidden}");
    }

    #[test]
    fn server_failures_keep_the_backend_reason() {
        let message = failure_message(
            500,
            r#"{"error":"write_failed","message":"写入配置失败：磁盘只读"}"#,
        );
        assert!(message.contains("后台服务处理失败"), "{message}");
        assert!(message.contains("磁盘只读"), "{message}");

        // 没有原因时也要给一句能读的结论，不能只剩状态码。
        let bare = failure_message(500, "");
        assert_eq!(bare, "后台服务处理失败");
    }

    #[test]
    fn candidate_text_applies_values_on_the_authoritative_config() {
        let mut values = BTreeMap::new();
        values.insert("agent.reasoning_effort".to_string(), "low".to_string());
        let candidate = apply_values_to_text(TOML, &values).expect("合法变更必须能拼出候选全文");
        assert!(
            candidate.contains("reasoning_effort = \"low\""),
            "{candidate}"
        );
        assert_eq!(
            candidate,
            axon_config::to_toml_string(
                &axon_config::ConfigLoader::load_from_str(&candidate)
                    .expect("候选全文必须仍可解析")
                    .config
            )
            .expect("候选全文必须可规范化"),
            "候选全文必须是规范化文本，后台据此判定"
        );
    }

    #[test]
    fn candidate_text_reports_every_illegal_value() {
        let mut values = BTreeMap::new();
        values.insert(
            "agent.reasoning_effort".to_string(),
            "不存在的档位".to_string(),
        );
        values.insert("不存在的键".to_string(), "1".to_string());
        let errors = apply_values_to_text(TOML, &values).expect_err("非法取值必须被拒绝");
        assert!(errors.len() >= 2, "每个非法键都要给一条原因：{errors:?}");
        assert!(
            errors
                .iter()
                .any(|line| line.contains("agent.reasoning_effort"))
        );
    }

    #[test]
    fn every_route_constant_is_a_full_path() {
        /*
         * 本模块的 GET 走 get_raw_at（不补前缀），POST 与 PUT 走 endpoint_at（同样不补）。
         * 因此这些常量必须自带 `/api/v1`；一旦有人把它们改成相对路径，请求会打到
         * `/api/v1/api/v1/...` 之外的错误地址，而 404 又会被翻成「后台服务版本过旧」，
         * 于是一个拼装错误被说成后端太旧。这条断言把分工固定下来。
         */
        for path in [
            super::CONFIG_PATH,
            super::SECRETS_STATUS_PATH,
            super::SECRETS_PATH,
            super::PROVIDERS_DISCOVER_PATH,
            super::MCP_CONFIG_PATH,
            super::SKILLS_PATH,
            super::DOCUMENTS_PATH_PREFIX,
        ] {
            assert!(
                path.starts_with("/api/v1/"),
                "配置类路由常量必须是带前缀的完整路径，实际：{path}"
            );
        }
    }

    #[test]
    fn full_path_routes_are_not_prefixed_twice() {
        /*
         * 复现曾经的缺陷：完整路径交给会补前缀的入口，拼出 `/api/v1/api/v1/config`。
         * 这里直接对拼装规则做断言，不依赖网络。
         */
        let base = "http://127.0.0.1:3143";
        let doubled = format!("{base}/api/v1{}", super::CONFIG_PATH);
        assert!(
            doubled.contains("/api/v1/api/v1/"),
            "这条断言记录的是错误拼法的形状：{doubled}"
        );
        // 正确拼法：完整路径直接跟在根地址后面。
        assert_eq!(
            format!("{base}{}", super::CONFIG_PATH),
            "http://127.0.0.1:3143/api/v1/config"
        );
    }
}
