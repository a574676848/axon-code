use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant as StdInstant, SystemTime},
};

use axon_agent_runtime::{
    ProviderStreamingCredentialPlan, ProviderStreamingHttpRequestPlan, RunCancelToken,
};
use reqwest::{StatusCode, header::HeaderMap};
use url::Url;

use crate::runtime_limits::PROVIDER_RATE_BUCKET_IDLE_TIMEOUT_MS;

const PROVIDER_RATE_BUCKET_CAPACITY: u32 = 4;
const PROVIDER_RATE_REFILL_MS: u64 = 250;
const PROVIDER_RATE_BUCKET_MAX_CAPACITY: u32 = 64;
const REMAINING_REQUEST_HEADERS: &[&str] =
    &["x-ratelimit-remaining-requests", "ratelimit-remaining"];
const REQUEST_LIMIT_HEADERS: &[&str] = &["x-ratelimit-limit-requests", "ratelimit-limit"];
const REQUEST_RESET_HEADERS: &[&str] = &["x-ratelimit-reset-requests", "ratelimit-reset"];

pub(super) async fn wait_for_rate_token(
    request_plan: &ProviderStreamingHttpRequestPlan,
    cancel_token: &RunCancelToken,
) -> bool {
    let scope = rate_scope(request_plan);
    loop {
        if cancel_token.is_cancelled() {
            return false;
        }
        let wait = rate_buckets().lock().map_or(
            Duration::from_millis(PROVIDER_RATE_REFILL_MS),
            |mut buckets| {
                retain_active_buckets(&mut buckets, StdInstant::now());
                buckets
                    .entry(scope.clone())
                    .or_insert_with(ProviderRateBucket::new)
                    .take_or_wait()
            },
        );
        if wait.is_zero() {
            return true;
        }
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            () = cancel_token.cancelled() => return false,
        }
    }
}

pub(super) fn observe_response(
    request_plan: &ProviderStreamingHttpRequestPlan,
    status: StatusCode,
    headers: &HeaderMap,
) -> Option<Duration> {
    let retry_after = parse_retry_after(headers);
    let remaining_requests = header_u32(headers, REMAINING_REQUEST_HEADERS);
    let request_limit = header_u32(headers, REQUEST_LIMIT_HEADERS);
    let request_reset = header_duration(headers, REQUEST_RESET_HEADERS);
    observe_rate_limit(
        request_plan,
        status == StatusCode::TOO_MANY_REQUESTS,
        remaining_requests.or_else(|| (status == StatusCode::TOO_MANY_REQUESTS).then_some(0)),
        retry_after,
        request_limit,
        request_reset,
    );
    retry_after
}

pub(super) fn parse_retry_after(headers: &HeaderMap) -> Option<Duration> {
    let value = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let retry_at = httpdate::parse_http_date(value).ok()?;
    retry_at.duration_since(SystemTime::now()).ok()
}

pub(super) fn observe_websocket_rate_limit_event(
    request_plan: &ProviderStreamingHttpRequestPlan,
    payload: &str,
) {
    let Ok(event) = serde_json::from_str::<serde_json::Value>(payload) else {
        return;
    };
    let Some(rate_limits) = event.get("rate_limits") else {
        return;
    };
    let rate_limited = rate_limits
        .get("allowed")
        .and_then(serde_json::Value::as_bool)
        == Some(false)
        || rate_limits
            .get("limit_reached")
            .and_then(serde_json::Value::as_bool)
            == Some(true);
    if event.get("type").and_then(serde_json::Value::as_str) != Some("codex.rate_limits")
        || !rate_limited
    {
        return;
    }
    let retry_after = ["primary", "secondary"]
        .into_iter()
        .filter_map(|window| rate_limits.pointer(&format!("/{window}/reset_at")))
        .filter_map(serde_json::Value::as_i64)
        .find_map(duration_until_unix_timestamp);
    observe_rate_limit(request_plan, true, Some(0), retry_after, None, None);
}

fn duration_until_unix_timestamp(timestamp: i64) -> Option<Duration> {
    let timestamp = u64::try_from(timestamp).ok()?;
    SystemTime::UNIX_EPOCH
        .checked_add(Duration::from_secs(timestamp))?
        .duration_since(SystemTime::now())
        .ok()
}

fn observe_rate_limit(
    request_plan: &ProviderStreamingHttpRequestPlan,
    rate_limited: bool,
    remaining_requests: Option<u32>,
    retry_after: Option<Duration>,
    request_limit: Option<u32>,
    request_reset: Option<Duration>,
) {
    let scope = rate_scope(request_plan);
    let mut buckets = rate_buckets()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    retain_active_buckets(&mut buckets, StdInstant::now());
    let bucket = buckets.entry(scope).or_insert_with(ProviderRateBucket::new);
    bucket.adapt(request_limit, request_reset);
    if let Some(remaining_requests) = remaining_requests {
        bucket.tokens = remaining_requests.min(bucket.capacity);
    }
    if let Some(retry_after) = retry_after.filter(|_| rate_limited || remaining_requests == Some(0))
    {
        bucket.blocked_until = StdInstant::now().checked_add(retry_after);
    }
}

