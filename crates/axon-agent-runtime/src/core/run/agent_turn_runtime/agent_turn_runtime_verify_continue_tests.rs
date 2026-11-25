use super::*;
use crate::{
    AgentModelMessage, ModelToolDefinition, PromptCacheProviderWireFormat,
    ProviderPromptCachePlanInput, ProviderPromptCachePlanRuntime, ProviderStreamProtocol,
    ProviderStreamingTransportInput, ProviderStreamingWireTransport,
};
use axon_client_identity::ClientChannel;

#[test]
fn tool_receipts_keep_tools_available_until_protocol_stop() {
    let input = transport_input();
    let provider_calls = std::cell::Cell::new(0);
    let tool_turns = std::cell::Cell::new(0);
    let report = AgentTurnRuntime
        .run(
            &input,
            |request| {
                let call = provider_calls.get();
                provider_calls.set(call + 1);
                let body: serde_json::Value =
                    serde_json::from_str(&request.request.body_text).unwrap();
                assert_eq!(body["tools"].as_array().map(Vec::len), Some(2));
                crate::test_stream_outcome(provider_body(call))
            },
            |_report, _provider_history, _turn_id| {
                let tool_turn = tool_turns.get();
                tool_turns.set(tool_turn + 1);
                let (call_id, receipt) = if tool_turn == 0 {
                    ("call_write", "verify=passed")
                } else {
                    ("call_search", "search=completed")
                };
                Ok::<_, axon_domain::AxonError>(AgentTurnToolDecision::Continue {
                    receipts: vec![receipt.to_owned()],
                    next_messages: vec![AgentModelMessage::tool_result(
                        call_id,
                        format!("status=completed {receipt}"),
                        true,
                    )],
                })
            },
            None,
            None,
        )
        .unwrap();

    assert_eq!(provider_calls.get(), 3);
    assert_eq!(tool_turns.get(), 2);
    assert_eq!(report.content, "工具工作完成");
    assert_eq!(
        report.tool_receipts,
        vec!["verify=passed", "search=completed"]
    );
}

fn provider_body(call: u32) -> &'static str {
    match call {
        0 => {
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_write\",\"type\":\"function\",\"function\":{\"name\":\"builtin_write\",\"arguments\":\"{\\\"path\\\":\\\"src/lib.rs\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\ndata: [DONE]\n"
        }
        1 => {
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_search\",\"type\":\"function\",\"function\":{\"name\":\"builtin_search\",\"arguments\":\"{\\\"query\\\":\\\"verify\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\ndata: [DONE]\n"
        }
        _ => {
            "data: {\"choices\":[{\"delta\":{\"content\":\"工具工作完成\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n"
        }
    }
}

fn model_tools() -> Vec<ModelToolDefinition> {
    vec![model_tool("builtin.write"), model_tool("builtin.search")]
}

fn model_tool(name: &str) -> ModelToolDefinition {
    ModelToolDefinition {
        qualified_name: name.to_owned(),
        description: "测试工具".to_owned(),
        schema_hash: format!("tool-schema-{name}"),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": { "query": { "type": "string" } },
            "required": ["query"]
        }),
        input_format: axon_domain::ToolInputFormat::JsonSchema,
    }
}

fn prompt_cache_plan() -> crate::ProviderPromptCachePlan {
    ProviderPromptCachePlanRuntime
        .plan(ProviderPromptCachePlanInput {
            provider_name: "test-provider".to_owned(),
            capabilities: axon_config::ProviderCapabilities::stateless(),
            format: PromptCacheProviderWireFormat::OpenAiCompatibleChat,
            model_id: "test-model".to_owned(),
            context_window: 32_000,
            supports_tools: true,
            supports_vision: false,
            messages: vec![AgentModelMessage::text("user", "ping")].into(),
            model_tools: model_tools(),
            context_section_count: 1,
        })
        .unwrap()
}

/*
 * 本轮没有收口就继续时，边界处关掉的输入通道必须重新打开。
 *
 * 现场（2026-11-25）：一轮里 provider 连续给出「没有工具调用、finish_reason 也不是
 * stop」的答复（该供应商确实会给出畸形工具调用，解析后就是零个有效调用），边界处
 * `close_and_drain` 关掉通道后没人重新打开。此后用户在本轮追加的消息一律被拒，
 * 由调用方静默降级成下一轮：界面上那条消息永远挂着「等待注入」，收口时还被标成
 * 未送达，同一条消息在正文里出现两格气泡。
 */

