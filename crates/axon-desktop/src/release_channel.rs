//! GitHub 发布渠道：仓库坐标、关于面板要展示的真实信息，以及更新检测。
//!
//! 仓库坐标只有这一份来源：`CARGO_PKG_REPOSITORY` 与 `CARGO_PKG_LICENSE` 由构建期
//! 从工作区元数据写入，仓库主页、发布页、问题反馈与许可全部由它派生，
//! 因此不存在「界面写一个地址、实际发布在另一个地址」这种漂移。
//!
//! 更新检测是桌面端唯一一处对外查询，因此失败也按一等结果处理：每个 HTTP 状态都
//! 翻成可执行的中文原因与一句下一步动作，界面拿不到裸状态码。仓库是私有仓库，
//! 匿名请求读不到发布信息，凭据沿用配置相邻 `.env` 这一条既有通道（与供应商密钥
//! 同源）；未配置时不伪造结论，只把「缺凭据」讲清楚，并在必要时指出配置方式。

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axon_config::ConfigLoader;
use serde::{Deserialize, Serialize};

/// 仓库地址，取自构建期元数据。
const REPOSITORY_URL: &str = env!("CARGO_PKG_REPOSITORY");
/// 开源许可标识，取自构建期元数据。
const LICENSE: &str = env!("CARGO_PKG_LICENSE");
/// GitHub 接口根地址。
const API_ROOT: &str = "https://api.github.com";
/// 单次查询的整体超时，避免界面被不可达端点挂住。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(12);
/// 访问令牌的候选环境变量名，按顺序取第一个非空值。
const TOKEN_ENV_NAMES: [&str; 3] = ["AXON_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"];
/// 未认证请求的每小时接口额度；写进提示里才能把「限流」讲成具体数字。
const ANONYMOUS_HOURLY_QUOTA: u64 = 60;
/// 已认证请求的每小时接口额度。
const AUTHENTICATED_HOURLY_QUOTA: u64 = 5000;
/// 转述 GitHub 自带说明时的长度上限。
const MESSAGE_LIMIT: usize = 160;

/// GitHub 渠道信息：关于页展示的全部真实坐标。
///
/// 只放产品页面上真正要展示的事实。访问令牌这类操作项不进这里：它在界面上
/// 没有任何一席之地，只在检测失败的详情里被提到。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GithubInfoDto {
    /// `owner/name` 形态的仓库标识。
    pub repo: String,
    /// 仓库主页。
    pub repository_url: String,
    /// 发布页。
    pub releases_url: String,
    /// 问题反馈页。
    pub issues_url: String,
    /// 开源许可标识。
    pub license: String,
}

/// 更新检查结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateCheckDto {
    /// 当前版本。
    pub current: String,
    /// 远端最新版本；查询失败时为空。
    pub latest: String,
    /// 是否存在可用更新。
    pub update_available: bool,
    /// 发布页地址。
    pub release_url: String,
    /// 首个安装包下载地址；没有资产时为空。
    pub download_url: String,
    /// 发布时间。
    pub published_at: String,
    /// 失败原因；成功时为空。
    pub error: String,
    /// 失败时的下一步动作；成功时为空。
    pub hint: String,
}

/// 一次失败的归因：一行原因，加一句下一步动作。
struct Failure {
    reason: String,
    hint: String,
}

impl Failure {
    /// 组装一条归因。
    fn new(reason: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            hint: hint.into(),
        }
    }
}

/// 当前仓库信息。
pub fn github_info() -> GithubInfoDto {
    let repository_url = REPOSITORY_URL
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .to_string();
    let repo = repository_url
        .split_once("github.com/")
        .map(|(_, tail)| tail.to_string())
        .unwrap_or_default();
    GithubInfoDto {
        releases_url: format!("{repository_url}/releases"),
        issues_url: format!("{repository_url}/issues"),
        license: LICENSE.to_string(),
        repo,
        repository_url,
    }
}

