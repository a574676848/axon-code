//! 车道历史索引页的载荷组装。
//!
//! 一页历史要给三样东西：这一页有哪几轮、每轮最后答了什么、每轮花了多少。三者都不得
//! 回放正文：轮次清单来自控制面索引，最终正文取自该轮的收口事件，逐轮读数取自该轮自己的
//! `turn_metrics` 事件。因此一次翻页是「若干行索引读取」，与轮次正文体量无关。

use std::path::Path;

use axon_session::{InteractionCursor, InteractionQueryStore as _};

use super::session_readings;
use super::{InteractionView, MAX_SESSION_INTERACTION_LIMIT, TurnMetricsView, runtime_store};

/// 车道内 Interaction 列表载荷。
///
/// `before` 是反向分页游标：为空取最近一页，给出则取它之前的一页。
/// 客户端按「一次一条」向前翻，因此这里不需要额外的页号或总量。
///
/// 载荷同时给出整条车道的两个累计读数（token 用量与已收口轮次的墙钟耗时）：界面的
/// 会话读数要的是整条会话，而不是客户端恰好翻到的那一页，因此这两个数按车道聚合，
/// 与分页无关。取数失败不牵连历史列表：列表是主事实，累计读数缺了就不给。
pub fn session_interactions_payload(
    axon_root: &Path,
    session_id: &str,
    limit: u32,
    before: Option<&InteractionCursor>,
) -> Result<serde_json::Value, String> {
    let limit = limit.clamp(1, MAX_SESSION_INTERACTION_LIMIT);
    let records = runtime_store(axon_root)
        .interactions_for_session(session_id, limit, before)
        .map_err(|error| format!("读取车道 Interaction 失败：{error:?}"))?;
    let interaction_ids: Vec<&str> = records
        .iter()
        .map(|record| record.interaction_id.as_str())
        .collect();
    // 逐轮事实（读数、最终正文、动作摘要）来自同一次有界的尾部读取：代价与会话总长度无关。
    let facts = session_readings::page_facts(axon_root, session_id, &interaction_ids);
    let interactions: Vec<InteractionView> = records
        .iter()
        .map(|record| {
            let interaction_id = record.interaction_id.as_str().to_owned();
            let reading = facts.readings.get(&interaction_id);
            InteractionView {
                tool_summary: String::new(),
                answer_text: facts
                    .answers
                    .get(&interaction_id)
                    .cloned()
                    .unwrap_or_default(),
                tool_names: facts
                    .tool_names
                    .get(&interaction_id)
                    .cloned()
                    .unwrap_or_default(),
                metrics: reading
                    .filter(|reading| reading.has_facts())
                    .map(|reading| TurnMetricsView {
                        active_ms: reading.active_ms.unwrap_or(0),
                        ttft_ms: reading.ttft_ms,
                        generation_speed_tps_x10: reading
                            .generation_speed_tps
                            .map(tokens_per_second_x10),
                        tool_calls: reading.tool_calls,
                        input_tokens: reading.input_tokens,
                        cache_read_tokens: reading.cache_read_tokens,
                        cache_write_tokens: reading.cache_write_tokens,
                        output_tokens: reading.output_tokens,
                        reasoning_tokens: reading.reasoning_tokens,
                    }),
                elapsed_ms: reading.and_then(|reading| reading.active_ms),
                interaction_id,
                status: serde_json::to_value(record.status)
                    .ok()
                    .and_then(|value| value.as_str().map(str::to_owned))
                    .unwrap_or_else(|| "unknown".to_owned()),
                channel: record.source.channel().to_owned(),
                client: record.source.client().unwrap_or_default().to_owned(),
                received_at_ms: record.received_at_ms,
                content: record.content.clone(),
                attachments: record.attachments.clone(),
                // 索引只读记录本身：模型与强度是记录上的现成字段，取它们不必扫事件流。
                model: record.model.clone(),
                reasoning_effort: record.reasoning_effort.clone(),
            }
        })
        .collect();
    // 两个累计读数：用量读 `session_metrics` 投影，耗时是各轮墙钟之和。
    let session_usage = session_readings::session_usage_payload(axon_root, session_id);
    let session_elapsed_ms = session_readings::settled_elapsed_ms(axon_root, session_id)
        .map_or(serde_json::Value::Null, serde_json::Value::from);
    Ok(serde_json::json!({
        "sessionId": session_id,
        "interactions": interactions,
        "totalRecords": interactions.len(),
        // 会话级后台作业清单：与打开哪一轮无关，重开会话时据此还原标题栏的作业读数。
        "jobs": facts.jobs,
        "sessionUsage": session_usage,
        "sessionElapsedMs": session_elapsed_ms,
        // 车道当前模型的权威事实：只读该车道最新一轮记录，供输入坞显示真实模型。
        "effectiveModel": session_readings::effective_model_payload(axon_root, session_id),
        // 最近一次记忆反思的提示文本：桌面端据此显示「上一次记忆动作」。
        "reflectionNotice": session_readings::reflection_notice_payload(axon_root, session_id),
        "rawPayloadPersisted": false,
    }))
}

/// 生成速率换成 0.1 tok/s 的整数刻度。
///
/// 协议只用整数：浮点会让载荷失去 `Eq`，而界面的显示口径本来就是一位小数。
fn tokens_per_second_x10(value: f64) -> u64 {
    if !value.is_finite() || value <= 0.0 {
        return 0;
    }
    let scaled = (value * 10.0).round();
    if scaled >= u64::MAX as f64 {
        u64::MAX
    } else {
        scaled as u64
    }
}
