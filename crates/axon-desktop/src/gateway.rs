//! 网关客户端：桌面端访问 Axon 运行态的唯一入口。
//!
//! 桌面端不直连运行态数据库，也不链接会话子系统，一律经网关 HTTP 协议读写，
//! 从而与后端实现解耦。网关地址与凭据都取自权威配置：
//! 监听地址读 `remote.gateway.http_bind`，凭据名读 `remote.gateway.api_token_env`，
//! 凭据值从配置相邻的 `.env` 加载。
//!
//! 通配绑定地址（`0.0.0.0`、`::`）表示「所有网卡」，不能作为连接目标，
//! 这里统一映射到回环地址后再发起请求。
//!
//! 网关有**两个协议面**：工作台扩展面以 [`API_PREFIX`] 为前缀，交互事件面则是
//! `/gateway/v1/*`。两者共用同一份地址与凭据，因此地址组装只在这里实现一次。

use std::time::Duration;

use axon_config::{ConfigLoader, get_config_value};

/// 网关 HTTP 监听地址配置键。
pub const KEY_HTTP_BIND: &str = "remote.gateway.http_bind";
/// 网关 WebSocket 监听地址配置键。
pub const KEY_WEBSOCKET_BIND: &str = "remote.gateway.websocket_bind";
/// 网关凭据环境变量名配置键。
pub const KEY_API_TOKEN_ENV: &str = "remote.gateway.api_token_env";
/// HTTP 监听地址缺失时的兜底值。
const FALLBACK_HTTP_BIND: &str = "127.0.0.1:3143";
/// WebSocket 监听地址缺失时的兜底值。
const FALLBACK_WEBSOCKET_BIND: &str = "127.0.0.1:3144";
/// 凭据变量名缺失时的兜底值。
const FALLBACK_TOKEN_ENV: &str = "AXON_GATEWAY_API_TOKEN";
/// 单次请求超时。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// 朗读合成的总超时。
///
/// 本地语音合成是命令行逐次加载模型，长句在冷启动时要十几秒；用通用的 15 秒预算
/// 会把「正在合成」误报成「网关不可达」。朗读本身也按句分段，单段很少接近这个上限。
const VOICE_TTS_TIMEOUT: Duration = Duration::from_secs(60);

/// 试听合成的总超时；首次试听要加载模型，给足冷启动余量。
const VOICE_PREVIEW_TIMEOUT: Duration = Duration::from_secs(30);

/// 把通配绑定地址映射成可连接的探测地址。
#[must_use]
pub fn probe_target(bind: &str) -> String {
    let trimmed = bind.trim();
    if let Some(port) = trimmed.strip_prefix("0.0.0.0:") {
        return format!("127.0.0.1:{port}");
    }
    if let Some(port) = trimmed.strip_prefix("[::]:") {
        return format!("[::1]:{port}");
    }
    if let Some(port) = trimmed.strip_prefix("::") {
        return format!("[::1]:{port}");
    }
    trimmed.to_string()
}

/// 读取配置项文本值。
pub fn config_text(key: &str) -> Option<String> {
    let path = ConfigLoader::default_user_config_path().ok()?;
    let loaded = ConfigLoader::load_from_path(&path).ok()?;
    get_config_value(&loaded.config, key).map(|value| value.to_string())
}

/// 网关根地址，形如 `http://127.0.0.1:3143`。
pub fn base_url() -> Result<String, String> {
    let bind = config_text(KEY_HTTP_BIND).unwrap_or_else(|| FALLBACK_HTTP_BIND.to_string());
    Ok(format!("http://{}", probe_target(&bind)))
}

/// 网关 WebSocket 根地址，形如 `ws://127.0.0.1:3144`。
pub fn websocket_base_url() -> Result<String, String> {
    let bind =
        config_text(KEY_WEBSOCKET_BIND).unwrap_or_else(|| FALLBACK_WEBSOCKET_BIND.to_string());
    Ok(format!("ws://{}", probe_target(&bind)))
}

