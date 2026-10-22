//! Feishu OpenAPI 真实执行器。
//!
//! 本模块只位于 Feishu adapter 层，统一承接 JSON、二进制与 multipart HTTP 交互。

use std::{
    collections::{HashMap, HashSet},
    io::Read,
    sync::{Condvar, Mutex, OnceLock},
    time::{Duration, Instant},
};

use axon_crypto::sha256_hex;
use axon_domain::{AxonError, AxonResult};
use axon_rustls_provider::ensure_rustls_crypto_provider;

use crate::{
    REMOTE_FEISHU_FILE_MAX_BYTES, REMOTE_FEISHU_TENANT_ACCESS_TOKEN_PATH,
    RemoteFeishuBinaryHttpResponse, RemoteFeishuHttpRequestPlan, RemoteFeishuHttpResponse,
    RemoteFeishuMultipartHttpRequest,
};

const CONTENT_TYPE_HEADER: &str = "Content-Type";
const DEFAULT_HTTP_TIMEOUT_SECS: u64 = 30;
const TOKEN_REFRESH_SKEW_SECS: u64 = 60;
const TOKEN_CACHE_MAX_ENTRIES: usize = 32;
const TEXT_RESPONSE_MAX_BYTES: usize = 1024 * 1024;
const RESPONSE_READ_BUFFER_BYTES: usize = 16 * 1024;
static HTTP_AGENT: OnceLock<ureq::Agent> = OnceLock::new();
static TOKEN_CACHE: OnceLock<TokenCache> = OnceLock::new();

#[derive(Debug, Clone)]
struct CachedTokenResponse {
    response: RemoteFeishuHttpResponse,
    valid_until: Instant,
}

#[derive(Debug, Default)]
struct TokenCacheState {
    responses: HashMap<String, CachedTokenResponse>,
    in_flight: HashSet<String>,
}

#[derive(Debug, Default)]
struct TokenCache {
    state: Mutex<TokenCacheState>,
    ready: Condvar,
}

/// Feishu OpenAPI HTTP 执行 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct RemoteFeishuOapiExecutorRuntime;

impl RemoteFeishuOapiExecutorRuntime {
    /// 执行 JSON HTTP 请求。
    pub fn execute_json(
        request: &RemoteFeishuHttpRequestPlan,
    ) -> AxonResult<RemoteFeishuHttpResponse> {
        if request
            .url
            .ends_with(REMOTE_FEISHU_TENANT_ACCESS_TOKEN_PATH)
        {
            return execute_token_request(request);
        }
        execute_json_uncached(request)
    }

    /// 以调用方指定的截止时间执行单次 JSON 请求。
    ///
    /// 实时进度属于可降级投影，不能被共享 token 刷新或全局 delivery timeout
    /// 无限阻塞；因此这里刻意不进入 token cache 的等待路径。
    pub fn execute_json_with_timeout(
        request: &RemoteFeishuHttpRequestPlan,
        timeout: Duration,
    ) -> AxonResult<RemoteFeishuHttpResponse> {
        execute_json_with_agent(request, ureq_agent_with_timeout(timeout))
    }

    /// 执行二进制下载请求。
    pub fn execute_binary(
        request: &RemoteFeishuHttpRequestPlan,
    ) -> AxonResult<RemoteFeishuBinaryHttpResponse> {
        Self::execute_binary_with_agent(request, ureq_agent())
    }

    /// 以调用方指定的截止时间执行二进制下载请求。
    ///
    /// 入站附件属于消息增强而非消息本体：必须能被短上界截断，否则单个大附件会用
    /// 全局 30s 超时占满调用方的入站主链。
    pub fn execute_binary_with_timeout(
        request: &RemoteFeishuHttpRequestPlan,
        timeout: Duration,
    ) -> AxonResult<RemoteFeishuBinaryHttpResponse> {
        Self::execute_binary_with_agent(request, ureq_agent_with_timeout(timeout))
    }