/// Tauri IPC 命令：查询最新发布版本。
///
/// 网络失败不是命令失败：结论连同归因一起回给界面，因此界面永远拿不到
/// 「http status: 403」这种东西，首屏也只渲染产品口径的一句话。
#[tauri::command]
pub fn check_update() -> Result<UpdateCheckDto, String> {
    let current = env!("CARGO_PKG_VERSION").to_string();
    let info = github_info();
    let mut result = UpdateCheckDto {
        current: current.clone(),
        latest: String::new(),
        update_available: false,
        release_url: info.releases_url,
        download_url: String::new(),
        published_at: String::new(),
        error: String::new(),
        hint: String::new(),
    };

    let token = access_token();
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(REQUEST_TIMEOUT))
        // 非 2xx 也把响应交回来：限流额度与重置时间都在响应头上，
        // 直接当异常抛出就只剩一个状态码，讲不清「为什么失败、下一步做什么」。
        .http_status_as_error(false)
        .build()
        .new_agent();

    let mut request = agent
        .get(&format!("{API_ROOT}/repos/{}/releases/latest", info.repo))
        .header("accept", "application/vnd.github+json")
        .header("x-github-api-version", "2022-11-28")
        .header("user-agent", "axon-desktop");
    if let Some(secret) = token.as_deref() {
        request = request.header("authorization", format!("Bearer {secret}"));
    }

    let mut response = match request.call() {
        Ok(value) => value,
        Err(error) => {
            let failure = transport_failure(&error);
            result.error = failure.reason;
            result.hint = failure.hint;
            return Ok(result);
        }
    };

    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        // 顺序有讲究：状态、限流头都是响应头上的事实，必须在读正文之前取下。
        let remaining = header_u64(response.headers(), "x-ratelimit-remaining");
        let reset_in = reset_countdown(response.headers());
        let detail = response.body_mut().read_to_string().unwrap_or_default();
        let failure = status_failure(status, remaining, reset_in, &detail, token.is_some());
        result.error = failure.reason;
        result.hint = failure.hint;
        return Ok(result);
    }

    let body = match response.body_mut().read_to_string() {
        Ok(text) => text,
        Err(error) => {
            result.error = format!("读取发布信息失败：{error}");
            result.hint = "稍后重试".to_string();
            return Ok(result);
        }
    };
    let payload: serde_json::Value = match serde_json::from_str(&body) {
        Ok(value) => value,
        Err(_) => {
            result.error = "发布信息不是合法 JSON".to_string();
            result.hint = "稍后重试；持续失败说明接口返回被中间层改写".to_string();
            return Ok(result);
        }
    };

    result.latest = payload
        .get("tag_name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    result.release_url = payload
        .get("html_url")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(&result.release_url)
        .to_string();
    result.published_at = payload
        .get("published_at")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    result.download_url = payload
        .get("assets")
        .and_then(serde_json::Value::as_array)
        .and_then(|assets| {
            assets
                .iter()
                .filter_map(|asset| {
                    asset
                        .get("browser_download_url")
                        .and_then(serde_json::Value::as_str)
                })
                .find(|url| url.ends_with(".msi") || url.ends_with(".zip"))
                .map(ToString::to_string)
        })
        .unwrap_or_default();

    if !result.latest.is_empty() {
        result.update_available = is_newer(&result.latest, &current);
    }
    Ok(result)
}

/// 可选的 GitHub 访问令牌。
///
/// 与供应商密钥同源：先把配置相邻 `.env` 载入进程凭据仓库，再按候选变量名取值，
/// 因此令牌只存在一处，桌面端不另建存储；调用方 shell 显式设置的同名变量优先。
fn access_token() -> Option<String> {
    if let Some(path) = config_env_path() {
        let _ = axon_config::load_provider_credentials_from_env_file(&path);
    }
    TOKEN_ENV_NAMES
        .iter()
        .find_map(|name| axon_config::provider_credential(name))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// 配置相邻的 `.env` 路径。
fn config_env_path() -> Option<PathBuf> {
    let config = ConfigLoader::default_user_config_path().ok()?;
    Some(config.parent()?.join(".env"))
}

/// 传输层失败：与状态码无关的连不上、超时这一类原因。
fn transport_failure(error: &ureq::Error) -> Failure {
    match error {
        ureq::Error::Timeout(_) => Failure::new(
            "连接 GitHub 超时",
            "检查本机网络与代理后重试；部分网络无法直连 GitHub",
        ),
        ureq::Error::HostNotFound => {
            Failure::new("无法解析 api.github.com", "检查本机 DNS 与网络连通性后重试")
        }
        ureq::Error::ConnectionFailed | ureq::Error::Io(_) => Failure::new(
            "无法连接 GitHub",
            "检查本机网络与代理后重试；部分网络无法直连 GitHub",
        ),
        other => Failure::new(
            format!("连接 GitHub 失败：{other}"),
            "检查本机网络与代理后重试",
        ),
    }
}

/// 状态码失败：把限流额度、令牌归属与仓库可见性翻成可排查的结论。
///
/// 这一层是「详情」里的内容，不是首屏文案：首屏统一是产品口径的一句话，
/// 因此这里可以说得具体，但仍然不出现任何裸状态码或库内部字符串。
fn status_failure(
    status: u16,
    remaining: Option<u64>,
    reset_in: Option<Duration>,
    body: &str,
    token_configured: bool,
) -> Failure {
    let detail = json_message(body);
    match status {
        401 => Failure::new(
            "GitHub 拒绝了访问令牌",
            "令牌无效或已过期，请在配置目录的 .env 中更新 AXON_GITHUB_TOKEN",
        ),
        403 | 429 if remaining == Some(0) => {
            Failure::new("GitHub 接口访问频率已达上限", quota_hint(reset_in))
        }
        403 | 429 => Failure::new(
            with_detail("GitHub 拒绝了本次查询", status, &detail),
            "确认访问令牌是否具备读取该仓库的权限",
        ),
        404 => Failure::new(
            "发布渠道没有可用的发布信息",
            if token_configured {
                "该仓库尚无正式发布版本，或令牌无权访问该仓库"
            } else {
                "该仓库尚无正式发布版本；私有仓库需要在配置目录的 .env 中配置 \
                 AXON_GITHUB_TOKEN"
            },
        ),
        _ if (500..600).contains(&status) => Failure::new(
            format!("GitHub 服务暂时不可用（HTTP {status}）"),
            "稍后重试",
        ),
        other => Failure::new(
            with_detail("GitHub 返回了未预期的结果", other, &detail),
            "稍后重试；持续失败可到 GitHub 状态页确认服务是否正常",
        ),
    }
}

/// 限流提示：先给额度事实与重置倒计时，再给提升额度的动作。
fn quota_hint(reset_in: Option<Duration>) -> String {
    let quota = format!("未认证请求的额度为每小时 {ANONYMOUS_HOURLY_QUOTA} 次");
    let advice = format!(
        "在配置目录的 .env 中配置 AXON_GITHUB_TOKEN 可提升到每小时 \
         {AUTHENTICATED_HOURLY_QUOTA} 次"
    );
    match reset_in {
        Some(left) => format!("{quota}，约 {}后可重试；{advice}", humanize_wait(left)),
        None => format!("{quota}，请稍后重试；{advice}"),
    }
}

/// 距离额度重置的可读时长。
fn humanize_wait(left: Duration) -> String {
    let seconds = left.as_secs();
    if seconds < 60 {
        return format!("{seconds} 秒");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes} 分钟");
    }
    format!("{} 小时", minutes / 60)
}

