//! durable append 后的 Feishu 引用正文解析器。

use std::{
    sync::{Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};

use axon_crypto::sha256_hex_prefix;
use axon_domain::{AxonResult, QuotedMessageContext};
use axon_remote_feishu::{
    RemoteFeishuHttpRequestPlan, RemoteFeishuHttpResponse, RemoteFeishuMessageLookupRuntime,
    RemoteFeishuOapiExecutorRuntime, RemoteFeishuReplyContext, RemoteFeishuReplyLookupFailure,
    RemoteFeishuReplyLookupFailureStage, RemoteFeishuReplyLookupOutcome,
};

use super::remote_feishu_reply_cache::RemoteFeishuReplyCache;

/// 单次引用查询超时。
pub(super) const REMOTE_FEISHU_REPLY_LOOKUP_TIMEOUT: Duration = Duration::from_secs(3);
/// 单个限流窗口内最多执行的真实引用查询数。
pub(super) const REMOTE_FEISHU_REPLY_LOOKUPS_PER_WINDOW: usize = 8;
/// 引用查询限流窗口长度。
///
/// 车道化后不存在统一的「worker tick」时钟，限流必须落在真实时间窗口上，否则预算会
/// 被单条车道吃光或彻底失效。
pub(super) const REMOTE_FEISHU_REPLY_LOOKUP_WINDOW: Duration = Duration::from_secs(1);
const MESSAGE_ID_HASH_CHARS: usize = 16;

/// 引用正文解析来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RemoteFeishuReplyResolutionSource {
    Inline,
    CacheHit,
    ApiHit,
    NoVisibleText,
    Degraded,
    LookupLimit,
    Missing,
}

/// 单次引用正文解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RemoteFeishuReplyResolution {
    pub(super) quoted_message: Option<QuotedMessageContext>,
    pub(super) source: RemoteFeishuReplyResolutionSource,
}

/// 跨车道共享的引用正文 resolver。
///
/// 双限缓存与限流预算在同一临界区内维护（保持 512 条 / 8MiB 与全局 QPS 上界不变），
/// 但真实 HTTP 查询在锁外执行：车道之间不应为一次远端查询互相排队。
#[derive(Debug, Default)]
pub(super) struct RemoteFeishuReplyResolver {
    state: Mutex<ResolverState>,
}

#[derive(Debug, Default)]
struct ResolverState {
    cache: RemoteFeishuReplyCache,
    window_started: Option<Instant>,
    api_lookups_this_window: usize,
}

impl ResolverState {
    /// 在时间窗内预留一次真实查询预算。
    fn reserve_lookup(&mut self, now: Instant) -> bool {
        let window_expired = self.window_started.is_none_or(|started| {
            now.saturating_duration_since(started) >= REMOTE_FEISHU_REPLY_LOOKUP_WINDOW
        });
        if window_expired {
            self.window_started = Some(now);
            self.api_lookups_this_window = 0;
        }
        if self.api_lookups_this_window >= REMOTE_FEISHU_REPLY_LOOKUPS_PER_WINDOW {
            return false;
        }
        self.api_lookups_this_window += 1;
        true
    }
}

impl RemoteFeishuReplyResolver {
    pub(super) fn resolve(
        &self,
        context: Option<&RemoteFeishuReplyContext>,
        tenant_access_token: &str,
        api_base_url: Option<&str>,
    ) -> RemoteFeishuReplyResolution {
        self.resolve_with_http(
            context,
            tenant_access_token,
            api_base_url,
            |request, timeout| {
                RemoteFeishuOapiExecutorRuntime::execute_json_with_timeout(request, timeout)
            },
        )
    }