/// 网关凭据；未配置时返回 None。
///
/// 与供应商凭据同源：先把配置相邻 `.env` 加载进进程缓存，再按配置声明的
/// 变量名取值，因此凭据只存在一份，不在桌面端另建存储。
pub fn api_token() -> Option<String> {
    let name = config_text(KEY_API_TOKEN_ENV).unwrap_or_else(|| FALLBACK_TOKEN_ENV.to_string());
    if let Some(existing) = axon_config::provider_credential(&name) {
        return Some(existing);
    }
    let config_path = ConfigLoader::default_user_config_path().ok()?;
    let env_path = config_path.parent()?.join(".env");
    let _ = axon_config::load_provider_credentials_from_env_file(&env_path);
    axon_config::provider_credential(&name)
}

/// 构造带鉴权的请求代理。
fn agent() -> ureq::Agent {
    agent_with_timeout(REQUEST_TIMEOUT)
}

fn agent_with_timeout(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        // 非 2xx 也交回响应对象：工作台拒绝请求时会在响应体里给出原因
        // （例如「该作业不属于这条车道」），把状态码当异常抛出会丢掉这句话，
        // 用户只能看到一句无法行动的状态码。
        .http_status_as_error(false)
        .build()
        .new_agent()
}

/// 网关请求共用的路径前缀。
const API_PREFIX: &str = "/api/v1";

/// 组装完整地址。
fn endpoint(path: &str) -> Result<String, String> {
    Ok(format!("{}{API_PREFIX}{path}", base_url()?))
}

/// 组装不经工作台前缀的完整地址。
///
/// 交互事件面（`/gateway/v1/*`）与工作台面同源同凭据，但不共用前缀，
/// 因此这里显式接受完整路径，避免调用方各自拼字符串。
pub fn endpoint_at(path: &str) -> Result<String, String> {
    Ok(format!("{}{path}", base_url()?))
}

/// 构造带鉴权的 GET 请求。
fn authorized_get(url: &str) -> ureq::RequestBuilder<ureq::typestate::WithoutBody> {
    authorized_get_with_timeout(url, REQUEST_TIMEOUT)
}

fn authorized_get_with_timeout(
    url: &str,
    timeout: Duration,
) -> ureq::RequestBuilder<ureq::typestate::WithoutBody> {
    let mut request = agent_with_timeout(timeout)
        .get(url)
        .header("accept", "application/json");
    if let Some(token) = api_token() {
        request = request.header("authorization", &format!("Bearer {token}"));
    }
    request
}

/// GET 一个网关接口并解析 JSON。
pub fn get_json(path: &str) -> Result<serde_json::Value, String> {
    let (status, body) = get_raw(path)?;
    if !(200..300).contains(&status) {
        return Err(format!("网关返回状态码 {status}"));
    }
    serde_json::from_str(&body).map_err(|_| "网关响应不是合法 JSON".to_string())
}

/// GET 一个网关接口，返回状态码与响应体。
///
/// 探测需要区分状态码：401 是凭据问题，404 是服务端不含该接口（版本过旧），
/// 二者修复动作完全不同，必须分别归类而不是笼统报「接口调用失败」。
pub fn get_raw(path: &str) -> Result<(u16, String), String> {
    get_raw_with_timeout(path, REQUEST_TIMEOUT)
}

/// GET 一个网关接口，并使用调用方指定的总超时。
pub fn get_raw_with_timeout(path: &str, timeout: Duration) -> Result<(u16, String), String> {
    let url = endpoint(path)?;
    let request = authorized_get_with_timeout(&url, timeout);
    match request.call() {
        Ok(response) => {
            let status = response.status().as_u16();
            let body = response
                .into_body()
                .read_to_string()
                .map_err(|error| format!("读取网关响应失败：{error}"))?;
            Ok((status, body))
        }
        Err(ureq::Error::StatusCode(status)) => Ok((status, String::new())),
        Err(error) => Err(format!("网关不可达：{error}")),
    }
}