struct ProviderRateBucket {
    tokens: u32,
    capacity: u32,
    refill: Duration,
    last_refill: StdInstant,
    last_used_at: StdInstant,
    blocked_until: Option<StdInstant>,
}

impl ProviderRateBucket {
    fn new() -> Self {
        let now = StdInstant::now();
        Self {
            tokens: PROVIDER_RATE_BUCKET_CAPACITY,
            capacity: PROVIDER_RATE_BUCKET_CAPACITY,
            refill: Duration::from_millis(PROVIDER_RATE_REFILL_MS),
            last_refill: now,
            last_used_at: now,
            blocked_until: None,
        }
    }

    fn idle_at(&self, now: StdInstant) -> bool {
        now.saturating_duration_since(self.last_used_at)
            >= Duration::from_millis(PROVIDER_RATE_BUCKET_IDLE_TIMEOUT_MS)
    }

    fn adapt(&mut self, request_limit: Option<u32>, request_reset: Option<Duration>) {
        let Some(request_limit) = request_limit else {
            return;
        };
        self.capacity = request_limit.clamp(1, PROVIDER_RATE_BUCKET_MAX_CAPACITY);
        self.tokens = self.tokens.min(self.capacity);
        if let Some(request_reset) = request_reset.filter(|reset| !reset.is_zero()) {
            let reset_ms = u64::try_from(request_reset.as_millis()).unwrap_or(u64::MAX);
            let refill_ms = (reset_ms / u64::from(self.capacity)).max(1);
            self.refill = Duration::from_millis(refill_ms);
        }
    }

    fn take_or_wait(&mut self) -> Duration {
        let now = StdInstant::now();
        self.last_used_at = now;
        if let Some(blocked_until) = self.blocked_until {
            if now < blocked_until {
                return blocked_until.saturating_duration_since(now);
            }
            self.blocked_until = None;
        }

        let refill = self.refill;
        let elapsed = self.last_refill.elapsed();
        let refill_units = elapsed.as_millis() / refill.as_millis();
        if refill_units > 0 {
            let refill_count = u32::try_from(refill_units).unwrap_or(u32::MAX);
            self.tokens = self.tokens.saturating_add(refill_count).min(self.capacity);
            self.last_refill = now;
        }
        if self.tokens > 0 {
            self.tokens = self.tokens.saturating_sub(1);
            Duration::ZERO
        } else {
            refill.saturating_sub(elapsed.min(refill))
        }
    }
}

fn rate_buckets() -> &'static Mutex<HashMap<String, ProviderRateBucket>> {
    static RATE_BUCKETS: OnceLock<Mutex<HashMap<String, ProviderRateBucket>>> = OnceLock::new();
    RATE_BUCKETS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn rate_scope(request_plan: &ProviderStreamingHttpRequestPlan) -> String {
    let endpoint = &request_plan.url;
    let origin = origin_scope(endpoint);
    let Some(auth_header) = request_plan.auth_header.as_ref() else {
        return origin;
    };
    format!(
        "{origin}|credential={}",
        credential_fingerprint(&auth_header.credential)
    )
}

fn origin_scope(endpoint: &str) -> String {
    let Ok(url) = Url::parse(endpoint) else {
        return endpoint.to_owned();
    };
    let Some(host) = url.host_str() else {
        return endpoint.to_owned();
    };
    match url.port_or_known_default() {
        Some(port) => format!("{}://{host}:{port}", url.scheme()),
        None => format!("{}://{host}", url.scheme()),
    }
}

fn credential_fingerprint(credential: &ProviderStreamingCredentialPlan) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    match credential {
        ProviderStreamingCredentialPlan::ApiKey(value) => value.hash(&mut hasher),
        ProviderStreamingCredentialPlan::Env { name } => name.hash(&mut hasher),
    }
    hasher.finish()
}

fn header_u32(headers: &HeaderMap, names: &[&str]) -> Option<u32> {
    names.iter().find_map(|name| {
        headers
            .get(*name)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u32>().ok())
    })
}

fn header_duration(headers: &HeaderMap, names: &[&str]) -> Option<Duration> {
    names
        .iter()
        .find_map(|name| headers.get(*name).and_then(|value| value.to_str().ok()))
        .and_then(parse_duration)
}

