//! 主回合的事件载荷与投影：把 Interaction / Task 记录映射成投递与可观测所需的 JSON。
//!
//! 从 `interaction_turn_runner` 拆出：载荷构造是**纯映射**，不碰运行态、不发布事件，
//! 因此它不该和「跑一个回合」的状态机挤在同一个文件里。

use super::interaction_turn_event_sink::TurnGenerationReadout;
use super::*;

pub(super) fn runtime_error(error: impl std::fmt::Display) -> PrimaryTurnRunnerError {
    PrimaryTurnRunnerError::Runtime(error.to_string())
}

pub(super) fn interaction_payload(interaction: &InteractionRecord) -> String {
    serde_json::json!({
        "prompt": interaction.content,
        "transportContext": interaction_transport_context(interaction),
    })
    .to_string()
}

/// 从主 turn 事件流里提取汇总回复正文。
///
/// 产品定义要求渠道交付主 turn 的汇总回复，而不是后台 Execution 的原始回复；
/// `turn_completed` 的 response 字段就是主 turn 的最终交付文本。
pub(super) fn completion_summary_from_events(events: &[RuntimeEvent]) -> Option<String> {
    events
        .iter()
        .rev()
        .find(|event| event.event_type == axon_session_runtime::model::TURN_COMPLETED_EVENT)
        .and_then(|event| serde_json::from_str::<serde_json::Value>(&event.payload_json).ok())
        .and_then(|value| {
            value
                .get("response")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .map(|text| text.trim().to_owned())
}

/// 本轮生成指标：首字、解码窗口与本轮实测输出。
///
/// 观测帧是广播投递、不进持久日志的，因此重放历史或重启客户端之后首字与速率就没了。
/// 这几个数必须随收尾事件一起落库，才能在重放时还原成与 TUI 同口径的读数。
#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TurnGenerationMetrics {
    /// 首字耗时。
    pub ttft_ms: Option<u64>,
    /// 解码窗口（本轮 provider 流式实测耗时减去首字）。
    pub decode_ms: Option<u64>,
    /// 本轮 Provider 实测输出 token；一条用量观测都没有时为 None。
    pub output_tokens: Option<u64>,
    /// 本轮 Provider 实测推理输出 token；一条用量观测都没有时为 None。
    pub reasoning_output_tokens: Option<u64>,
}

/// 发布本轮的收尾观测帧：解码速率、用量与缓存命中率。
///
/// 运行期只在首个事件落地时发过一帧 `ttft_ms`，速率要等 provider 流收口才知道。这里用
/// 本轮 provider 流式实测耗时减去首字得到解码窗口，客户端据此就能算出与 TUI 同口径的速率；
/// 返回值由调用方随收尾事件一起落库。
pub(super) fn publish_turn_performance(
    sink: &RuntimeTurnEventSink<'_, impl RuntimeStore, impl EventPublisher, impl RuntimeClock>,
    interaction_id: &axon_domain::InteractionId,
    total_ms: u64,
    now_ms: u64,
) -> Option<TurnGenerationMetrics> {
    // 观测不到任何 provider 流边界时退回整轮墙钟：宁可口径退化，也不让速率整段消失。
    let readout = sink.generation_readout(Some(total_ms));
    let ttft_ms = readout.ttft_ms;
    let decode_ms = readout.decode_ms;
    if decode_ms.is_none() && ttft_ms.is_none() {
        return None;
    }
    let generation = turn_generation_metrics(sink, readout);
    // 观测帧按广播投递：会话级路由键与订阅端登记的键不必同源，
    // 而帧本身带 turnId，接收端据此即可确认归属，靠广播最不容易丢。
    let frame = axon_domain::RuntimeObservationFrame::new(now_ms, "", interaction_id.as_str())
        .with_generation(generation);
    let _ = sink.publish_turn_observation(&frame);
    Some(TurnGenerationMetrics {
        ttft_ms,
        decode_ms,
        // 没有实测样本时不写 0：客户端据此区分「没测到」与「测得 0」。
        output_tokens: (readout.output_tokens > 0).then_some(readout.output_tokens),
        reasoning_output_tokens: (readout.reasoning_output_tokens > 0)
            .then_some(readout.reasoning_output_tokens),
    })
}

/// 发布在跑轮的实时生成指标帧：每收口一条 provider 流补一帧。
///
/// 收尾帧只在整个回合结束时才发，在跑的这一轮在此之前不进会话级速率；按实测值即时补帧，
/// 速率才能与首字一样在生成过程中就可见并随之推进。帧按广播投递，与本轮收尾帧同一条路径。
pub(super) fn publish_stream_settled_generation(
    sink: &RuntimeTurnEventSink<'_, impl RuntimeStore, impl EventPublisher, impl RuntimeClock>,
    interaction_id: &axon_domain::InteractionId,
    now_ms: u64,
) {
    // 实时帧不给墙钟回退：整轮墙钟里混着工具执行，会把正在推进的速率压低。
    let readout = sink.generation_readout(None);
    if readout.decode_ms.is_none() {
        return;
    }
    let generation = turn_generation_metrics(sink, readout);
    let frame = axon_domain::RuntimeObservationFrame::new(now_ms, "", interaction_id.as_str())
        .with_generation(generation);
    let _ = sink.publish_turn_observation(&frame);
}

/// 生成指标快照：首字、解码窗口、本轮实测输出与最近一次用量观测。
fn turn_generation_metrics(
    sink: &RuntimeTurnEventSink<'_, impl RuntimeStore, impl EventPublisher, impl RuntimeClock>,
    readout: TurnGenerationReadout,
) -> axon_domain::GenerationMetrics {
    let usage = sink.last_usage().unwrap_or_default();
    axon_domain::GenerationMetrics {
        ttft_ms: readout.ttft_ms,
        decode_ms: readout.decode_ms,
        generation_speed_tps: None,
        total_input_tokens: usage.input_tokens,
        cache_read_input_tokens: usage.cached_input_tokens,
        cache_write_input_tokens: usage.cache_write_input_tokens,
        output_tokens: readout.output_tokens,
        reasoning_output_tokens: readout.reasoning_output_tokens,
        cache_hit_rate_basis_points: u16::try_from(cache_hit_rate_basis_points(&usage))
            .unwrap_or(u16::MAX),
        ..Default::default()
    }
}

/// 缓存命中率（万分比）：命中读取占全部输入的比例，与 TUI 同口径。
pub(super) fn cache_hit_rate_basis_points(
    usage: &interaction_turn_event_sink::TurnUsageSnapshot,
) -> u64 {
    let total = usage.input_tokens.saturating_add(usage.cached_input_tokens);
    if total == 0 {
        return 0;
    }
    usage
        .cached_input_tokens
        .saturating_mul(10_000)
        .saturating_div(total)
}

pub(super) fn interaction_transport_context(interaction: &InteractionRecord) -> serde_json::Value {
    let mut context = serde_json::json!({
        "channel": interaction.source.channel(),
        "destination": interaction.reply_route.destination(),
        "interactionId": interaction.interaction_id.as_str(),
        "workspaceId": interaction
            .scope
            .workspace_id()
            .map_or_else(String::new, |value| value.as_str().to_owned()),
        "primaryLaneId": interaction_session_lane_id(interaction)
            .unwrap_or_default(),
        "audience": interaction.audience.clone(),
    });
    // 入站附件以结构化路径进入 Provider turn（`transport_files` 读取 `files`），
    // 让图片成为 vision 输入、文件成为可读附件；为空时不写该键，保持既有载荷形状。
    if !interaction.attachments.is_empty() {
        context["files"] = serde_json::Value::Array(
            interaction
                .attachments
                .iter()
                .map(|path| serde_json::Value::String(path.clone()))
                .collect(),
        );
    }
    // 由投递项派生的主 turn 携带投递项 ID：渠道 egress 据此把正文留给持久收件箱
    // 投递，而不是直接出站，保证同一个投递项在所有渠道只投递一次；真实外部来源
    // 则保留 sourceMessageId 供渠道回帖。
    if let Some(notification_id) = delivery_item_notification_id(interaction) {
        context["deliveryNotificationId"] = serde_json::Value::String(notification_id);
    } else {
        context["sourceMessageId"] =
            serde_json::Value::String(interaction.source.source_message_id().to_owned());
    }
    context
}

/// 由投递项派生的主 turn 所对应的投递项通知 ID。
///
/// 覆盖两类系统投递：Goal/Execution 完成通知的 steer 主 turn，以及 Cron 任务派发
/// 的主 turn。普通用户回合返回 `None`，其回复仍由渠道 egress 直接出站。
pub(super) fn delivery_item_notification_id(interaction: &InteractionRecord) -> Option<String> {
    completion_steer_execution_id(interaction.source.source_message_id())
        .map(|execution_id| completion_notification_id(&execution_id))
}

pub(super) fn annotate_runtime_payload(
    payload_json: &str,
    interaction_id: &axon_domain::InteractionId,
    performance: Option<&axon_session_runtime::TurnPerformanceSnapshot>,
    generation: Option<TurnGenerationMetrics>,
) -> Result<String, String> {
    let mut payload: serde_json::Value =
        serde_json::from_str(payload_json).map_err(|error| error.to_string())?;
    let object = payload
        .as_object_mut()
        .ok_or_else(|| "Runtime 事件 payload 必须是 JSON 对象".to_owned())?;
    object.insert(
        "interactionId".to_owned(),
        serde_json::Value::String(interaction_id.as_str().to_owned()),
    );
    if let Some(performance) = performance {
        object.insert(
            "firstTokenPerformance".to_owned(),
            serde_json::to_value(performance).map_err(|error| error.to_string())?,
        );
    }
    if let Some(generation) = generation {
        // 与观测帧同名字段：客户端一套解析逻辑即可同时吃下实时帧与重放负载。
        object.insert(
            "generation".to_owned(),
            serde_json::to_value(generation).map_err(|error| error.to_string())?,
        );
    }
    serde_json::to_string(&payload).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{TurnGenerationMetrics, annotate_runtime_payload};

    #[test]
    fn annotated_payload_carries_generation_in_the_observation_shape() {
        // 字段名必须与实时观测帧一致（camelCase），否则重放时客户端解析不到首字与速率。
        let payload = serde_json::json!({ "status": "completed" }).to_string();
        let annotated = annotate_runtime_payload(
            &payload,
            &axon_domain::InteractionId::new("interaction-1").expect("交互标识"),
            None,
            Some(TurnGenerationMetrics {
                ttft_ms: Some(8_460),
                decode_ms: Some(10_000),
                output_tokens: Some(1_204),
                reasoning_output_tokens: Some(318),
            }),
        )
        .expect("注解应成功");
        let value: serde_json::Value = serde_json::from_str(&annotated).expect("注解后仍是 JSON");
        assert_eq!(value["interactionId"], "interaction-1");
        assert_eq!(value["generation"]["ttftMs"], 8_460);
        assert_eq!(value["generation"]["decodeMs"], 10_000);
        // 输出 token 随收尾负载落库：重放历史时速率分子不能只剩「没测到」。
        assert_eq!(value["generation"]["outputTokens"], 1_204);
        assert_eq!(value["generation"]["reasoningOutputTokens"], 318);

        // 没有指标时不产生该键：客户端据此知道这一轮确实没测到，而不是测到了 0。
        let bare = annotate_runtime_payload(
            &payload,
            &axon_domain::InteractionId::new("interaction-1").expect("交互标识"),
            None,
            None,
        )
        .expect("注解应成功");
        let value: serde_json::Value = serde_json::from_str(&bare).expect("注解后仍是 JSON");
        assert!(value.get("generation").is_none());
    }
}
