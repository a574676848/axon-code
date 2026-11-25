//! 车道历史索引页的载荷组装。
//!
//! 一页历史要给三样东西：这一页有哪几轮、每轮最后答了什么、每轮花了多少。三者都不得
//! 回放正文：轮次清单来自控制面索引，最终正文取自该轮的收口事件，逐轮读数取自该轮自己的
//! `turn_metrics` 事件。因此一次翻页是「若干行索引读取」，与轮次正文体量无关。

use std::path::Path;

use axon_domain::{InteractionRecord, InteractionStatus};
use axon_session::{InteractionCursor, InteractionQueryStore as _};

use super::runtime_store;
use super::session_readings;
use super::{InteractionView, MAX_SESSION_INTERACTION_LIMIT, TurnMetricsView, interactions_cache};

/// 只在别人回合里出现过的入站行：已收口，却没有自己的逐字稿。
///
/// 运行中追加的消息必须先落一条 durable Interaction（进程崩了也不丢），再去注入活跃回合的
/// 安全边界；注入成功后那句话的正文与过程全部写在**父回合**里，这一行再不会有自己的回答、
/// 工具行与 `turn_metrics` 投影。它不是一轮：列进索引页，正文末尾就会把同一句话再画一遍，
/// 而所有按索引读回合的入口（桌面、TUI、CLI、网关）都会跟着错。
///
/// 判据只用「这一轮没跑过」的正面证据：仍在排队（`received` / `processing`）的照常显示，
/// 它随时会成为真正的一轮；跑过但失败的轮次状态不是 `understood`，也不受影响。
fn is_injected_elsewhere(
    record: &InteractionRecord,
    answer_text: &str,
    tool_names: &[String],
    reading: Option<&session_readings::TurnReadings>,
) -> bool {
    record.status == InteractionStatus::Understood
        && answer_text.trim().is_empty()
        && tool_names.is_empty()
        && reading.is_none_or(|reading| !reading.has_facts())
}

/// 会话行事实：能力块与车道模型读数共用同一次读取。
///
/// 会话行不存在时给 `None`（索引页本身仍可读，界面据此知道这是未登记会话）。
fn session_row_facts(
    axon_root: &Path,
    session_id: &str,
) -> Result<Option<session_readings::SessionRowFacts>, String> {
    use axon_session::SessionKind;
    use rusqlite::OptionalExtension as _;
    let row = runtime_store(axon_root)
        .with_read_connection(|connection| {
            connection
                .query_row(
                    "SELECT s.kind, s.provider, s.model, s.model_name,
                            (SELECT COUNT(*) FROM inbox AS i WHERE i.session_id = s.session_id),
                            (SELECT i.interaction_id FROM inbox AS i WHERE i.session_id = s.session_id
                             ORDER BY i.received_at_ms DESC, i.interaction_id DESC LIMIT 1)
                     FROM sessions AS s WHERE s.session_id = ?1",
                    [session_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, Option<String>>(3)?,
                            row.get::<_, i64>(4)?,
                            row.get::<_, Option<String>>(5)?,
                        ))
                    },
                )
                .optional()
        })
        .map_err(|error| format!("读取会话能力事实失败：{error:?}"))?;
    let Some((kind, provider, model, model_name, count, latest)) = row else {
        return Ok(None);
    };
    let Some(kind) = SessionKind::parse(&kind) else {
        return Err(format!("会话 kind 取值非法：{kind}"));
    };
    let primary = kind == SessionKind::Primary;
    // 可写性：用户可以直接续聊的会话类型。目标执行保持只读，它由目标契约驱动，
    // 取消也只能由来源会话调用目标工具发起；cron 与子代理会话允许用户追加消息。
    let writable = matches!(
        kind,
        SessionKind::Primary | SessionKind::Cron | SessionKind::Subagent
    );
    let provider = provider.unwrap_or_default();
    let model = model.unwrap_or_default();
    let model_name = model_name.unwrap_or_default();
    let capability = serde_json::json!({
        "kind": kind.as_str(),
        // 来源只由会话类型映射，禁止用 `source_channel` 猜：同一会话可能被别端续聊。
        "origin": origin_of_kind(kind),
        "writable": writable,
        // 后台链不注入用户提问端口，因此只有主会话能答问。
        "canAnswerQuestions": primary,
        // 只有主 turn 有进程内取消令牌。目标执行不经过中断接口：它的取消只能由来源
        // 会话调用 Goal 工具发起；cron 与子会话同理不提供中断入口。
        "canInterrupt": primary,
        "provider": provider,
        "model": model,
        "modelName": model_name,
        "interactionCount": u64::try_from(count).unwrap_or(0),
        "latestInteractionId": latest.unwrap_or_default(),
    });
    Ok(Some(session_readings::SessionRowFacts {
        capability,
        provider,
        model,
        model_name,
    }))
}

/// 会话类型到来源的映射：一条会话只有一种来源，不随轮次变化。
fn origin_of_kind(kind: axon_session::SessionKind) -> &'static str {
    match kind {
        axon_session::SessionKind::Primary => "human",
        axon_session::SessionKind::Goal => "goal",
        axon_session::SessionKind::Cron => "cron",
        axon_session::SessionKind::Subagent => "subagent",
    }
}

