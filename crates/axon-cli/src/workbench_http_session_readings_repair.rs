//! 一页历史的事件面补帧：一段有界尾扫，加上按缺失交互定向补。
//!
//! 投影表覆盖到的轮次完全不碰事件日志。剩下的按交互定向取自己的那几帧：帧索引里的
//! 交互指纹把解压范围收窄到这条交互，缺失的轮次全部命中即停，因此代价正比于这一页
//! 真正缺的那几条交互，与一条会话跑了多久无关。会话级事实（计划快照、后台作业）不
//! 属于任何一轮，仍从尾部一小段固定窗口取，窗口不随页内是否还有未收口的轮次放大。

use std::collections::HashSet;
use std::path::Path;

use axon_eventlog::{InteractionRecord, RuntimeEventKind, TurnMetrics};

use super::runtime_store;
use super::{
    PageFacts, TurnReadings, clamp_answer_text, collect_job_fact, plan_snapshot, tool_call_name,
};

/// 补一页历史的事件面事实：先有界尾扫，再为仍然缺失的轮次定向补帧。
pub(super) fn fill(
    axon_root: &Path,
    path: &Path,
    interaction_ids: &[&str],
    tail_frames: usize,
    facts: &mut PageFacts,
) {
    let wanted: HashSet<&str> = interaction_ids.iter().copied().collect();
    collect_tail_window(path, tail_frames, &wanted, facts);
    collect_directed(axon_root, path, interaction_ids, facts);
}

/// 尾部窗口：会话级事实，加上窗口内顺带读到的逐轮事实。
///
/// `max_frames` 是硬上限。页内还有未收口的轮次时这里也不放大：那些轮次的答案与读数
/// 由定向补帧负责，窗口只保证会话级事实（计划、作业）不必回放整条会话。
fn collect_tail_window(
    path: &Path,
    max_frames: usize,
    wanted: &HashSet<&str>,
    facts: &mut PageFacts,
) {
    /*
     * 只扫到「本页最早那一轮」为止：代价正比于这一页真实占用的帧数，与日志总长无关。
     */
    let mut current: Option<String> = None;
    let mut covered: HashSet<String> = HashSet::new();
    let _ = axon_eventlog::read_tail_scan(path, max_frames, |record| {
        match record.event_type.as_str() {
            "turn_completed" => {
                let Some(interaction_id) = record
                    .payload
                    .get("interactionId")
                    .and_then(serde_json::Value::as_str)
                else {
                    current = None;
                    return true;
                };
                // 已经覆盖完整页、又遇到更早的一轮：这一页到此为止。
                if wanted.contains(interaction_id) {
                    covered.insert(interaction_id.to_owned());
                } else if !covered.is_empty() {
                    return false;
                }
                current = Some(interaction_id.to_owned());
                if let Some(answer) = record
                    .payload
                    .get("response")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                {
                    facts
                        .answers
                        .entry(interaction_id.to_owned())
                        .or_insert_with(|| clamp_answer_text(answer));
                }
            }
            "turn_metrics" => {
                let Some(owner) = current.as_deref().filter(|owner| wanted.contains(owner)) else {
                    return true;
                };
                let Ok(metrics) = serde_json::from_value::<TurnMetrics>(record.payload.clone())
                else {
                    return true;
                };
                facts
                    .readings
                    .entry(owner.to_owned())
                    .or_insert_with(|| TurnReadings::from_metrics(&metrics));
            }
            "wire_event" => {
                /*
                 * 计划是会话级事实：反向扫描遇到的第一个 `planUpdate` 就是最新那一份，
                 * 取到即止。热路径只做一次 `kind.type` 比较，非计划帧不反序列化、不分配。
                 */
                if facts.plan.is_none()
                    && let Some(plan) = plan_snapshot(&record.payload)
                {
                    facts.plan = Some(plan);
                }
                let (Some(owner), Some(name)) =
                    (current.as_deref(), tool_call_name(&record.payload))
                else {
                    return true;
                };
                if !wanted.contains(owner) {
                    return true;
                }
                let names = facts.tool_names.entry(owner.to_owned()).or_default();
                if !names.iter().any(|known| known == &name) {
                    names.push(name);
                }
            }
            "job_started" | "job_completed" => {
                // 作业是会话级事实：它不属于某一轮，因此在收口边界之外单独收集。
                collect_job_fact(record, &mut facts.jobs);
            }
            _ => {}
        }
        true
    });
}