/// 读取一个整数响应头。
fn header_u64(headers: &ureq::http::HeaderMap, name: &str) -> Option<u64> {
    headers.get(name)?.to_str().ok()?.trim().parse().ok()
}

/// 距离额度重置还剩多久。
///
/// `Retry-After` 优先：服务端已经算好了等待秒数；它缺失或是 HTTP 日期形态时，
/// 回落到 `X-RateLimit-Reset` 给出的重置时刻。
fn reset_countdown(headers: &ureq::http::HeaderMap) -> Option<Duration> {
    if let Some(seconds) = header_u64(headers, "retry-after") {
        return Some(Duration::from_secs(seconds));
    }
    let reset_at = header_u64(headers, "x-ratelimit-reset")?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    Some(Duration::from_secs(reset_at.saturating_sub(now)))
}

/// GitHub 错误响应体里的 `message`：比状态码具体，能直接说清原因。
fn json_message(body: &str) -> String {
    let message = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("message")
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string)
        })
        .unwrap_or_default();
    let trimmed = message.trim();
    if trimmed.chars().count() <= MESSAGE_LIMIT {
        return trimmed.to_string();
    }
    let kept: String = trimmed.chars().take(MESSAGE_LIMIT).collect();
    format!("{kept}…")
}

/// 把 GitHub 自带说明拼进原因；没有说明时只报状态码。
fn with_detail(prefix: &str, status: u16, detail: &str) -> String {
    if detail.is_empty() {
        format!("{prefix}（HTTP {status}）")
    } else {
        format!("{prefix}：{detail}")
    }
}

/// 把 `v1.2.3` 形态的版本拆成可比较的数字序列。
fn version_parts(tag: &str) -> Vec<u64> {
    tag.trim_start_matches('v')
        .split(['.', '-', '+'])
        .map(|piece| piece.parse::<u64>().unwrap_or(0))
        .collect()
}