/// 探活失败的原因分类：决定桌面端给用户哪一句话、要不要重试。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeFailure {
    /// 请求在预算内没有完成：网关可能正在排队，也可能是连接挂住。
    Timeout,
    /// 连接层失败：端口无人接受连接，或链路根本不通。
    Unreachable,
    /// 网关已经应答，但响应体没能读完。
    BodyRead,
}

/// 带失败分类的 GET。
///
/// 口径与 [`get_raw_with_timeout`] 完全一致，只是把传输层错误分成「超时」与
/// 「不可达」两类：二者的修复动作不同，笼统报一句不可达会把排队误报成宕机。
pub fn probe_raw_with_timeout(
    path: &str,
    timeout: Duration,
) -> Result<(u16, String), ProbeFailure> {
    let url = endpoint(path).map_err(|_| ProbeFailure::Unreachable)?;
    match authorized_get_with_timeout(&url, timeout).call() {
        Ok(response) => {
            let status = response.status().as_u16();
            response
                .into_body()
                .read_to_string()
                .map(|body| (status, body))
                .map_err(|_| ProbeFailure::BodyRead)
        }
        Err(ureq::Error::StatusCode(status)) => Ok((status, String::new())),
        Err(error) => Err(classify_transport_failure(&error)),
    }
}

/// 把传输层错误归到探活的两类失败上；读体失败已在上游单独归类。
fn classify_transport_failure(error: &ureq::Error) -> ProbeFailure {
    match error {
        ureq::Error::Timeout(_) | ureq::Error::BodyStalled => ProbeFailure::Timeout,
        ureq::Error::Io(io_error)
            if matches!(
                io_error.kind(),
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
            ) =>
        {
            ProbeFailure::Timeout
        }
        _ => ProbeFailure::Unreachable,
    }
}

/// GET 一个绝对路径接口，要求 2xx 并把响应解析为 JSON。
pub fn get_json_at(path: &str) -> Result<serde_json::Value, String> {
    let url = endpoint_at(path)?;
    let (status, body) = match authorized_get(&url).call() {
        Ok(response) => {
            let status = response.status().as_u16();
            let body = response
                .into_body()
                .read_to_string()
                .map_err(|error| format!("读取网关响应失败：{error}"))?;
            (status, body)
        }
        Err(ureq::Error::StatusCode(status)) => (status, String::new()),
        Err(error) => return Err(format!("网关不可达：{error}")),
    };
    if !(200..300).contains(&status) {
        return Err(gateway_status_message(status, &body));
    }
    serde_json::from_str(&body).map_err(|_| "网关响应不是合法 JSON".to_string())
}

/// POST 一个网关接口并解析 JSON。
pub fn post_json(path: &str, payload: &serde_json::Value) -> Result<serde_json::Value, String> {
    let url = endpoint(path)?;
    let mut request = agent()
        .post(&url)
        .header("accept", "application/json")
        .header("content-type", "application/json");
    if let Some(token) = api_token() {
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
        Err(ureq::Error::StatusCode(status)) => (status, String::new()),
        Err(error) => return Err(format!("网关不可达或拒绝请求：{error}")),
    };
    if !(200..300).contains(&status) {
        return Err(gateway_status_message(status, &body));
    }
    serde_json::from_str(&body).map_err(|_| "网关响应不是合法 JSON".to_string())
}

/// DELETE 一个网关接口并解析 JSON。
pub fn delete_json(path: &str) -> Result<serde_json::Value, String> {
    let url = endpoint(path)?;
    let mut request = agent().delete(&url).header("accept", "application/json");
    if let Some(token) = api_token() {
        request = request.header("authorization", &format!("Bearer {token}"));
    }
    let (status, body) = match request.call() {
        Ok(response) => {
            let status = response.status().as_u16();
            let body = response
                .into_body()
                .read_to_string()
                .map_err(|error| format!("读取网关响应失败：{error}"))?;
            (status, body)
        }
        Err(ureq::Error::StatusCode(status)) => (status, String::new()),
        Err(error) => return Err(format!("网关不可达或拒绝请求：{error}")),
    };
    if !(200..300).contains(&status) {
        return Err(gateway_status_message(status, &body));
    }
    serde_json::from_str(&body).map_err(|_| "网关响应不是合法 JSON".to_string())
}