    fn resolve_with_http<F>(
        &self,
        context: Option<&RemoteFeishuReplyContext>,
        tenant_access_token: &str,
        api_base_url: Option<&str>,
        mut http: F,
    ) -> RemoteFeishuReplyResolution
    where
        F: FnMut(&RemoteFeishuHttpRequestPlan, Duration) -> AxonResult<RemoteFeishuHttpResponse>,
    {
        let Some(context) = context else {
            return resolution(None, RemoteFeishuReplyResolutionSource::Missing);
        };
        let Some(message_id) = non_empty(context.reply_to_message_id.as_deref()) else {
            return resolution(None, RemoteFeishuReplyResolutionSource::Missing);
        };
        {
            let mut state = self.lock();
            if let Some(text) = non_empty(context.reply_to_text.as_deref()) {
                let quoted = QuotedMessageContext::new(message_id, text).ok();
                state
                    .cache
                    .insert(message_id.to_owned(), quoted_text(&quoted));
                return resolution(quoted, RemoteFeishuReplyResolutionSource::Inline);
            }
            if let Some(cached) = state.cache.get(message_id) {
                let quoted =
                    cached.and_then(|text| QuotedMessageContext::new(message_id, text).ok());
                return resolution(quoted, RemoteFeishuReplyResolutionSource::CacheHit);
            }
            if !state.reserve_lookup(Instant::now()) {
                return resolution(None, RemoteFeishuReplyResolutionSource::LookupLimit);
            }
        }
        let outcome = RemoteFeishuMessageLookupRuntime::fetch_reply_lookup_with_http(
            api_base_url,
            tenant_access_token,
            message_id,
            |request| http(request, REMOTE_FEISHU_REPLY_LOOKUP_TIMEOUT),
        );
        self.finish_api_lookup(message_id, outcome)
    }

    fn lock(&self) -> MutexGuard<'_, ResolverState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn finish_api_lookup(
        &self,
        message_id: &str,
        outcome: AxonResult<RemoteFeishuReplyLookupOutcome>,
    ) -> RemoteFeishuReplyResolution {
        match outcome {
            Ok(RemoteFeishuReplyLookupOutcome::VisibleText(text)) => {
                let quoted = QuotedMessageContext::new(message_id, text).ok();
                self.lock()
                    .cache
                    .insert(message_id.to_owned(), quoted_text(&quoted));
                resolution(quoted, RemoteFeishuReplyResolutionSource::ApiHit)
            }
            Ok(RemoteFeishuReplyLookupOutcome::NoVisibleText) => {
                self.lock().cache.insert(message_id.to_owned(), None);
                resolution(None, RemoteFeishuReplyResolutionSource::NoVisibleText)
            }
            Ok(RemoteFeishuReplyLookupOutcome::Degraded(failure)) => {
                log_degraded_lookup(message_id, &failure);
                resolution(None, RemoteFeishuReplyResolutionSource::Degraded)
            }
            Err(_) => {
                log_degraded_lookup(
                    message_id,
                    &RemoteFeishuReplyLookupFailure {
                        stage: RemoteFeishuReplyLookupFailureStage::Http,
                        http_status: None,
                        feishu_code: None,
                    },
                );
                resolution(None, RemoteFeishuReplyResolutionSource::Degraded)
            }
        }
    }
}

fn resolution(
    quoted_message: Option<QuotedMessageContext>,
    source: RemoteFeishuReplyResolutionSource,
) -> RemoteFeishuReplyResolution {
    RemoteFeishuReplyResolution {
        quoted_message,
        source,
    }
}