    fn execute_binary_with_agent(
        request: &RemoteFeishuHttpRequestPlan,
        agent: ureq::Agent,
    ) -> AxonResult<RemoteFeishuBinaryHttpResponse> {
        if request.method != "GET" {
            return Err(AxonError::runtime_failure(
                "Feishu OpenAPI 二进制请求方法不支持",
            ));
        }
        let mut req = agent.get(&request.url);
        for (name, value) in &request.headers {
            req = req.header(name, value);
        }
        let mut response = req
            .call()
            .map_err(|_| AxonError::runtime_failure("Feishu OpenAPI 二进制请求失败"))?;
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get(CONTENT_TYPE_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body = read_response_bytes(
            response.body_mut().as_reader(),
            REMOTE_FEISHU_FILE_MAX_BYTES,
            "Feishu OpenAPI 二进制响应超过大小上限",
            "Feishu OpenAPI 二进制响应不可读",
        )?;
        Ok(RemoteFeishuBinaryHttpResponse {
            status,
            body,
            content_type,
        })
    }

    /// 执行 multipart 上传请求。
    pub fn execute_multipart(
        request: &RemoteFeishuMultipartHttpRequest,
    ) -> AxonResult<RemoteFeishuHttpResponse> {
        let agent = ureq_agent();
        let mut req = agent.post(&request.url);
        for (name, value) in &request.headers {
            req = req.header(name, value);
        }
        match req.send(request.body.as_slice()) {
            Ok(resp) => read_text_response(resp),
            Err(error) => Err(AxonError::runtime_failure(format!(
                "Feishu multipart 上传传输失败：{error}"
            ))),
        }
    }
}

fn ureq_agent() -> ureq::Agent {
    ensure_rustls_crypto_provider();
    HTTP_AGENT
        .get_or_init(|| {
            ureq::Agent::config_builder()
                .timeout_global(Some(timeout()))
                .http_status_as_error(false)
                .build()
                .into()
        })
        .clone()
}

fn ureq_agent_with_timeout(timeout: Duration) -> ureq::Agent {
    ensure_rustls_crypto_provider();
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout.max(Duration::from_millis(1))))
        .http_status_as_error(false)
        .build()
        .into()
}

fn read_text_response(
    mut response: ureq::http::Response<ureq::Body>,
) -> AxonResult<RemoteFeishuHttpResponse> {
    Ok(RemoteFeishuHttpResponse {
        status: response.status().as_u16(),
        body: read_response_text(response.body_mut().as_reader())?,
    })
}

fn read_response_text(reader: impl Read) -> AxonResult<String> {
    String::from_utf8(read_response_bytes(
        reader,
        TEXT_RESPONSE_MAX_BYTES,
        "Feishu OpenAPI 文本响应超过大小上限",
        "Feishu OpenAPI 文本响应不可读",
    )?)
    .map_err(|_| AxonError::runtime_failure("Feishu OpenAPI 文本响应不可读"))
}

fn read_response_bytes(
    mut reader: impl Read,
    max_bytes: usize,
    too_large_message: &'static str,
    unreadable_message: &'static str,
) -> AxonResult<Vec<u8>> {
    let mut body = Vec::with_capacity(max_bytes.min(RESPONSE_READ_BUFFER_BYTES));
    let mut buffer = [0_u8; RESPONSE_READ_BUFFER_BYTES];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|_| AxonError::runtime_failure(unreadable_message))?;
        if read == 0 {
            return Ok(body);
        }
        if body.len().saturating_add(read) > max_bytes {
            return Err(AxonError::runtime_failure(too_large_message));
        }
        body.extend_from_slice(&buffer[..read]);
    }
}

fn timeout() -> Duration {
    Duration::from_secs(DEFAULT_HTTP_TIMEOUT_SECS)
}

fn execute_token_request(
    request: &RemoteFeishuHttpRequestPlan,
) -> AxonResult<RemoteFeishuHttpResponse> {
    execute_token_request_with(request, execute_json_uncached)
}