/// POST 一个绝对路径接口，要求 2xx 并把响应解析为 JSON。
pub fn post_json_at(path: &str, payload: &serde_json::Value) -> Result<serde_json::Value, String> {
    let url = endpoint_at(path)?;
    let mut request = agent()
        .post(&url)
        .header("accept", "application/json")
        .header("content-type", "application/json");
    if let Some(token) = api_token() {
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
        Err(ureq::Error::StatusCode(status)) => (status, String::new()),
        Err(error) => return Err(format!("网关不可达或拒绝请求：{error}")),
    };
    if !(200..300).contains(&status) {
        return Err(gateway_status_message(status, &body));
    }
    serde_json::from_str(&body).map_err(|_| "网关响应不是合法 JSON".to_string())
}

/// 把非 2xx 响应翻译成可执行的中文说明。
///
/// 401 是凭据问题、404 是服务端不含该接口（版本过旧），二者修复动作完全不同，
/// 不能笼统报「调用失败」；网关自身的错误码原样附在后面，便于定位。
fn gateway_status_message(status: u16, body: &str) -> String {
    let code = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| {
            value.get("error").and_then(|error| {
                error.as_str().map(str::to_owned).or_else(|| {
                    error
                        .get("code")
                        .and_then(|v| v.as_str())
                        .map(str::to_owned)
                })
            })
        })
        .unwrap_or_default();
    let suffix = if code.is_empty() {
        String::new()
    } else {
        format!("（{code}）")
    };
    match status {
        // 401 是凭据问题，403 是网关入口策略拒绝了这条事件（字段、附件或时效不合规）：
        // 两者的修复动作不同，共用一句「凭据不匹配」会把内容问题报成配置问题。
        401 => format!("网关拒绝访问：凭据缺失或不匹配{suffix}"),
        403 if code == "event_rejected" => {
            format!("网关拒绝了这条消息：内容、附件或时效不合规{suffix}")
        }
        403 => format!("网关拒绝访问：凭据缺失或不匹配{suffix}"),
        404 => format!("网关不含该接口，后台服务版本可能过旧{suffix}"),
        429 => format!("网关繁忙，请稍后重试{suffix}"),
        _ => format!("网关返回状态码 {status}{suffix}"),
    }
}

#[cfg(test)]
mod tests {
    use super::gateway_status_message;

