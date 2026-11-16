//! 车道的会话累计读数：把「这条车道一共花了多少」组装成协议载荷。
//!
//! 两个读数各有各的事实源，且都不是车道表本身：
//! - 用量：`rollout.sqlite3` 的账本，按车道当前的 run session 汇总。车道标识
//!   （`session-session-<hash>`）是复合车道的稳定身份，账本却记在 run session 名下，
//!   因此要先经 `sessions` 解析一层。
//! - 耗时：`inbox` 里已收口轮次的墙钟差，按车道求和即可。
//!
//! 两者都是「打开会话就要立刻有数」的读数，因此这里只做一次索引查与一次索引求和，
//! 不回放正文，也不解析单轮记录里的 JSON。

use std::collections::HashMap;
use std::path::Path;

use super::runtime_store;

/// 一页历史里每条 Interaction 的补充事实。
#[derive(Debug, Clone, Default)]
pub(super) struct InteractionFacts {
    /// 这一轮用过的工具名（逗号分隔、已去重）；老记录为空。
    pub tool_summary: String,
    /// 已收口轮次的墙钟用时（毫秒）；还在排队或读不到时间事实时为空。
    pub elapsed_ms: Option<u64>,
}

/// 单条历史的正文上限（字符）。
///
/// 索引只为一页历史服务，单条正文再长也不该把整页载荷撑到几 MB：超出即截断并如实
/// 标注，想看全文的人可以点开过程回放那条记录。
const ANSWER_TEXT_LIMIT_CHARS: usize = 16 * 1024;

/// 一次主键批次查补齐整页的聚合事实：工具动作摘要与本轮用时。
///
/// 工具名在写入事件时就已累加进 `inbox` 的 `tool_summary` 列，两个时间戳
/// 也在同一行上，所以这里既不碰事件表，也不扫历史。老记录工具列为空，界面据此退回
/// 状态短语；还没有收口结论的轮次用时字段不参与折叠。
pub(super) fn interaction_facts(
    axon_root: &Path,
    interaction_ids: &[&str],
) -> HashMap<String, InteractionFacts> {
    let mut facts = HashMap::new();
    if interaction_ids.is_empty() {
        return facts;
    }
    let store = runtime_store(axon_root);
    let Ok(map) = store.with_read_connection(|connection| {
        let placeholders = vec!["?"; interaction_ids.len()].join(",");
        let sql = format!(
            "SELECT interaction_id, tool_summary, updated_at_ms - received_at_ms \
             FROM inbox WHERE interaction_id IN ({placeholders})"
        );
        let mut statement = connection.prepare(&sql)?;
        let rows =
            statement.query_map(rusqlite::params_from_iter(interaction_ids.iter()), |row| {
                let elapsed: i64 = row.get(2)?;
                Ok((
                    row.get::<_, String>(0)?,
                    InteractionFacts {
                        tool_summary: row.get::<_, String>(1)?,
                        elapsed_ms: u64::try_from(elapsed).ok(),
                    },
                ))
            })?;
        let mut map = HashMap::new();
        for (interaction_id, fact) in rows.flatten() {
            map.insert(interaction_id, fact);
        }
        Ok(map)
    }) else {
        return facts;
    };
    facts = map;
    facts
}

/// 一页历史里每条 Interaction 的最终交付正文。
///
/// 正文只有一条来源：收口事件 `turn_completed` 载荷里的 `response`（它就是主回合的最终
/// 交付文本，按定义排在全部工具动作之后）。载荷里另带 `interactionId`，因此这里按会话
/// 取收口事件、在 Rust 侧认领到具体一条，不依赖 `session_events.interaction_id` 列：
/// 该列是后加的，既有库里的历史事件一律为空，按列过滤会一条都取不到。
///
/// 代价模型：一条车道只有「轮数」条收口事件（实测 54 轮 52 条），从新往旧取，凑齐这一页
/// 要的标识就停；超过 4 KiB 的载荷在写侧已外置，按引用只读回这一段。因此一次翻页是
/// 若干行索引加上少量外置读取，而不是几十万帧。
pub(super) fn answer_texts(
    axon_root: &Path,
    session_id: &str,
    interaction_ids: &[&str],
) -> HashMap<String, String> {
    let mut answers = HashMap::new();
    if interaction_ids.is_empty() {
        return answers;
    }
    let wanted: std::collections::HashSet<&str> = interaction_ids.iter().copied().collect();
    let store = runtime_store(axon_root);
    let Ok(session_id) = axon_domain::DaemonSessionId::new(session_id.to_owned()) else {
        return answers;
    };
    let path = store.event_log(&session_id).path();
    if !path.exists() {
        return answers;
    }
    let Ok(reader) = axon_eventlog::EventLogReader::open(&path) else {
        return answers;
    };
    // 从新往旧取，凑齐这一页要的标识就停：最新一页只读几条，老页才走完整条会话。
    for record in reader.records().iter().rev() {
        if answers.len() == wanted.len() {
            break;
        }
        if record.event_type.as_str() != "turn_completed" {
            continue;
        }
        let Some(interaction_id) = record
            .payload
            .get("interactionId")
            .and_then(serde_json::Value::as_str)
        else {
            continue;
        };
        if !wanted.contains(interaction_id) || answers.contains_key(interaction_id) {
            continue;
        }
        let Some(text) = record
            .payload
            .get("response")
            .and_then(serde_json::Value::as_str)
        else {
            continue;
        };
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        answers.insert(interaction_id.to_owned(), clamp_answer_text(text));
    }
    answers
}

