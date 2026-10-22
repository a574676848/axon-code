//! CLI 接入 runtime 跨渠道故障转移执行循环（外层重试）。
//!
//! 遍历 `RunRequest::channel_chain`，逐个渠道尝试单渠道执行单元：
//! - 成功：记录健康成功 + 写亲和缓存，返回响应（附带渠道尝试审计链）。
//! - 瞬时失败（5xx/429/超时等传输错误）：记录健康失败，切换下一渠道。
//! - 永久失败（4xx 鉴权/参数、工具循环、配置错误）：快速失败，不再换渠道。
//!
//! 内层单渠道传输重试由 `provider_stream_transport` 负责；本层只在内层耗尽后才换渠道，
//! 避免一次抖动就漂移会话。

use axon_agent_runtime::{
    AgentRunChannelEvent, AgentRunChannelEventKind, AgentRunChannelTransport, AgentRunFailoverPlan,
    run_agent_channel_failover,
};
use axon_config::ChannelBalancerConfig;
use axon_domain::BreakerPolicy;
use axon_protocol::wire::{WireProviderStatus, WireProviderStatusPhase, WireProviderTransport};
use axon_provider_transport::AgentRunProviderTransportSession;
use std::time::Instant;

use super::RunRequest;
use super::run_channel_affinity::{
    record_channel_sample, with_affinity_cache, with_health_registry,
};
use super::run_execution::{RunExecutionError, RunResponse, now_ms};
use super::run_progress::RunProgressSink;
use super::run_provider_loop::run_provider_single_channel;
use super::run_steer::{RunSteerBridge, register_deferred_message_session};

#[cfg(test)]
const OUTCOME_SUCCESS: &str = "success";
#[cfg(test)]
const OUTCOME_TRANSIENT: &str = "transient_failure";
#[cfg(test)]
const OUTCOME_PERMANENT: &str = "permanent_failure";

/// 故障转移执行入口：遍历渠道链逐个尝试，按错误分类决定换渠道或快速失败。
pub(super) fn run_with_failover(
    request: &RunRequest,
    progress: Option<&RunProgressSink<'_>>,
) -> Result<RunResponse, Box<RunExecutionError>> {
    let breaker_policy = breaker_policy(request.balancer);
    let turn_inbox = request
        .steer_inbox
        .clone()
        .unwrap_or_else(|| RunSteerBridge::default().open());
    let _deferred_message_registration =
        register_deferred_message_session(&request.session_id, turn_inbox.clone());
    let _completion_notice_subscription =
        super::run_completion_notice_subscription::subscribe_run_completion_notices(
            &request.session_id,
        );
    let _session_reclaim_subscription =
        super::run_completion_notice_subscription::subscribe_run_shell_session_reclaims(
            &request.session_id,
        );
    // 摘要级恢复：把此前完成、尚未被任何一轮装配过的子代理终态注入本轮请求
    // （合同 §5.3.3）。注入通道刚在上一步注册，因此这里注入的内容必然进入
    // 本轮 provider 请求；装配失败不阻断 run：终态事实保留待下次装配。
    if let Err(error) = super::run_continuation_assembly::assemble_pending_continuations(
        Some(request.config_path.as_path()),
        &request.session_id,
        now_ms(),
    ) {
        axon_logger::warning(&format!(
            "子代理终态装配失败，事实保留待下次装配: session={}, error={error}",
            request.session_id
        ));
    }
    run_agent_channel_failover(
        AgentRunFailoverPlan {
            chain: &request.channel_chain,
            routing_key: &request.routing_key,
            session_id: &request.session_id,
            balancer: request.balancer,
            now_ms: now_ms(),
        },
        |key| {
            with_health_registry(breaker_policy, |health| {
                health.record_success(key);
            });
        },
        |key, now| {
            with_health_registry(breaker_policy, |health| {
                health.record_failure(key, now);
            });
        },
        |routing_key, session_id, provider, now, ttl_ms| {
            with_affinity_cache(|cache| {
                cache.record(routing_key, session_id, provider, now, ttl_ms);
            });
        },
        |event| emit_channel_status(request, progress, event),
        |model, provider| {
            let started_at = Instant::now();
            let result =
                run_provider_single_channel(request, model, provider, progress, &turn_inbox);
            let token_usage = result
                .as_ref()
                .ok()
                .and_then(|response| response.token_usage)
                .or_else(|| result.as_ref().err().and_then(|error| error.token_usage));
            record_channel_sample(
                &request.config_path,
                request.balancer.telemetry_persistence_enabled,
                axon_agent_runtime::channel_metrics_key(&request.routing_key, &provider.name),
                u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX),
                result.is_ok(),
                token_usage,
                now_ms(),
            );
            result
        },
    )
}