    #[test]
    fn credential_and_ingress_rejections_give_different_advice() {
        let unauthorized = gateway_status_message(401, r#"{"error":"unauthorized"}"#);
        assert!(unauthorized.contains("凭据"), "{unauthorized}");

        let rejected = gateway_status_message(403, r#"{"error":"event_rejected"}"#);
        assert!(rejected.contains("内容、附件或时效"), "{rejected}");
        assert!(!rejected.contains("凭据"), "{rejected}");
        assert!(rejected.contains("event_rejected"), "{rejected}");
    }

    #[test]
    fn preview_status_classification_follows_protocol_codes() {
        use super::{VoicePreviewError, classify_preview_status};

        let unavailable = classify_preview_status(
            409,
            r#"{"error":"provider_unavailable","message":"尚未接入云端语音提供方，请先配置阿里百炼或火山引擎"}"#,
        );
        assert!(
            matches!(unavailable, VoicePreviewError::Unavailable(ref reason) if reason.contains("尚未接入云端语音提供方")),
            "{unavailable:?}"
        );

        let timeout = classify_preview_status(504, r#"{"error":"preview_timeout"}"#);
        assert!(matches!(timeout, VoicePreviewError::Timeout));

        let invalid = classify_preview_status(400, r#"{"error":"invalid_json"}"#);
        assert!(
            matches!(invalid, VoicePreviewError::Invalid(ref reason) if !reason.is_empty()),
            "{invalid:?}"
        );

        // 没有服务端文案时按错误码兜底，不把空串交给界面。
        let fallback = classify_preview_status(409, r#"{"error":"tts_failed"}"#);
        assert!(
            matches!(fallback, VoicePreviewError::Unavailable(ref reason) if reason.contains("朗读失败")),
            "{fallback:?}"
        );
    }
}

/// 语音合成失败分类：409 归为运行时不可用，其余归为请求失败。
#[derive(Debug)]
pub enum VoiceTtsError {
    /// 语音提供方或音色未就绪。
    Unavailable(String),
    /// 其它失败。
    Failed(String),
}

impl std::fmt::Display for VoiceTtsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(reason) | Self::Failed(reason) => f.write_str(reason),
        }
    }
}

/// 调用网关合成一段朗读音频，返回 WAV 字节。
///
/// # Errors
///
/// 网关不可达、鉴权失败、语音提供方未就绪或合成失败。
pub fn post_voice_tts(
    text: &str,
    preset_id: Option<&str>,
    persona: Option<&serde_json::Value>,
) -> Result<Vec<u8>, VoiceTtsError> {
    let url = endpoint("/voice/tts").map_err(VoiceTtsError::Failed)?;
    let mut request = agent_with_timeout(VOICE_TTS_TIMEOUT)
        .post(&url)
        .header("accept", "audio/wav")
        .header("content-type", "application/json");
    if let Some(token) = api_token() {
        request = request.header("authorization", &format!("Bearer {token}"));
    }
    let payload = serde_json::json!({
        "text": text,
        "presetId": preset_id.unwrap_or(""),
        "persona": persona,
    });
    let response = match request.send(payload.to_string()) {
        Ok(response) => response,
        Err(ureq::Error::StatusCode(status)) => {
            return Err(classify_voice_status(status, String::new()));
        }
        Err(ureq::Error::Timeout(_)) => {
            return Err(VoiceTtsError::Failed(
                "朗读合成超时：这段文字较长，稍后重试或缩短内容".to_owned(),
            ));
        }
        Err(error) => return Err(VoiceTtsError::Failed(format!("网关不可达：{error}"))),
    };
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        let body = response.into_body().read_to_string().unwrap_or_default();
        return Err(classify_voice_status(status, body));
    }
    let mut body = response.into_body();
    body.read_to_vec()
        .map_err(|error| VoiceTtsError::Failed(format!("读取音频失败：{error}")))
}

fn classify_voice_status(status: u16, body: String) -> VoiceTtsError {
    let reason = gateway_status_message(status, &body);
    if status == 409 || status == 503 {
        VoiceTtsError::Unavailable(reason)
    } else {
        VoiceTtsError::Failed(reason)
    }
}

/// 读取音色预设清单。
///
/// # Errors
///
/// 网关不可达、鉴权失败或响应不是合法 JSON。
pub fn get_voice_presets() -> Result<serde_json::Value, String> {
    get_json("/voice/presets")
}

/// 读取语音提供方状态：是否可用、降级原因与预设数量。
///
/// 与 `/api/v1/voice` 建连时给出的 `ready` 共用同一判据，因此「点麦克风之前」
/// 就能知道本地语音是否装好，不必先等一次冷启动失败。
///
/// # Errors
///
/// 网关不可达、鉴权失败或响应不是合法 JSON。
pub fn get_voice_runtime_status() -> Result<serde_json::Value, String> {
    get_json("/voice/status")
}

/// 试听音频：WAV 字节与网关实际采用的音色。
pub struct VoicePreviewAudio {
    /// WAV 字节。
    pub bytes: Vec<u8>,
    /// 网关响应头给出的实际音色；缺失时为空串。
    pub preset_id: String,
}

/// 试听失败分类：超时、不可用、参数非法与其它失败各有各的文案。
#[derive(Debug)]
pub enum VoicePreviewError {
    /// 504：合成超时。
    Timeout,
    /// 409：语音提供方或音色不可用。
    Unavailable(String),
    /// 400：请求体不合法。
    Invalid(String),
    /// 其它失败。
    Failed(String),
}

impl std::fmt::Display for VoicePreviewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout => f.write_str("试听超时，请重试或换一个音色"),
            Self::Unavailable(reason) | Self::Invalid(reason) | Self::Failed(reason) => {
                f.write_str(reason)
            }
        }
    }
}