/// 车道内 Interaction 列表载荷。
///
/// `before` 是反向分页游标：为空取最近一页，给出则取它之前的一页。
/// 客户端按「一次一条」向前翻，因此这里不需要额外的页号或总量。
///
/// 载荷同时给出整条车道的两个累计读数（token 用量与已收口轮次的墙钟耗时）：界面的
/// 会话读数要的是整条会话，而不是客户端恰好翻到的那一页，因此这两个数按车道聚合，
/// 与分页无关。取数失败不牵连历史列表：列表是主事实，累计读数缺了就不给。
///
/// `brief` 是翻页专用档：向前翻页只补历史轮次，会话能力块、作业清单、累计读数与
/// 车道最新事实都与页码无关，重复下发只是白读一遍线程。此档只返回 `sessionId`、
/// `interactions` 与 `totalRecords`，其余键缺省。
pub fn session_interactions_payload(
    axon_root: &Path,
    session_id: &str,
    limit: u32,
    before: Option<&InteractionCursor>,
    brief: bool,
) -> Result<serde_json::Value, String> {
    let limit = limit.clamp(1, MAX_SESSION_INTERACTION_LIMIT);
    let Some(cursor) = before else {
        // 首页走缓存：同一会话在同一数据水位上的同一页只组装一次，切回看过的会话直接命中。
        return interactions_cache::cached_page(axon_root, session_id, limit, brief, || {
            build_page(axon_root, session_id, limit, None, brief)
        });
    };
    build_page(axon_root, session_id, limit, Some(cursor), brief)
}

/// 组装一页索引载荷；缓存层只负责决定要不要调用它。
fn build_page(
    axon_root: &Path,
    session_id: &str,
    limit: u32,
    before: Option<&InteractionCursor>,
    brief: bool,
) -> Result<serde_json::Value, String> {
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
        .filter_map(|record| {
            let interaction_id = record.interaction_id.as_str().to_owned();
            let reading = facts.readings.get(&interaction_id);
            let answer_text = facts
                .answers
                .get(&interaction_id)
                .cloned()
                .unwrap_or_default();
            let tool_names = facts
                .tool_names
                .get(&interaction_id)
                .cloned()
                .unwrap_or_default();
            // 注入到别处的追加消息没有自己的逐字稿，不占一格历史。
            if is_injected_elsewhere(record, &answer_text, &tool_names, reading) {
                return None;
            }
            Some(InteractionView {
                answer_text,
                tool_names,
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
                elapsed_ms: reading
                    .filter(|reading| !reading.has_facts())
                    .and_then(|reading| reading.active_ms),
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
            })
        })
        .collect();
    // 翻页档到此为止：历史轮次已经足够，剩下的键都要再读一遍车道级投影。
    if brief {
        let total = interactions.len();
        return Ok(serde_json::json!({
            "sessionId": session_id,
            "interactions": interactions,
            "totalRecords": total,
        }));
    }
    // 两个累计读数：用量读 `session_metrics` 投影，耗时是各轮墙钟之和。
    let session_usage = session_readings::session_usage_payload(axon_root, session_id);
    let session_elapsed_ms = session_readings::settled_elapsed_ms(axon_root, session_id)
        .map_or(serde_json::Value::Null, serde_json::Value::from);
    /*
     * 会话级计划快照：日志尾部最近一次 `update_plan`。
     *
     * 计划坞是会话级事实，不能只靠「回放刚好读到那一帧」才出现；随索引一起下发之后，
     * 打开会话首屏就能立起当前进度。定向取从尾部倒着走，碰到最近一条即停，因此计划
     * 落在两千帧之前也照样下发。
     */
    let session_plan = facts.plan.as_ref().map_or(serde_json::Value::Null, |plan| {
        serde_json::to_value(plan).unwrap_or(serde_json::Value::Null)
    });
    /*
     * 会话行事实(能力块 + 车道模型)读一次；metadata 一侧只读最新一轮。
     *
     * 思考强度与记忆反思只存在于轮次 metadata：只认最新那一轮，尾窗读不到就当没有，
     * 既不回退整条 thread，也不拿更早的轮次顶替。
     */
    let row_facts = session_row_facts(axon_root, session_id)?;
    let latest_turn = session_readings::latest_turn_facts(axon_root, session_id);
    Ok(serde_json::json!({
        "sessionId": session_id,
        // 会话能力与身份事实：界面只读这一块，不再按渠道推断只读与可写。
        "session": row_facts.as_ref().map(|facts| facts.capability.clone()),
        "interactions": interactions,
        "totalRecords": interactions.len(),
        // 会话级后台作业清单：与打开哪一轮无关，按事件类型从日志尾部定向取，
        // 重开会话时据此还原标题栏的作业读数。
        "jobs": facts.jobs,
        "sessionUsage": session_usage,
        "sessionElapsedMs": session_elapsed_ms,
        "sessionPlan": session_plan,
        // 车道当前模型的权威事实：provider 与模型取会话行，强度取最新一轮 metadata。
        "effectiveModel": session_readings::effective_model_payload(
            row_facts.as_ref(),
            latest_turn.as_ref(),
        ),
        // 最近一次记忆反思的提示文本：桌面端据此显示「上一次记忆动作」。
        "reflectionNotice": session_readings::reflection_notice_payload(latest_turn.as_ref()),
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

#[cfg(test)]
#[path = "workbench_http_session_interactions_tests.rs"]
mod tests;