/// 按缺失的交互定向补帧：只碰这些交互自己的帧。
///
/// 只有已收口的轮次才写下 `turn_completed` 与 `turn_metrics`，在途轮次一定还没有这两条
/// 事件；把它们一并交给索引倒推，只会让索引把这条交互的帧解到自己的帧预算上，因此先
/// 按在途表排除。定向读取读失败时保持已经拿到的部分，不回退到整段尾扫。
fn collect_directed(
    axon_root: &Path,
    path: &Path,
    interaction_ids: &[&str],
    facts: &mut PageFacts,
) {
    let missing: Vec<&str> = interaction_ids
        .iter()
        .copied()
        .filter(|id| !facts.answers.contains_key(*id) || !facts.readings.contains_key(*id))
        .collect();
    if missing.is_empty() {
        return;
    }
    let in_flight = in_flight_interactions(axon_root, &missing);
    let answers: Vec<&str> = missing
        .iter()
        .copied()
        .filter(|id| !facts.answers.contains_key(*id))
        .collect();
    for entry in read_latest(path, &answers, RuntimeEventKind::TurnCompleted, &in_flight) {
        let Some(answer) = entry
            .record
            .payload
            .get("response")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
        else {
            continue;
        };
        // 投影行已经在里面时保留投影值：事件日志只兜底，绝不覆盖投影的结论。
        facts
            .answers
            .entry(entry.interaction_id)
            .or_insert_with(|| clamp_answer_text(answer));
    }
    let readings: Vec<&str> = missing
        .iter()
        .copied()
        .filter(|id| !facts.readings.contains_key(*id))
        .collect();
    for entry in read_latest(path, &readings, RuntimeEventKind::TurnMetrics, &in_flight) {
        let Ok(metrics) = serde_json::from_value::<TurnMetrics>(entry.record.payload) else {
            continue;
        };
        facts
            .readings
            .entry(entry.interaction_id)
            .or_insert_with(|| TurnReadings::from_metrics(&metrics));
    }
}

/// 一次批量定向读：取每个交互各自最近一条指定类型的事件。
///
/// 在途轮次直接剔掉，它们不可能有收口事件。事件日志侧自带帧预算，索引缺失时退化为
/// 单趟顺序解压并在同一处封顶，读失败按「一条也没读到」处理。
fn read_latest(
    path: &Path,
    interaction_ids: &[&str],
    kind: RuntimeEventKind,
    in_flight: &HashSet<String>,
) -> Vec<InteractionRecord> {
    let wanted: Vec<&str> = interaction_ids
        .iter()
        .copied()
        .filter(|id| !in_flight.contains(*id))
        .collect();
    if wanted.is_empty() {
        return Vec::new();
    }
    axon_eventlog::read_latest_per_interaction(path, &wanted, kind.as_str()).unwrap_or_default()
}

/// 还在途（排队或运行中）的轮次。
///
/// 在途表只放排队与运行中的行，终态行搬进历史表，因此在这里读到一行就说明这一轮还没
/// 收口，不需要再看任何状态词表。
fn in_flight_interactions(axon_root: &Path, interaction_ids: &[&str]) -> HashSet<String> {
    if interaction_ids.is_empty() {
        return HashSet::new();
    }
    let store = runtime_store(axon_root);
    let Ok(in_flight) = store.with_read_connection(|connection| {
        let placeholders = vec!["?"; interaction_ids.len()].join(",");
        let sql = format!("SELECT inbox_id FROM work_items WHERE inbox_id IN ({placeholders})");
        let mut statement = connection.prepare(&sql)?;
        let rows = statement
            .query_map(rusqlite::params_from_iter(interaction_ids.iter()), |row| {
                row.get::<_, Option<String>>(0)
            })?;
        let mut in_flight = HashSet::new();
        for interaction_id in rows.flatten().flatten() {
            in_flight.insert(interaction_id);
        }
        Ok(in_flight)
    }) else {
        return HashSet::new();
    };
    in_flight
}