/// 调用网关合成一句试听，返回 WAV 字节。
///
/// # Errors
///
/// 网关不可达、鉴权失败、语音提供方未就绪或合成超时。
pub fn post_voice_preview(
    preset_id: Option<&str>,
    persona: Option<&serde_json::Value>,
) -> Result<VoicePreviewAudio, VoicePreviewError> {
    let url = endpoint("/voice/preview").map_err(VoicePreviewError::Failed)?;
    let mut request = agent_with_timeout(VOICE_PREVIEW_TIMEOUT)
        .post(&url)
        .header("accept", "audio/wav")
        .header("content-type", "application/json");
    if let Some(token) = api_token() {
        request = request.header("authorization", &format!("Bearer {token}"));
    }
    let payload = serde_json::json!({
        "presetId": preset_id.unwrap_or(""),
        "persona": persona,
    });
    let response = match request.send(payload.to_string()) {
        Ok(response) => response,
        Err(ureq::Error::StatusCode(status)) => {
            return Err(classify_preview_status(status, ""));
        }
        Err(ureq::Error::Timeout(_)) => return Err(VoicePreviewError::Timeout),
        Err(error) => return Err(VoicePreviewError::Failed(format!("网关不可达：{error}"))),
    };
    let status = response.status().as_u16();
    let preset_id = response
        .headers()
        .get("x-voice-preset")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    if !(200..300).contains(&status) {
        let body = response.into_body().read_to_string().unwrap_or_default();
        return Err(classify_preview_status(status, &body));
    }
    let mut body = response.into_body();
    let bytes = body
        .read_to_vec()
        .map_err(|error| VoicePreviewError::Failed(format!("读取试听音频失败：{error}")))?;
    Ok(VoicePreviewAudio { bytes, preset_id })
}

/// 把试听接口的非 2xx 响应翻译成可执行结论。
///
/// 409 与 400 的响应体带服务端中文文案，优先原样展示；没有文案时按错误码兜底。
fn classify_preview_status(status: u16, body: &str) -> VoicePreviewError {
    let value = serde_json::from_str::<serde_json::Value>(body).ok();
    let code = value
        .as_ref()
        .and_then(|value| value.get("error"))
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let message = value
        .as_ref()
        .and_then(|value| value.get("message"))
        .and_then(|value| value.as_str())
        .filter(|text| !text.is_empty());
    let fallback = match code {
        "provider_unavailable" => Some("尚未接入云端语音提供方，请先配置阿里百炼或火山引擎"),
        "tts_failed" => Some("朗读失败，请稍后重试"),
        _ => None,
    };
    match status {
        409 => VoicePreviewError::Unavailable(
            message
                .or(fallback)
                .unwrap_or("该音色当前不可用")
                .to_owned(),
        ),
        504 => VoicePreviewError::Timeout,
        400 => {
            VoicePreviewError::Invalid(message.or(fallback).unwrap_or("试听参数不正确").to_owned())
        }
        _ => VoicePreviewError::Failed(gateway_status_message(status, body)),
    }
}

/// 语音 WebSocket 地址：与工作台 HTTP 面同端口，凭据走查询串。
///
/// 浏览器 WebSocket 不能携带握手头，因此语音上行由宿主建立连接；
/// 地址解析与凭据读取集中在这里，命令里不另拼一份。
pub fn voice_websocket_url() -> Result<String, String> {
    let bind = config_text(KEY_HTTP_BIND).unwrap_or_else(|| FALLBACK_HTTP_BIND.to_string());
    let host = probe_target(&bind);
    let mut url = url::Url::parse(&format!("ws://{host}/api/v1/voice"))
        .map_err(|error| format!("构造语音连接地址失败：{error}"))?;
    if let Some(token) = api_token() {
        url.query_pairs_mut().append_pair("api_token", &token);
    }
    Ok(url.to_string())
}