/// 判断 latest 是否比 current 更新。
fn is_newer(latest: &str, current: &str) -> bool {
    let left = version_parts(latest);
    let right = version_parts(current);
    for index in 0..left.len().max(right.len()) {
        let a = left.get(index).copied().unwrap_or(0);
        let b = right.get(index).copied().unwrap_or(0);
        if a != b {
            return a > b;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::{
        ANONYMOUS_HOURLY_QUOTA, API_ROOT, AUTHENTICATED_HOURLY_QUOTA, Duration, Failure,
        REPOSITORY_URL, github_info, is_newer, json_message, quota_hint, status_failure,
        transport_failure, version_parts,
    };

    #[test]
    fn version_comparison_handles_tags_and_lengths() {
        assert_eq!(version_parts("v1.2.3"), vec![1, 2, 3]);
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("1.0.1", "1.0"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("v0.0.9", "0.1.0"));
    }

    #[test]
    fn github_info_derives_every_entry_from_the_build_metadata() {
        let info = github_info();
        assert_eq!(info.repository_url, REPOSITORY_URL);
        assert_eq!(
            info.repository_url,
            "https://github.com/a574676848/axon-code"
        );
        assert_eq!(info.repo, "a574676848/axon-code");
        assert_eq!(info.releases_url, format!("{REPOSITORY_URL}/releases"));
        assert_eq!(info.issues_url, format!("{REPOSITORY_URL}/issues"));
        assert_eq!(info.license, "MIT");
        assert!(API_ROOT.starts_with("https://"));
    }

    #[test]
    fn rate_limited_status_keeps_the_status_code_out_of_the_reason() {
        let failure = status_failure(403, Some(0), Some(Duration::from_secs(600)), "{}", false);
        // 首屏只显示产品口径的一句话，因此原因行本身要短、要能直接读；
        // 额度事实与下一步动作放在提示里。
        assert_eq!(failure.reason, "GitHub 接口访问频率已达上限");
        assert!(!failure.reason.contains("403"));
        assert!(failure.hint.contains(&ANONYMOUS_HOURLY_QUOTA.to_string()));
        assert!(failure.hint.contains("10 分钟"));
        assert!(
            failure
                .hint
                .contains(&AUTHENTICATED_HOURLY_QUOTA.to_string())
        );
    }

    #[test]
    fn forbidden_without_exhaustion_keeps_the_github_explanation() {
        let body = r#"{"message":"Resource not accessible by personal access token"}"#;
        let failure = status_failure(403, Some(41), None, body, true);
        assert!(failure.reason.contains("Resource not accessible"));
        assert!(!failure.reason.contains("http status"));
    }

    #[test]
    fn missing_release_explains_the_private_repository_path() {
        let anonymous = status_failure(404, None, None, r#"{"message":"Not Found"}"#, false);
        assert!(anonymous.hint.contains("AXON_GITHUB_TOKEN"));
        let authorized = status_failure(404, None, None, "", true);
        assert!(!authorized.hint.contains("AXON_GITHUB_TOKEN"));
    }

    #[test]
    fn server_errors_and_unknown_statuses_stay_actionable() {
        let outage = status_failure(502, None, None, "", false);
        assert!(outage.reason.contains("502"));
        assert_eq!(outage.hint, "稍后重试");

        let unknown = status_failure(418, None, None, "", false);
        assert!(unknown.reason.contains("418"));
        assert!(!unknown.hint.is_empty());
    }

    #[test]
    fn quota_hint_without_reset_time_still_offers_the_next_step() {
        let hint = quota_hint(None);
        assert!(hint.contains("AXON_GITHUB_TOKEN"));
    }

    #[test]
    fn transport_failures_never_leak_the_raw_error_kind() {
        let Failure { reason, hint } = transport_failure(&ureq::Error::HostNotFound);
        assert!(reason.contains("api.github.com"));
        assert!(!hint.is_empty());
    }

    #[test]
    fn json_message_truncates_overlong_explanations() {
        let long = "x".repeat(400);
        let body = format!(r#"{{"message":"{long}"}}"#);
        let message = json_message(&body);
        assert_eq!(message.chars().count(), super::MESSAGE_LIMIT + 1);
        assert!(message.ends_with('…'));
        assert_eq!(json_message("not json"), "");
    }

    #[test]
    fn every_failure_stays_free_of_library_internals() {
        // 界面上不允许出现任何库内部字符串：ureq 的 Display 就是「http status: 403」
        // 那一类文本，用户看到的必须是中文归因。
        let cases = [
            status_failure(401, None, None, "", false),
            status_failure(403, Some(0), None, "", false),
            status_failure(403, Some(12), None, "", true),
            status_failure(404, None, None, "", false),
            status_failure(502, None, None, "", false),
            status_failure(418, None, None, "", false),
            transport_failure(&ureq::Error::Timeout(ureq::Timeout::Global)),
            transport_failure(&ureq::Error::ConnectionFailed),
        ];
        for failure in cases {
            for text in [&failure.reason, &failure.hint] {
                assert!(!text.trim().is_empty());
                assert!(!text.contains("http status"), "{text}");
                assert!(!text.contains("ureq"), "{text}");
                assert!(
                    text.chars()
                        .any(|ch| ('\u{4e00}'..='\u{9fff}').contains(&ch)),
                    "归因必须是中文：{text}"
                );
            }
        }
    }
}