fn execute_token_request_with(
    request: &RemoteFeishuHttpRequestPlan,
    execute: impl FnOnce(&RemoteFeishuHttpRequestPlan) -> AxonResult<RemoteFeishuHttpResponse>,
) -> AxonResult<RemoteFeishuHttpResponse> {
    let cache_key = token_cache_key(request)?;
    let cache = TOKEN_CACHE.get_or_init(TokenCache::default);
    loop {
        let mut state = cache
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        prune_expired_token_responses(&mut state);
        if let Some(response) = cached_token_response(&mut state, &cache_key) {
            return Ok(response);
        }
        if state.in_flight.insert(cache_key.clone()) {
            break;
        }
        drop(
            cache
                .ready
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
    }

    let response = execute(request);
    let mut state = cache
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    state.in_flight.remove(&cache_key);
    if let Ok(response) = &response
        && let Some(valid_until) = token_valid_until(response)
    {
        insert_cached_token_response(&mut state, cache_key, response.clone(), valid_until);
    }
    cache.ready.notify_all();
    response
}

fn execute_json_uncached(
    request: &RemoteFeishuHttpRequestPlan,
) -> AxonResult<RemoteFeishuHttpResponse> {
    execute_json_with_agent(request, ureq_agent())
}

fn execute_json_with_agent(
    request: &RemoteFeishuHttpRequestPlan,
    agent: ureq::Agent,
) -> AxonResult<RemoteFeishuHttpResponse> {
    let response = match request.method.as_str() {
        "POST" => {
            let mut req = agent.post(&request.url);
            for (name, value) in &request.headers {
                req = req.header(name, value);
            }
            req.send(&request.body_json)
        }
        "PATCH" => {
            let mut req = agent.patch(&request.url);
            for (name, value) in &request.headers {
                req = req.header(name, value);
            }
            req.send(&request.body_json)
        }
        "GET" => {
            let mut req = agent.get(&request.url);
            for (name, value) in &request.headers {
                req = req.header(name, value);
            }
            req.call()
        }
        "DELETE" => {
            let mut req = agent.delete(&request.url);
            for (name, value) in &request.headers {
                req = req.header(name, value);
            }
            req.call()
        }
        _ => {
            return Err(AxonError::runtime_failure(
                "Feishu OpenAPI JSON 请求方法不支持",
            ));
        }
    };
    match response {
        Ok(resp) => read_text_response(resp),
        Err(error) => Err(AxonError::runtime_failure(format!(
            "Feishu OpenAPI 传输失败：{error}"
        ))),
    }
}

fn token_cache_key(request: &RemoteFeishuHttpRequestPlan) -> AxonResult<String> {
    let value = serde_json::from_str::<serde_json::Value>(&request.body_json)
        .map_err(|_| AxonError::runtime_failure("Feishu token 请求 JSON 无效"))?;
    let app_id = value
        .get("app_id")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or(AxonError::runtime_failure("Feishu token 请求缺少 app_id"))?;
    let app_secret = value
        .get("app_secret")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or(AxonError::runtime_failure(
            "Feishu token 请求缺少 app_secret",
        ))?;
    Ok(format!(
        "{}|{}|{}",
        request.url,
        app_id,
        sha256_hex(app_secret)
    ))
}

fn cached_token_response(
    state: &mut TokenCacheState,
    cache_key: &str,
) -> Option<RemoteFeishuHttpResponse> {
    let entry = state.responses.get(cache_key)?;
    if Instant::now() < entry.valid_until {
        return Some(entry.response.clone());
    }
    state.responses.remove(cache_key);
    None
}

fn prune_expired_token_responses(state: &mut TokenCacheState) {
    let now = Instant::now();
    state.responses.retain(|_, entry| entry.valid_until > now);
}

fn insert_cached_token_response(
    state: &mut TokenCacheState,
    cache_key: String,
    response: RemoteFeishuHttpResponse,
    valid_until: Instant,
) {
    prune_expired_token_responses(state);
    if !state.responses.contains_key(&cache_key)
        && state.responses.len() >= TOKEN_CACHE_MAX_ENTRIES
        && let Some(evicted_key) = state
            .responses
            .iter()
            .min_by_key(|(_, entry)| entry.valid_until)
            .map(|(key, _)| key.clone())
    {
        state.responses.remove(&evicted_key);
    }
    state.responses.insert(
        cache_key,
        CachedTokenResponse {
            response,
            valid_until,
        },
    );
}

fn token_valid_until(response: &RemoteFeishuHttpResponse) -> Option<Instant> {
    if response.status != 200 {
        return None;
    }
    let value = serde_json::from_str::<serde_json::Value>(&response.body).ok()?;
    if value.get("code").and_then(serde_json::Value::as_i64) != Some(0)
        || value
            .get("tenant_access_token")
            .and_then(serde_json::Value::as_str)
            .is_none_or(str::is_empty)
    {
        return None;
    }
    let expires_in = value
        .get("expire")
        .and_then(serde_json::Value::as_u64)?
        .saturating_sub(TOKEN_REFRESH_SKEW_SECS);
    (expires_in > 0).then(|| Instant::now() + Duration::from_secs(expires_in))
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Barrier,
        atomic::{AtomicUsize, Ordering},
    };

    use super::*;

    struct FailingReader;

    impl std::io::Read for FailingReader {
        fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("read failure"))
        }
    }

    #[test]
    fn unreadable_text_response_is_propagated() {
        assert_eq!(
            read_response_text(FailingReader).expect_err("读取失败必须传播"),
            AxonError::runtime_failure("Feishu OpenAPI 文本响应不可读")
        );
    }

    #[test]
    fn oversized_text_response_is_rejected_before_allocation_grows_unbounded() {
        let body = vec![b'x'; TEXT_RESPONSE_MAX_BYTES + 1];

        assert_eq!(
            read_response_text(body.as_slice()).expect_err("超限响应必须拒绝"),
            AxonError::runtime_failure("Feishu OpenAPI 文本响应超过大小上限")
        );
    }

    #[test]
    fn token_cache_key_isolates_secret_without_exposing_it() {
        let request = RemoteFeishuHttpRequestPlan {
            dispatch_id: "token".to_owned(),
            url: format!("https://open.feishu.cn{REMOTE_FEISHU_TENANT_ACCESS_TOKEN_PATH}"),
            method: "POST".to_owned(),
            headers: Vec::new(),
            body_json: serde_json::json!({
                "app_id": "cli_xxx",
                "app_secret": "sensitive-secret",
            })
            .to_string(),
        };

        let key = token_cache_key(&request).expect("cache key");

        assert!(key.contains("cli_xxx"));
        assert!(!key.contains("sensitive-secret"));
    }

    #[test]
    fn token_cache_uses_expire_with_refresh_skew() {
        let before = Instant::now();
        let valid_until = token_valid_until(&RemoteFeishuHttpResponse {
            status: 200,
            body: serde_json::json!({
                "code": 0,
                "tenant_access_token": "token",
                "expire": 7_200,
            })
            .to_string(),
        })
        .expect("valid token");

        let remaining = valid_until.duration_since(before).as_secs();
        assert!((7_139..=7_140).contains(&remaining));
    }

    #[test]
    fn token_cache_prunes_expired_entries_and_caps_live_entries() {
        let now = Instant::now();
        let mut state = TokenCacheState::default();
        state.responses.insert(
            "expired".to_owned(),
            CachedTokenResponse {
                response: RemoteFeishuHttpResponse {
                    status: 200,
                    body: "expired".to_owned(),
                },
                valid_until: now
                    .checked_sub(Duration::from_secs(1))
                    .expect("当前时间必须晚于一秒"),
            },
        );

        for index in 0..=TOKEN_CACHE_MAX_ENTRIES {
            insert_cached_token_response(
                &mut state,
                format!("live-{index}"),
                RemoteFeishuHttpResponse {
                    status: 200,
                    body: format!("token-{index}"),
                },
                now + Duration::from_secs(u64::try_from(index + 1).expect("seconds")),
            );
        }

        assert_eq!(state.responses.len(), TOKEN_CACHE_MAX_ENTRIES);
        assert!(!state.responses.contains_key("expired"));
        assert!(!state.responses.contains_key("live-0"));
        assert!(
            state
                .responses
                .contains_key(&format!("live-{TOKEN_CACHE_MAX_ENTRIES}"))
        );
    }

    #[test]
    fn concurrent_token_requests_share_one_in_flight_fetch() {
        const CALLER_COUNT: usize = 8;
        const FETCH_DELAY_MS: u64 = 50;

        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let request = RemoteFeishuHttpRequestPlan {
            dispatch_id: "token-single-flight".to_owned(),
            url: format!(
                "https://single-flight-{suffix}.invalid{REMOTE_FEISHU_TENANT_ACCESS_TOKEN_PATH}"
            ),
            method: "POST".to_owned(),
            headers: Vec::new(),
            body_json: serde_json::json!({
                "app_id": format!("cli_{suffix}"),
                "app_secret": format!("secret_{suffix}"),
            })
            .to_string(),
        };
        let barrier = Arc::new(Barrier::new(CALLER_COUNT));
        let fetch_count = Arc::new(AtomicUsize::new(0));
        let handles = (0..CALLER_COUNT)
            .map(|_| {
                let request = request.clone();
                let barrier = Arc::clone(&barrier);
                let fetch_count = Arc::clone(&fetch_count);
                std::thread::spawn(move || {
                    barrier.wait();
                    execute_token_request_with(&request, |_| {
                        fetch_count.fetch_add(1, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(FETCH_DELAY_MS));
                        Ok(RemoteFeishuHttpResponse {
                            status: 200,
                            body: serde_json::json!({
                                "code": 0,
                                "tenant_access_token": "shared-token",
                                "expire": 7_200,
                            })
                            .to_string(),
                        })
                    })
                    .expect("token response")
                })
            })
            .collect::<Vec<_>>();

        let responses = handles
            .into_iter()
            .map(|handle| handle.join().expect("caller"))
            .collect::<Vec<_>>();

        assert_eq!(fetch_count.load(Ordering::SeqCst), 1);
        assert!(
            responses
                .iter()
                .all(|response| response.body.contains("shared-token"))
        );
    }
}