fn parse_duration(value: &str) -> Option<Duration> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }

    let mut value = value;
    let mut duration = Duration::ZERO;
    while !value.is_empty() {
        let digits = value.chars().take_while(char::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        let amount = value[..digits].parse::<u64>().ok()?;
        value = &value[digits..];
        let (unit, rest) = if let Some(rest) = value.strip_prefix("ms") {
            (Duration::from_millis(amount), rest)
        } else if let Some(rest) = value.strip_prefix('s') {
            (Duration::from_secs(amount), rest)
        } else if let Some(rest) = value.strip_prefix('m') {
            (Duration::from_secs(amount.checked_mul(60)?), rest)
        } else {
            let rest = value.strip_prefix('h')?;
            (Duration::from_secs(amount.checked_mul(3_600)?), rest)
        };
        duration = duration.checked_add(unit)?;
        value = rest;
    }
    Some(duration)
}

fn retain_active_buckets(buckets: &mut HashMap<String, ProviderRateBucket>, now: StdInstant) {
    buckets.retain(|_, bucket| !bucket.idle_at(now));
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_agent_runtime::{ProviderStreamingAuthHeaderPlan, ProviderStreamingHttpHeader};

    fn request_plan(
        endpoint: &str,
        credential: Option<ProviderStreamingCredentialPlan>,
    ) -> ProviderStreamingHttpRequestPlan {
        ProviderStreamingHttpRequestPlan {
            method: "POST",
            url: endpoint.to_owned(),
            headers: Vec::<ProviderStreamingHttpHeader>::new(),
            auth_header: credential.map(|credential| ProviderStreamingAuthHeaderPlan {
                credential,
                header_name: "Authorization".to_owned(),
                value_prefix: "Bearer ".to_owned(),
            }),
            body_text: "{}".to_owned(),
            server_conversation_state_allowed: false,
            remote_request_body_allowed: false,
            reasons: Vec::new(),
        }
    }

    #[test]
    fn rate_scope_groups_paths_by_origin() {
        assert_eq!(
            rate_scope(&request_plan("https://api.example.com/v1/responses", None)),
            rate_scope(&request_plan("https://api.example.com/v1/models", None))
        );
        assert_ne!(
            rate_scope(&request_plan("https://api.example.com/v1/responses", None)),
            rate_scope(&request_plan(
                "https://backup.example.com/v1/responses",
                None
            ))
        );
    }

    #[test]
    fn rate_scope_separates_credentials_sharing_an_origin() {
        let endpoint = "https://api.example.com/v1/responses";
        let first = request_plan(
            endpoint,
            Some(ProviderStreamingCredentialPlan::ApiKey("first".to_owned())),
        );
        let second = request_plan(
            endpoint,
            Some(ProviderStreamingCredentialPlan::ApiKey("second".to_owned())),
        );

        assert_ne!(rate_scope(&first), rate_scope(&second));
    }

    #[test]
    fn response_headers_adapt_bucket_capacity_and_refill() {
        let request = request_plan("https://adaptive.example/v1/responses", None);
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-ratelimit-limit-requests",
            reqwest::header::HeaderValue::from_static("10"),
        );
        headers.insert(
            "x-ratelimit-remaining-requests",
            reqwest::header::HeaderValue::from_static("7"),
        );
        headers.insert(
            "x-ratelimit-reset-requests",
            reqwest::header::HeaderValue::from_static("20s"),
        );

        observe_response(&request, StatusCode::OK, &headers);

        let scope = rate_scope(&request);
        let buckets = rate_buckets()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let bucket = buckets.get(&scope).expect("自适应限流桶");
        assert_eq!(bucket.capacity, 10);
        assert_eq!(bucket.tokens, 7);
        assert_eq!(bucket.refill, Duration::from_secs(2));
    }

    #[test]
    fn reset_header_accepts_milliseconds_seconds_and_plain_seconds() {
        assert_eq!(parse_duration("250ms"), Some(Duration::from_millis(250)));
        assert_eq!(parse_duration("2s"), Some(Duration::from_secs(2)));
        assert_eq!(parse_duration("3"), Some(Duration::from_secs(3)));
        assert_eq!(parse_duration("1m0s"), Some(Duration::from_mins(1)));
        assert_eq!(
            parse_duration("1h2m3s4ms"),
            Some(Duration::from_millis(3_723_004))
        );
    }

    #[test]
    fn retry_after_blocks_429_even_with_positive_remaining_header() {
        let request = request_plan("https://retry-after.example/v1/responses", None);
        let mut headers = HeaderMap::new();
        headers.insert(
            reqwest::header::RETRY_AFTER,
            reqwest::header::HeaderValue::from_static("1"),
        );
        headers.insert(
            "x-ratelimit-remaining-requests",
            reqwest::header::HeaderValue::from_static("1"),
        );

        observe_response(&request, StatusCode::TOO_MANY_REQUESTS, &headers);

        let scope = rate_scope(&request);
        let buckets = rate_buckets()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(buckets[&scope].blocked_until.is_some());
    }

    #[test]
    fn websocket_rate_limit_event_blocks_until_primary_reset() {
        let request = request_plan("https://websocket-limit.example/v1/responses", None);
        let reset_at = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .saturating_add(60);
        let payload = serde_json::json!({
            "type": "codex.rate_limits",
            "rate_limits": {
                "allowed": false,
                "primary": { "reset_at": reset_at },
            },
        })
        .to_string();

        observe_websocket_rate_limit_event(&request, &payload);

        let scope = rate_scope(&request);
        let buckets = rate_buckets()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let bucket = &buckets[&scope];
        assert_eq!(bucket.tokens, 0);
        assert!(bucket.blocked_until.is_some());
    }

    #[test]
    fn websocket_rate_limit_event_uses_secondary_reset_when_primary_is_missing() {
        let request = request_plan(
            "https://websocket-secondary-limit.example/v1/responses",
            None,
        );
        let reset_at = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .saturating_add(60);
        let payload = serde_json::json!({
            "type": "codex.rate_limits",
            "rate_limits": {
                "allowed": false,
                "secondary": { "reset_at": reset_at },
            },
        })
        .to_string();

        observe_websocket_rate_limit_event(&request, &payload);

        let scope = rate_scope(&request);
        let buckets = rate_buckets()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let bucket = &buckets[&scope];
        assert_eq!(bucket.tokens, 0);
        assert!(bucket.blocked_until.is_some());
    }

    #[test]
    fn websocket_rate_limit_event_without_reset_still_exhausts_bucket() {
        let request = request_plan("https://websocket-missing-reset.example/v1/responses", None);
        let payload = serde_json::json!({
            "type": "codex.rate_limits",
            "rate_limits": { "limit_reached": true },
        })
        .to_string();

        observe_websocket_rate_limit_event(&request, &payload);

        let scope = rate_scope(&request);
        let buckets = rate_buckets()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let bucket = &buckets[&scope];
        assert_eq!(bucket.tokens, 0);
        assert!(bucket.blocked_until.is_none());
    }

    #[test]
    fn idle_rate_buckets_are_reclaimed() {
        let now = StdInstant::now();
        let mut idle = ProviderRateBucket::new();
        idle.last_used_at = now
            .checked_sub(Duration::from_millis(PROVIDER_RATE_BUCKET_IDLE_TIMEOUT_MS))
            .unwrap();
        let mut active = ProviderRateBucket::new();
        active.last_used_at = now;
        let mut buckets = HashMap::from([("idle".to_owned(), idle), ("active".to_owned(), active)]);

        retain_active_buckets(&mut buckets, now);

        assert!(!buckets.contains_key("idle"));
        assert!(buckets.contains_key("active"));
    }

    #[tokio::test]
    async fn rate_wait_observes_cancellation() {
        let request = request_plan("https://cancel-limit.example/v1/responses", None);
        let mut headers = HeaderMap::new();
        headers.insert(
            reqwest::header::RETRY_AFTER,
            reqwest::header::HeaderValue::from_static("1"),
        );
        observe_response(&request, StatusCode::TOO_MANY_REQUESTS, &headers);

        let cancel_token = RunCancelToken::default();
        let cancel_trigger = cancel_token.clone();
        let trigger = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(5));
            cancel_trigger.cancel();
        });

        assert!(!wait_for_rate_token(&request, &cancel_token).await);
        trigger.join().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn exhausted_endpoint_does_not_block_another_origin() {
        const LIMITED_ENDPOINT: &str = "https://limited.example/v1/responses";
        const HEALTHY_ENDPOINT: &str = "https://healthy.example/v1/responses";
        const PROBE_TIMEOUT_MS: u64 = 1;

        let mut headers = HeaderMap::new();
        headers.insert(
            "x-ratelimit-remaining-requests",
            reqwest::header::HeaderValue::from_static("0"),
        );
        let limited = request_plan(LIMITED_ENDPOINT, None);
        let healthy = request_plan(HEALTHY_ENDPOINT, None);
        let cancel_token = RunCancelToken::default();
        observe_response(&limited, StatusCode::OK, &headers);

        assert!(
            tokio::time::timeout(
                Duration::from_millis(PROBE_TIMEOUT_MS),
                wait_for_rate_token(&healthy, &cancel_token),
            )
            .await
            .is_ok()
        );
        assert!(
            tokio::time::timeout(
                Duration::from_millis(PROBE_TIMEOUT_MS),
                wait_for_rate_token(&limited, &cancel_token),
            )
            .await
            .is_err()
        );
    }
}
