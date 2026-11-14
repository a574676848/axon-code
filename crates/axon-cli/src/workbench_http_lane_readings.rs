//! 车道的会话累计读数：把「这条车道一共花了多少」组装成协议载荷。
//!
//! 两个读数各有各的事实源，且都不是车道表本身：
//! - 用量：`rollout.sqlite3` 的账本，按车道当前的 run session 汇总。车道标识
//!   （`session-lane-<hash>`）是复合车道的稳定身份，账本却记在 run session 名下，
//!   因此要先经 `session_runtimes` 解析一层。
//! - 耗时：`jarvis_interactions` 里已收口轮次的墙钟差，按车道求和即可。
//!
//! 两者都是「打开会话就要立刻有数」的读数，因此这里只做一次索引查与一次索引求和，
//! 不回放正文，也不解析单轮记录里的 JSON。

use std::path::Path;

use super::runtime_store;

/// 一页历史里每条 Interaction 的工具动作摘要（逗号分隔的工具名）。
///
/// 一次主键批次查覆盖整页：工具名在写入事件时就已累加进 `jarvis_interactions`
/// 的 `tool_summary` 列，所以这里既不碰事件表，也不扫历史。老记录该列为空，
/// 界面据此退回状态短语。
pub(super) fn tool_summaries(
    axon_root: &Path,
    interaction_ids: &[&str],
) -> std::collections::HashMap<String, String> {
    let mut summaries = std::collections::HashMap::new();
    if interaction_ids.is_empty() {
        return summaries;
    }
    let store = runtime_store(axon_root);
    let Ok(connection) = rusqlite::Connection::open(store.path()) else {
        return summaries;
    };
    let placeholders = vec!["?"; interaction_ids.len()].join(",");
    let sql = format!(
        "SELECT interaction_id, tool_summary FROM jarvis_interactions \
         WHERE interaction_id IN ({placeholders})"
    );
    let Ok(mut statement) = connection.prepare(&sql) else {
        return summaries;
    };
    let Ok(rows) = statement.query_map(rusqlite::params_from_iter(interaction_ids.iter()), |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    }) else {
        return summaries;
    };
    for (interaction_id, tool_summary) in rows.flatten() {
        summaries.insert(interaction_id, tool_summary);
    }
    summaries
}

/// 车道上已收口轮次的累计墙钟耗时（毫秒）。
///
/// 每轮的耗时取「到达 → 收尾」这段墙钟差：`jarvis_interactions` 里一行就是一轮，两个
/// 时间戳都是网关自己写的，因此按车道求和即可，不必回放正文，也不必解析事件载荷。
/// 仍在排队或在跑的轮次不参与：它们还没收尾，读数由界面按实时帧自己补，避免同一轮
/// 在基线与增量里各算一次。
pub(super) fn settled_elapsed_ms(axon_root: &Path, lane_id: &str) -> Option<u64> {
    let store = runtime_store(axon_root);
    let connection = rusqlite::Connection::open(store.path()).ok()?;
    connection
        .query_row(
            "SELECT COALESCE(SUM(updated_at_ms - received_at_ms), 0) FROM jarvis_interactions \
             WHERE lane_id = ?1 AND status NOT IN ('received', 'processing')",
            [lane_id],
            |row| row.get::<_, i64>(0),
        )
        .ok()
        .and_then(|total| u64::try_from(total).ok())
}

/// 车道的累计用量载荷。
///
/// 车道还没有 run session 时读作全零而不是「没有结论」：新建的会话在第一轮跑起来之前
/// 就是这个状态，把读数藏起来会让「这一轮已经花了多少」在页面上完全看不见。
/// 真的读失败（账本打不开、列缺失且回填失败）才给 null，由界面按「未知」处理。
pub(super) fn session_usage_payload(axon_root: &Path, lane_id: &str) -> serde_json::Value {
    let totals = match lane_run_session_id(axon_root, lane_id) {
        Some(run_session) => {
            crate::run_commands::load_session_usage_totals_from_root(axon_root, &run_session).ok()
        }
        None => Some(crate::run_commands::SessionUsageTotals::default()),
    };
    totals.map_or(serde_json::Value::Null, |usage| {
        serde_json::json!({
            "totalTokens": usage.total_tokens,
            "inputTokens": usage.input_tokens,
            "cacheReadTokens": usage.cache_read_tokens,
            "cacheWriteTokens": usage.cache_write_tokens,
            "outputTokens": usage.output_tokens,
            "reasoningTokens": usage.reasoning_tokens,
        })
    })
}

/// 车道当前模型的权威事实：只读该车道运行会话的**最新一条**轮次记录。
///
/// 性能约束（CRITICAL）：不遍历历史轮次，也不回放 Interaction 事件帧。一次
/// `session_runtimes` 单行查 + 一次 `rollout_turns` 最新行查就够；解析不到运行会话、
/// 或该会话还没有任何一轮时给 null，由界面回退配置默认值（那也是运行时会选的模型）。
pub(super) fn effective_model_payload(axon_root: &Path, lane_id: &str) -> serde_json::Value {
    let Some(run_session_id) = lane_run_session_id(axon_root, lane_id) else {
        return serde_json::Value::Null;
    };
    let Ok(Some(facts)) =
        crate::run_commands::load_latest_session_model_from_root(axon_root, &run_session_id)
    else {
        return serde_json::Value::Null;
    };
    serde_json::json!({
        "provider": facts.provider,
        "model": facts.model,
        "name": facts.model_name,
        "reasoningEffort": facts.reasoning_effort,
    })
}

/// 该车道最近一轮的记忆反思提示；读不到时返回 `Null`。
///
/// 与 `effective_model_payload` 同一读法：先把车道解析成 run session，再读它最新
/// 一轮记录里的 `reflectionNotice`。反思是轮后异步治理的产物，这里给的是「最近一次
/// 已完成治理」的结果，界面按「上一次记忆动作」呈现，不承诺与当前轮同步。
pub(super) fn reflection_notice_payload(axon_root: &Path, lane_id: &str) -> serde_json::Value {
    let Some(run_session_id) = lane_run_session_id(axon_root, lane_id) else {
        return serde_json::Value::Null;
    };
    crate::run_commands::load_latest_reflection_notice_from_root(axon_root, &run_session_id)
        .map_or(serde_json::Value::Null, serde_json::Value::from)
}

/// 车道的运行时 session 标识。
///
/// 车道标识（`session-lane-<hash>`）是复合车道的稳定身份，每轮的用量却记在该车道当前的
/// run session 名下（`session_runtimes.run_session_id`）。会话累计读数因此要先把车道解析
/// 成 run session，再按它汇总；解析不到时返回 None。
fn lane_run_session_id(axon_root: &Path, lane_id: &str) -> Option<String> {
    let store = runtime_store(axon_root);
    let connection = rusqlite::Connection::open(store.path()).ok()?;
    connection
        .query_row(
            "SELECT run_session_id FROM session_runtimes WHERE session_id = ?1",
            [lane_id],
            |row| row.get::<_, String>(0),
        )
        .ok()
}