/// 与真实 FIFO 同口径的 steer 源：通道关着就拒绝，开着就收下。
struct GatedSteerSource {
    accepting: std::cell::Cell<bool>,
    pending: std::cell::RefCell<Vec<AgentTurnSteerInput>>,
    pending_jobs: std::cell::Cell<usize>,
}

impl GatedSteerSource {
    fn new(pending_jobs: usize) -> Self {
        Self {
            accepting: std::cell::Cell::new(true),
            pending: std::cell::RefCell::new(Vec::new()),
            pending_jobs: std::cell::Cell::new(pending_jobs),
        }
    }

    /// 运行中追加；返回通道是否接收。
    fn submit(&self, text: &str) -> bool {
        if !self.accepting.get() {
            return false;
        }
        self.pending.borrow_mut().push(AgentTurnSteerInput {
            message: AgentModelMessage::text("user", text),
            quoted_message: None,
        });
        true
    }
}

impl AgentTurnSteerSource for GatedSteerSource {
    fn drain(&self) -> Vec<AgentTurnSteerInput> {
        std::mem::take(&mut *self.pending.borrow_mut())
    }

    fn close_and_drain(&self) -> Vec<AgentTurnSteerInput> {
        self.accepting.set(false);
        self.drain()
    }

    fn reopen(&self) {
        self.accepting.set(true);
    }

    fn pending_background_jobs(&self) -> usize {
        self.pending_jobs.get()
    }
}

fn transport_input() -> ProviderStreamingTransportInput {
    ProviderStreamingTransportInput {
        client_channel: ClientChannel::Cli,
        turn_id: axon_domain::TurnId(1),
        provider_name: "test-provider".to_owned(),
        capabilities: axon_config::ProviderCapabilities::stateless(),
        protocol: ProviderStreamProtocol::OpenAiCompatible,
        base_url: "https://provider.example/v1".to_owned(),
        api_key_env: None,
        api_key: None,
        model_id: "test-model".to_owned(),
        service_tier: None,
        reasoning_effort: None,
        reasoning_summary: Some("auto".to_owned()),
        structured_output: None,
        context_window: 32_000,
        supports_tools: true,
        supports_vision: false,
        messages: vec![AgentModelMessage::text("user", "ping")].into(),
        model_tools: model_tools(),
        context_composition: None,
        context_section_count: 1,
        prompt_cache_plan: prompt_cache_plan(),
        anthropic_max_tokens: 4096,
        wire_transport: ProviderStreamingWireTransport::Http,
        websocket_url: None,
        enable_request_compression: false,
        server_conversation_state_ref: None,
    }
}

/// 没有工具调用、finish_reason 也不是 stop：本轮继续。
const CONTINUE_STREAM: &str = "data: {\"choices\":[{\"delta\":{\"content\":\"继续推进\"},\"finish_reason\":\"length\"}]}\n\ndata: [DONE]\n";

/// 正常收口。
const STOP_STREAM: &str = "data: {\"choices\":[{\"delta\":{\"content\":\"完成\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n";

#[test]
fn continuing_provider_round_keeps_steer_channel_open() {
    let input = transport_input();
    let source = GatedSteerSource::new(0);
    let provider_calls = std::cell::Cell::new(0);
    let accepted = std::cell::Cell::new(false);

    AgentTurnRuntime
        .run(
            &input,
            |_request| {
                let call = provider_calls.get();
                provider_calls.set(call + 1);
                match call {
                    0 => crate::test_stream_outcome(CONTINUE_STREAM),
                    1 => {
                        accepted.set(source.submit("边界之后追加"));
                        crate::test_stream_outcome(STOP_STREAM)
                    }
                    _ => crate::test_stream_outcome(STOP_STREAM),
                }
            },
            |_, _, _| -> axon_domain::AxonResult<AgentTurnToolDecision<()>> {
                panic!("本轮不应执行工具")
            },
            Some(&source),
            None,
        )
        .unwrap();

    assert!(
        accepted.get(),
        "本轮还在跑，安全边界之后追加的输入必须仍被接收；被拒就会被静默降级成下一轮"
    );
    assert!(provider_calls.get() >= 2, "该轮必须继续请求 provider");
}

