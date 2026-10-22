use super::*;
use axon_session_runtime::{TurnPerformanceSnapshot, TurnPerformanceStageTiming};

#[test]
fn disabled_telemetry_does_not_add_performance_payload() {
    let interaction_id =
        axon_domain::InteractionId::new("interaction-test").expect("Interaction ID 应合法");
    let payload = annotate_runtime_payload("{\"response\":\"ok\"}", &interaction_id, None, None)
        .expect("payload 应可标注");
    let payload: serde_json::Value = serde_json::from_str(&payload).expect("payload 应为 JSON");

    assert!(payload.get("firstTokenPerformance").is_none());
}

#[test]
fn completion_payload_carries_generation_metrics_for_replay() {
    let interaction_id =
        axon_domain::InteractionId::new("interaction-test").expect("Interaction ID 应合法");
    let payload = annotate_runtime_payload(
        "{\"response\":\"ok\"}",
        &interaction_id,
        None,
        Some(TurnGenerationMetrics {
            ttft_ms: Some(1_200),
            decode_ms: Some(3_000),
            output_tokens: Some(420),
            reasoning_output_tokens: None,
        }),
    )
    .expect("payload 应可标注");
    let payload: serde_json::Value = serde_json::from_str(&payload).expect("payload 应为 JSON");

    // 字段名与实时观测帧一致：重放历史时客户端用同一套解析拿到首字与速率。
    assert_eq!(payload["generation"]["ttftMs"], 1_200);
    assert_eq!(payload["generation"]["decodeMs"], 3_000);
    assert_eq!(payload["generation"]["outputTokens"], 420);
    // 没观测到推理输出时不写该数，客户端继续用自己累计的用量兜底。
    assert_eq!(
        payload["generation"]["reasoningOutputTokens"],
        serde_json::Value::Null
    );
    // 没有测得指标时不写该键，客户端据此区分「没测到」与「测得 0」。
    let bare = annotate_runtime_payload("{\"response\":\"ok\"}", &interaction_id, None, None)
        .expect("payload 应可标注");
    let bare: serde_json::Value = serde_json::from_str(&bare).expect("payload 应为 JSON");
    assert!(bare.get("generation").is_none());
}

#[test]
fn enabled_telemetry_adds_only_bounded_stage_snapshot() {
    let interaction_id =
        axon_domain::InteractionId::new("interaction-test").expect("Interaction ID 应合法");
    let snapshot = TurnPerformanceSnapshot {
        stages: vec![TurnPerformanceStageTiming {
            stage: TurnPerformanceStage::WorkerClaimed,
            elapsed_us: 0,
        }],
        ..TurnPerformanceSnapshot::default()
    };
    let payload = annotate_runtime_payload(
        "{\"response\":\"ok\"}",
        &interaction_id,
        Some(&snapshot),
        None,
    )
    .expect("payload 应可标注");

    assert!(payload.contains("firstTokenPerformance"));
    assert!(!payload.contains("prompt"));
}