fn emit_channel_status(
    request: &RunRequest,
    progress: Option<&RunProgressSink<'_>>,
    event: AgentRunChannelEvent,
) {
    let Some(sink) = progress else {
        return;
    };
    let phase = match event.kind {
        AgentRunChannelEventKind::AttemptStarted => WireProviderStatusPhase::Connecting,
        AgentRunChannelEventKind::AttemptFailed => WireProviderStatusPhase::Failed,
        AgentRunChannelEventKind::Failover => WireProviderStatusPhase::Failover,
    };
    let transport = effective_wire_transport(request, &event);
    sink.provider_status(WireProviderStatus {
        provider: event.provider,
        model: event.model,
        phase,
        transport,
        attempt_index: 1,
        http_status: event.http_status,
        error_message: event.error_message,
        next_provider: event.next_provider,
        next_model: event.next_model,
    });
}

fn effective_wire_transport(
    request: &RunRequest,
    event: &AgentRunChannelEvent,
) -> WireProviderTransport {
    let Some((_, provider)) = request
        .channel_chain
        .iter()
        .find(|(model, provider)| model.id == event.model && provider.name == event.provider)
    else {
        return match event.transport {
            AgentRunChannelTransport::Http => WireProviderTransport::Http,
            AgentRunChannelTransport::WebSocket => WireProviderTransport::WebSocket,
        };
    };
    let preferred = if provider.wire_transport == axon_config::PROVIDER_WIRE_TRANSPORT_WEBSOCKET {
        axon_agent_runtime::ProviderStreamingWireTransport::WebSocket
    } else {
        axon_agent_runtime::ProviderStreamingWireTransport::Http
    };
    match AgentRunProviderTransportSession::new(
        &provider.name,
        &request.session_id,
        &provider.base_url,
        preferred,
    )
    .effective_transport()
    {
        axon_agent_runtime::ProviderStreamingWireTransport::Http => WireProviderTransport::Http,
        axon_agent_runtime::ProviderStreamingWireTransport::WebSocket => {
            WireProviderTransport::WebSocket
        }
    }
}