fn quoted_text(context: &Option<QuotedMessageContext>) -> Option<String> {
    context.as_ref().map(|item| item.visible_text.clone())
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn log_degraded_lookup(message_id: &str, failure: &RemoteFeishuReplyLookupFailure) {
    let message_id_hash = sha256_hex_prefix(message_id.as_bytes(), MESSAGE_ID_HASH_CHARS);
    axon_logger::warning(&format!(
        "Feishu 引用正文查询已降级: stage={}, http_status={:?}, feishu_code={:?}, message_id_hash={message_id_hash}",
        failure_stage_code(failure.stage),
        failure.http_status,
        failure.feishu_code,
    ));
}

const fn failure_stage_code(stage: RemoteFeishuReplyLookupFailureStage) -> &'static str {
    match stage {
        RemoteFeishuReplyLookupFailureStage::Http => "http",
        RemoteFeishuReplyLookupFailureStage::Status => "status",
        RemoteFeishuReplyLookupFailureStage::FeishuCode => "feishu_code",
        RemoteFeishuReplyLookupFailureStage::Json => "json",
        RemoteFeishuReplyLookupFailureStage::Content => "content",
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    fn context(message_id: &str, text: Option<&str>) -> RemoteFeishuReplyContext {
        RemoteFeishuReplyContext {
            reply_to_message_id: Some(message_id.to_owned()),
            reply_to_text: text.map(str::to_owned),
            thread_id: None,
        }
    }

    fn response(body: &str) -> RemoteFeishuHttpResponse {
        RemoteFeishuHttpResponse {
            status: 200,
            body: body.to_owned(),
        }
    }

    #[test]
    fn inline_text_warms_cache_without_http() {
        let resolver = RemoteFeishuReplyResolver::default();
        let calls = Cell::new(0);
        let inline = resolver.resolve_with_http(
            Some(&context("om_parent", Some("父消息"))),
            "tenant",
            None,
            |_, _| {
                calls.set(calls.get() + 1);
                unreachable!()
            },
        );
        let cached = resolver.resolve_with_http(
            Some(&context("om_parent", None)),
            "tenant",
            None,
            |_, _| unreachable!(),
        );

        assert_eq!(calls.get(), 0);
        assert_eq!(inline.source, RemoteFeishuReplyResolutionSource::Inline);
        assert_eq!(cached.source, RemoteFeishuReplyResolutionSource::CacheHit);
        assert_eq!(cached.quoted_message.unwrap().visible_text, "父消息");
    }

    #[test]
    fn no_visible_text_is_cached_but_failure_is_not() {
        let resolver = RemoteFeishuReplyResolver::default();
        let no_text_calls = Cell::new(0);
        for _ in 0..2 {
            resolver.resolve_with_http(Some(&context("om_empty", None)), "tenant", None, |_, _| {
                no_text_calls.set(no_text_calls.get() + 1);
                Ok(response(r#"{"code":0,"data":{"items":[]}}"#))
            });
        }
        let failure_calls = Cell::new(0);
        for _ in 0..2 {
            resolver.resolve_with_http(
                Some(&context("om_failed", None)),
                "tenant",
                None,
                |_, _| {
                    failure_calls.set(failure_calls.get() + 1);
                    Ok(RemoteFeishuHttpResponse {
                        status: 503,
                        body: String::new(),
                    })
                },
            );
        }

        assert_eq!(no_text_calls.get(), 1);
        assert_eq!(failure_calls.get(), 2);
    }

    #[test]
    fn lookup_uses_short_timeout_and_enforces_window_budget() {
        let resolver = RemoteFeishuReplyResolver::default();
        let calls = Cell::new(0);
        for index in 0..=REMOTE_FEISHU_REPLY_LOOKUPS_PER_WINDOW {
            let resolution = resolver.resolve_with_http(
                Some(&context(&format!("om_{index}"), None)),
                "tenant",
                None,
                |_, timeout| {
                    assert_eq!(timeout, REMOTE_FEISHU_REPLY_LOOKUP_TIMEOUT);
                    calls.set(calls.get() + 1);
                    Ok(response(r#"{"code":0,"data":{"items":[]}}"#))
                },
            );
            if index == REMOTE_FEISHU_REPLY_LOOKUPS_PER_WINDOW {
                assert_eq!(
                    resolution.source,
                    RemoteFeishuReplyResolutionSource::LookupLimit
                );
            }
        }
        assert_eq!(calls.get(), REMOTE_FEISHU_REPLY_LOOKUPS_PER_WINDOW);
    }

    #[test]
    fn window_budget_refills_after_the_window_elapses() {
        let start = Instant::now();
        let mut state = ResolverState::default();
        for _ in 0..REMOTE_FEISHU_REPLY_LOOKUPS_PER_WINDOW {
            assert!(state.reserve_lookup(start));
        }

        assert!(!state.reserve_lookup(start));
        assert!(state.reserve_lookup(start + REMOTE_FEISHU_REPLY_LOOKUP_WINDOW));
    }
}
