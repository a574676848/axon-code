use super::super::test_support::test_transport_input;
use super::*;

fn overflow() -> ProviderStreamingTransportOutcome {
    ProviderStreamingTransportOutcome::Error(crate::ProviderStreamingTransportError::new(
        "The input token count exceeds the maximum number of tokens allowed 1048576.",
        Some(400),
    ))
}

#[test]
fn oversized_request_is_not_rejected_by_transport_layer() {
    let input = test_transport_input();
    let mut calls = 0;
    let error = WindowRequest { input: &input }
        .send(|_| {
            calls += 1;
            overflow()
        })
        .unwrap_err();
    assert_eq!(calls, 1);
    assert!(format!("{error:?}").contains("1048576"));
}

#[test]
fn provider_overflow_returns_without_rewriting_history_or_retrying() {
    let input = test_transport_input();
    let original = input.messages.clone();
    let mut calls = 0;
    let error = WindowRequest { input: &input }
        .send(|_| {
            calls += 1;
            overflow()
        })
        .unwrap_err();
    assert_eq!(calls, 1);
    assert_eq!(input.messages, original);
    assert!(format!("{error:?}").contains("1048576"));
}

#[test]
fn visible_output_is_never_replayed_after_overflow() {
    let input = test_transport_input();
    let mut calls = 0;
    let result = WindowRequest { input: &input }.send(|_| {
        calls += 1;
        overflow().with_visible_output()
    });
    assert!(result.is_err());
    assert_eq!(calls, 1);
}

#[test]
fn context_window_does_not_add_a_second_send_gate() {
    let mut input = test_transport_input();
    input.context_window = 1_000;
    let mut calls = 0;
    assert!(
        WindowRequest { input: &input }
            .send(|_| {
                calls += 1;
                overflow()
            })
            .is_err()
    );
    assert_eq!(calls, 1);
}

#[test]
fn failed_rollover_prompts_agent_before_provider_request() {
    let mut input = test_transport_input();
    input
        .messages
        .make_mut()
        .push(AgentModelMessage::runtime_note(
            AgentRuntimeNoteKind::ContextRolloverRequest,
            "开启新窗口",
        ));
    let mut rollover = |_: &[AgentModelMessage]| -> AxonResult<crate::CanonicalCheckpoint> {
        Err(AxonError::runtime_failure("便签交接无效"))
    };
    let compression = WindowRolloverRuntime::new(
        crate::WindowRolloverPolicyRuntime::new(crate::CompressionPolicy {
            trigger_tokens: 20_000,
            context_window: 32_000,
        }),
        &mut rollover,
    );
    let mut calls = 0;
    let error = AgentTurnRuntime
        .run(
            &input,
            |_| {
                calls += 1;
                overflow()
            },
            |_, _, _| -> AxonResult<AgentTurnToolDecision<()>> { panic!("不应执行工具") },
            None,
            Some(compression),
        )
        .unwrap_err();
    assert_eq!(calls, 1);
    assert!(format!("{error:?}").contains("1048576"));
}