fn breaker_policy(balancer: ChannelBalancerConfig) -> BreakerPolicy {
    BreakerPolicy {
        failure_threshold: balancer.failure_threshold,
        cooldown_ms: balancer.cooldown_ms,
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use axon_config::{ChannelBalancerConfig, ModelConfig, ProviderConfig};

    use super::super::run_channel_affinity::{
        channel_singleton_test_guard, reset_channel_singletons_for_test,
    };
    use super::*;
    use axon_agent_runtime::AGENT_RUN_ERROR_TYPE_PROVIDER_TRANSPORT;

    const ROUTING_KEY: &str = "gpt-4o";
    const SESSION_ID: &str = "session-failover";

    fn provider(name: &str) -> ProviderConfig {
        ProviderConfig::new(name, axon_config::PROVIDER_PROTOCOL_OPENAI, "https://x/v1")
    }

    fn model(id: &str, provider_name: &str) -> ModelConfig {
        ModelConfig {
            name: ROUTING_KEY.to_owned(),
            id: id.to_owned(),
            provider: provider_name.to_owned(),
            context_window: 128_000,
            ..ModelConfig::default()
        }
    }

    fn chain(specs: &[(&str, &str)]) -> Vec<(ModelConfig, ProviderConfig)> {
        specs
            .iter()
            .map(|(model_id, provider_name)| {
                (model(model_id, provider_name), provider(provider_name))
            })
            .collect()
    }

    fn ok_response(provider_name: &str, model_id: &str) -> RunResponse {
        RunResponse {
            content: "ok".to_owned(),
            assistant_part_id: "assistant-part:failover".to_owned(),
            http_status: 200,
            token_usage: None,
            tool_receipts: Vec::new(),
            tool_calls: Vec::new(),
            provider: provider_name.to_owned(),
            model: model_id.to_owned(),
            channel_attempts: Vec::new(),
            reasoning_messages: Vec::new(),
            turn_events: Vec::new(),
            max_tokens_truncated: false,
        }
    }

    fn transport_error(http_status: Option<u16>, message: &str) -> Box<RunExecutionError> {
        Box::new(RunExecutionError {
            message: message.to_owned(),
            http_status,
            error_type: AGENT_RUN_ERROR_TYPE_PROVIDER_TRANSPORT,
            error_code: None,
            provider_protocol: None,
            error_param: None,
            failure_kind: None,
            call_index: None,
            provider_error: None,
            token_usage: None,
            tool_receipts: Box::new([]),
            visible_output_emitted: false,
            channel_attempts: Vec::new(),
            provider_event_type: None,
            upstream_request_id: None,
            committed_side_effects: false,
        })
    }

    fn run(
        chain: &[(ModelConfig, ProviderConfig)],
        execute: impl FnMut(
            &ModelConfig,
            &ProviderConfig,
        ) -> Result<RunResponse, Box<RunExecutionError>>,
    ) -> Result<RunResponse, Box<RunExecutionError>> {
        let balancer = ChannelBalancerConfig::default();
        let policy = breaker_policy(balancer);
        run_agent_channel_failover(
            AgentRunFailoverPlan {
                chain,
                routing_key: ROUTING_KEY,
                session_id: SESSION_ID,
                balancer,
                now_ms: 1_000,
            },
            |key| {
                with_health_registry(policy, |health| {
                    health.record_success(key);
                });
            },
            |key, now| {
                with_health_registry(policy, |health| {
                    health.record_failure(key, now);
                });
            },
            |routing_key, session_id, provider, now, ttl_ms| {
                with_affinity_cache(|cache| {
                    cache.record(routing_key, session_id, provider, now, ttl_ms);
                });
            },
            |_| {},
            execute,
        )
    }

    #[test]
    fn transient_failure_switches_to_next_channel() {
        let _singleton_guard = channel_singleton_test_guard();
        reset_channel_singletons_for_test();
        let chain = chain(&[("gpt-5", "p-a"), ("gpt-5", "p-b")]);
        let calls = Cell::new(0u32);
        let result = run(&chain, |_model, provider| {
            calls.set(calls.get() + 1);
            if provider.name == "p-a" {
                Err(transport_error(Some(503), "service unavailable"))
            } else {
                Ok(ok_response(&provider.name, "gpt-5"))
            }
        });
        let response = result.expect("第二渠道应成功");
        assert_eq!(calls.get(), 2);
        assert_eq!(response.provider, "p-b");
        assert_eq!(response.model, "gpt-5");
        assert_eq!(response.channel_attempts.len(), 2);
        assert_eq!(response.channel_attempts[0].model, "gpt-5");
        assert_eq!(response.channel_attempts[1].model, "gpt-5");
        assert_eq!(response.channel_attempts[0].outcome, OUTCOME_TRANSIENT);
        assert_eq!(response.channel_attempts[1].outcome, OUTCOME_SUCCESS);
        // 成功后写入亲和缓存。
        let pinned = with_affinity_cache(|cache| {
            cache.get(ROUTING_KEY, SESSION_ID, 1_001).map(str::to_owned)
        });
        assert_eq!(pinned.as_deref(), Some("p-b"));
        reset_channel_singletons_for_test();
    }

    #[test]
    fn permanent_failure_does_not_switch() {
        let _singleton_guard = channel_singleton_test_guard();
        reset_channel_singletons_for_test();
        let chain = chain(&[("m-a", "p-a"), ("m-b", "p-b")]);
        let calls = Cell::new(0u32);
        let result = run(&chain, |_model, _provider| {
            calls.set(calls.get() + 1);
            Err(transport_error(Some(401), "unauthorized"))
        });
        let error = result.expect_err("永久错误应快速失败");
        // 仅尝试一次，未换渠道。
        assert_eq!(calls.get(), 1);
        assert_eq!(error.channel_attempts.len(), 1);
        assert_eq!(error.channel_attempts[0].outcome, OUTCOME_PERMANENT);
        reset_channel_singletons_for_test();
    }

    #[test]
    fn permanent_failures_feed_channel_breaker() {
        let _singleton_guard = channel_singleton_test_guard();
        reset_channel_singletons_for_test();
        let chain = chain(&[("m-a", "p-a")]);
        for _ in 0..3 {
            let _ = run(&chain, |_model, _provider| {
                Err(transport_error(Some(401), "unauthorized"))
            });
        }
        let key = axon_agent_runtime::ChannelKey {
            model_name: ROUTING_KEY.to_owned(),
            provider: "p-a".to_owned(),
        };
        let available =
            with_health_registry(breaker_policy(ChannelBalancerConfig::default()), |health| {
                health.is_available(&key, 2_000)
            });
        assert!(
            !available,
            "连续 3 次永久失败后渠道应被熔断（400/401/404 必须供数熔断器）"
        );
        reset_channel_singletons_for_test();
    }

    #[test]
    fn all_channels_exhausted_returns_aggregated_error() {
        let _singleton_guard = channel_singleton_test_guard();
        reset_channel_singletons_for_test();
        let chain = chain(&[("m-a", "p-a"), ("m-b", "p-b")]);
        let result = run(&chain, |_model, _provider| {
            Err(transport_error(Some(503), "down"))
        });
        let error = result.expect_err("全链耗尽应返回错误");
        assert_eq!(error.channel_attempts.len(), 2);
        assert!(error.message.contains("全部 2 个渠道均失败"));
        reset_channel_singletons_for_test();
    }

    #[test]
    fn first_channel_success_skips_rest() {
        let _singleton_guard = channel_singleton_test_guard();
        reset_channel_singletons_for_test();
        let chain = chain(&[("m-a", "p-a"), ("m-b", "p-b")]);
        let calls = Cell::new(0u32);
        let result = run(&chain, |_model, provider| {
            calls.set(calls.get() + 1);
            Ok(ok_response(&provider.name, "m-a"))
        });
        let response = result.expect("首选渠道应成功");
        assert_eq!(calls.get(), 1);
        assert_eq!(response.provider, "p-a");
        assert_eq!(response.channel_attempts.len(), 1);
        reset_channel_singletons_for_test();
    }

    #[test]
    fn tool_loop_error_is_treated_as_permanent() {
        let _singleton_guard = channel_singleton_test_guard();
        reset_channel_singletons_for_test();
        let chain = chain(&[("m-a", "p-a"), ("m-b", "p-b")]);
        let calls = Cell::new(0u32);
        let result = run(&chain, |_model, _provider| {
            calls.set(calls.get() + 1);
            Err(Box::new(RunExecutionError {
                message: "tool loop exhausted".to_owned(),
                http_status: Some(200),
                error_type: "tool_loop",
                error_code: None,
                provider_protocol: None,
                error_param: None,
                failure_kind: None,
                call_index: None,
                provider_error: None,
                token_usage: None,
                tool_receipts: Box::new([]),
                visible_output_emitted: false,
                channel_attempts: Vec::new(),
                provider_event_type: None,
                upstream_request_id: None,
                committed_side_effects: false,
            }))
        });
        result.expect_err("工具循环错误不应换渠道");
        // 非传输错误视为永久，仅尝试一次。
        assert_eq!(calls.get(), 1);
        reset_channel_singletons_for_test();
    }
}