#[test]
fn pre_final_guard_round_keeps_steer_channel_open() {
    let input = transport_input();
    let source = GatedSteerSource::new(1);
    let provider_calls = std::cell::Cell::new(0);
    let accepted = std::cell::Cell::new(false);

    AgentTurnRuntime
        .run(
            &input,
            |_request| {
                let call = provider_calls.get();
                provider_calls.set(call + 1);
                if call == 1 {
                    accepted.set(source.submit("守卫之后追加"));
                }
                crate::test_stream_outcome(STOP_STREAM)
            },
            |_, _, _| -> axon_domain::AxonResult<AgentTurnToolDecision<()>> {
                panic!("本轮不应执行工具")
            },
            Some(&source),
            None,
        )
        .unwrap();

    assert!(
        accepted.get(),
        "守卫推迟最终答复后本轮仍在继续，追加输入必须仍被接收"
    );
}

/// 一条有效的工具调用；用于制造「真正推进了一轮」。
const TOOL_STREAM: &str = "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_ping\",\"type\":\"function\",\"function\":{\"name\":\"builtin_search\",\"arguments\":\"{\\\"query\\\":\\\"ping\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\ndata: [DONE]\n";

#[test]
fn idle_continue_rounds_are_bounded() {
    let input = transport_input();
    let provider_calls = std::cell::Cell::new(0);

    let report = AgentTurnRuntime
        .run(
            &input,
            |_request| {
                provider_calls.set(provider_calls.get() + 1);
                crate::test_stream_outcome(CONTINUE_STREAM)
            },
            |_, _, _| -> axon_domain::AxonResult<AgentTurnToolDecision<()>> {
                panic!("本轮不应执行工具")
            },
            None,
            None,
        )
        .unwrap();

    assert_eq!(
        provider_calls.get(),
        MAX_IDLE_CONTINUE_ROUNDS + 1,
        "空转必须有上限：供应商一直说没完时不能无限重发全量上下文"
    );
    assert_eq!(report.content, "继续推进", "收口仍要带回已产出的正文");
}

#[test]
fn productive_tool_round_resets_the_idle_continue_budget() {
    let input = transport_input();
    let provider_calls = std::cell::Cell::new(0);
    // 先空转到差一轮满额，夹一次真正的工具推进，再空转整整一档预算。
    let tool_round = MAX_IDLE_CONTINUE_ROUNDS - 1;
    let stop_round = tool_round + 1 + MAX_IDLE_CONTINUE_ROUNDS;

    let report = AgentTurnRuntime
        .run(
            &input,
            |_request| {
                let call = provider_calls.get();
                provider_calls.set(call + 1);
                if call == tool_round {
                    crate::test_stream_outcome(TOOL_STREAM)
                } else if call < stop_round {
                    crate::test_stream_outcome(CONTINUE_STREAM)
                } else {
                    crate::test_stream_outcome(STOP_STREAM)
                }
            },
            |_, _, _| {
                Ok::<_, axon_domain::AxonError>(AgentTurnToolDecision::Continue {
                    receipts: vec!["ping-ok".to_owned()],
                    next_messages: vec![AgentModelMessage::tool_result(
                        "call_ping",
                        "status=completed ping-ok",
                        true,
                    )],
                })
            },
            None,
            None,
        )
        .unwrap();

    assert_eq!(
        provider_calls.get(),
        stop_round + 1,
        "工具执行是真推进：它必须把空转预算重置，长任务不能被轮数误判"
    );
    assert_eq!(report.tool_receipts, vec!["ping-ok"]);
}

#[test]
fn idle_continue_budget_is_replenished_by_steer_injection() {
    let input = transport_input();
    let source = GatedSteerSource::new(0);
    let provider_calls = std::cell::Cell::new(0);
    // 远超空转上限的轮数：每轮都有真实的运行中追加被注入，就属于真推进。
    let stop_round = MAX_IDLE_CONTINUE_ROUNDS * 2 + 1;

    let report = AgentTurnRuntime
        .run(
            &input,
            |_request| {
                let call = provider_calls.get();
                provider_calls.set(call + 1);
                if call < stop_round {
                    assert!(source.submit(&format!("追加约束 {call}")), "追加必须被接收");
                    crate::test_stream_outcome(CONTINUE_STREAM)
                } else {
                    crate::test_stream_outcome(STOP_STREAM)
                }
            },
            |_, _, _| -> axon_domain::AxonResult<AgentTurnToolDecision<()>> {
                panic!("本轮不应执行工具")
            },
            Some(&source),
            None,
        )
        .unwrap();

    assert_eq!(
        provider_calls.get(),
        stop_round + 1,
        "注入的运行中追加是真推进：它必须把空转预算重置，长会话不能被上限误杀"
    );
    assert_eq!(report.content, "完成");
}