/// 正文超出索引预算时截断，并留一句可执行的说明。
fn clamp_answer_text(text: &str) -> String {
    if text.chars().count() <= ANSWER_TEXT_LIMIT_CHARS {
        return text.to_owned();
    }
    let mut clamped: String = text.chars().take(ANSWER_TEXT_LIMIT_CHARS).collect();
    clamped.push_str("\n\n（这一轮的正文较长，这里只给到前段；点开过程可回放完整记录。）");
    clamped
}

/// 会话累计墙钟耗时（毫秒）：直接读 `session_metrics` 投影。
///
/// `inbox` 已不再持有处理状态，逐轮耗时只能由 `turn_metrics` 投影到
/// `session_metrics.total_active_ms`；这里读投影而不是回放事件，一次主键查找即可。
pub(super) fn settled_elapsed_ms(axon_root: &Path, session_id: &str) -> Option<u64> {
    let store = runtime_store(axon_root);
    store
        .with_read_connection(|connection| {
            connection.query_row(
                "SELECT total_active_ms FROM session_metrics WHERE session_id = ?1",
                [session_id],
                |row| row.get::<_, i64>(0),
            )
        })
        .ok()
        .and_then(|total| u64::try_from(total).ok())
}

/// 车道的累计用量载荷。
///
/// 车道还没有 run session 时读作全零而不是「没有结论」：新建的会话在第一轮跑起来之前
/// 就是这个状态，把读数藏起来会让「这一轮已经花了多少」在页面上完全看不见。
/// 真的读失败（账本打不开、列缺失且回填失败）才给 null，由界面按「未知」处理。
pub(super) fn session_usage_payload(axon_root: &Path, session_id: &str) -> serde_json::Value {
    let totals = match session_run_session_id(axon_root, session_id) {
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
/// `sessions` 单行查 + 一次 `rollout_turns` 最新行查就够；解析不到运行会话、
/// 或该会话还没有任何一轮时给 null，由界面回退配置默认值（那也是运行时会选的模型）。
pub(super) fn effective_model_payload(axon_root: &Path, session_id: &str) -> serde_json::Value {
    let Some(run_session_id) = session_run_session_id(axon_root, session_id) else {
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
pub(super) fn reflection_notice_payload(axon_root: &Path, session_id: &str) -> serde_json::Value {
    let Some(run_session_id) = session_run_session_id(axon_root, session_id) else {
        return serde_json::Value::Null;
    };
    // 反思提示随 thread 轮次落盘，只读接口直接从 thread 投影取。`axon_root` 的数据目录
    // 与 CLI 一致，直接复用同一实现。
    crate::run_commands::run_thread_store::latest_reflection_notice_at_root(
        axon_root,
        &run_session_id,
    )
    .map_or(serde_json::Value::Null, serde_json::Value::from)
}

/// 车道的运行时 session 标识。
///
/// 车道标识（`session-session-<hash>`）是复合车道的稳定身份，每轮的用量却记在该车道当前的
/// run session 名下（`sessions.run_session_id`）。会话累计读数因此要先把车道解析
/// 成 run session，再按它汇总；解析不到时返回 None。
fn session_run_session_id(axon_root: &Path, session_id: &str) -> Option<String> {
    let store = runtime_store(axon_root);
    store
        .with_read_connection(|connection| {
            connection.query_row(
                "SELECT run_session_id FROM sessions WHERE session_id = ?1",
                [session_id],
                |row| row.get::<_, String>(0),
            )
        })
        .ok()
}
