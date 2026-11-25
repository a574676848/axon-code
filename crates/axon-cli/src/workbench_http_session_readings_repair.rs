//! 一页历史的事件面补帧：会话级事实定向取，逐轮事实有界尾扫加定向补。
//!
//! 投影表覆盖到的轮次完全不碰事件日志。剩下的按交互定向取自己的那几帧：帧索引里的
//! 交互指纹把解压范围收窄到这条交互，缺失的轮次全部命中即停，因此代价正比于这一页
//! 真正缺的那几条交互，与一条会话跑了多久无关。
//!
//! 会话级事实（计划快照、后台作业）不属于任何一轮，因此按事件类型从尾部定向取：倒着
//! 走，碰到最近一条计划就收，作业按标识各取最新一条，另给一个帧数上界兜住异常日志。
//! 尾窗依然扫一遍，它是窗口内逐轮事实的来源，也是定向取中途失败时的会话级兜底；两边
//! 按「定向优先、尾窗补缺」合并，同一个作业不会因为两条路径都读到而出现两行。

use std::collections::HashSet;
use std::path::Path;

use axon_eventlog::{InteractionRecord, RuntimeEventKind, TurnMetrics};

use super::super::workbench_http_session_jobs::JOB_FACT_MAX_IDS;
use super::runtime_store;
use super::{
    PageFacts, SessionJobFact, SessionPlanSnapshot, TurnReadings, clamp_answer_text,
    collect_job_fact, plan_snapshot, tool_call_name,
};

/// 会话级事实定向取最多回看的帧数。
///
/// 计划与作业都不属于任何一轮，只能按事件类型从尾部倒推，因此必须有硬上界：取两千帧
/// 的十倍，正常会话在这段距离里一定见过最近一次计划与最近若干次作业，而异常日志最多
/// 把一次翻页拖成两万帧解压，不随会话总长度增长。命中即停，常见情况远小于这个上界。
const SESSION_FACT_SCAN_MAX_FRAMES: usize = 20_000;

/// 补一页历史的事件面事实：会话级定向取，逐轮有界尾扫，再为仍然缺失的轮次定向补帧。
pub(super) fn fill(
    axon_root: &Path,
    path: &Path,
    interaction_ids: &[&str],
    tail_frames: usize,
    facts: &mut PageFacts,
) {
    let wanted: HashSet<&str> = interaction_ids.iter().copied().collect();
    // 会话级事实先定向取：计划与作业的权威来源是这一趟，不是尾窗。
    collect_session_facts(path, &mut facts.plan, &mut facts.jobs);
    let mut tail_plan: Option<SessionPlanSnapshot> = None;
    let mut tail_jobs: Vec<SessionJobFact> = Vec::new();
    collect_tail_window(
        path,
        tail_frames,
        &wanted,
        facts,
        &mut tail_plan,
        &mut tail_jobs,
    );
    absorb_fallback(&mut facts.plan, &mut facts.jobs, tail_plan, tail_jobs);
    collect_directed(axon_root, path, interaction_ids, facts);
}

/// 尾部窗口：窗口内顺带读到的逐轮事实，外加定向取没给出的会话级事实。
///
/// `max_frames` 是硬上限。页内还有未收口的轮次时这里也不放大：那些轮次的答案与读数
/// 由定向补帧负责。会话级事实只往 `plan` 与 `jobs` 里写，由调用方按「定向优先」合并，
/// 因此尾窗读到同一份事实时只会补空缺字段，不会覆盖定向取已经给出的结论。
fn collect_tail_window(
    path: &Path,
    max_frames: usize,
    wanted: &HashSet<&str>,
    facts: &mut PageFacts,
    plan: &mut Option<SessionPlanSnapshot>,
    jobs: &mut Vec<SessionJobFact>,
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
                if plan.is_none()
                    && let Some(snapshot) = plan_snapshot(&record.payload)
                {
                    *plan = Some(snapshot);
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
                collect_job_fact(record, jobs);
            }
            _ => {}
        }
        true
    });
}

/// 会话级事实定向取：从尾部倒着走，计划取最近一条，作业按标识各取最新一类事实。
///
/// 三类事实合并成一次倒扫：它们都贴着日志尾部，分三次扫等于把同一段解压三遍。停止条件
/// 是「计划已取到，且启动与收口两类作业都取满条数」；取不满就一路退到帧数上界。读失败
/// （索引缺失、帧解码失败）时保留已经拿到的部分，不报错，也不清空。
fn collect_session_facts(
    path: &Path,
    plan: &mut Option<SessionPlanSnapshot>,
    jobs: &mut Vec<SessionJobFact>,
) {
    let mut started: HashSet<String> = HashSet::new();
    let mut completed: HashSet<String> = HashSet::new();
    let _ = axon_eventlog::read_tail_scan(path, SESSION_FACT_SCAN_MAX_FRAMES, |record| {
        match record.event_type.as_str() {
            "wire_event" => {
                // 计划只认最近一条：倒扫里第一次读到的就是最新那一份，不必再往后走。
                if plan.is_none()
                    && let Some(snapshot) = plan_snapshot(&record.payload)
                {
                    *plan = Some(snapshot);
                }
            }
            "job_started" | "job_completed" => {
                /*
                 * 同一作业的同一类事实只认最近一条：倒扫里先到的是较新的记录，重试留下
                 * 的旧记录不再覆盖它。启动与收口各留一份，`collect_job_fact` 按标识合并
                 * 成一行，与尾窗路径完全同一套规则。
                 */
                let Some(job_id) = record
                    .payload
                    .get("jobId")
                    .and_then(serde_json::Value::as_str)
                else {
                    return true;
                };
                let seen = if record.event_type.as_str() == "job_completed" {
                    &mut completed
                } else {
                    &mut started
                };
                if seen.insert(job_id.to_owned()) {
                    collect_job_fact(record, jobs);
                }
            }
            _ => {}
        }
        // 三类事实都取满才收工：谁也说不准更早的地方还有没有计划或仍在跑的作业。
        !(plan.is_some()
            && started.len() >= JOB_FACT_MAX_IDS
            && completed.len() >= JOB_FACT_MAX_IDS)
    });
}

/// 尾窗兜底：定向取已经给出的作业行与字段保持原样，尾窗只补它没读到的部分。
///
/// 定向取中途失败（索引缺失、帧解码失败）时，尾窗是唯一还能给出会话级事实的来源，
/// 因此不能因为它排在后面就整体丢弃；反过来，定向取读到的值更权威，不能被更旧的
/// 尾窗记录覆盖，否则同一个作业会凭旧记录改写终态。
fn absorb_fallback(
    plan: &mut Option<SessionPlanSnapshot>,
    jobs: &mut Vec<SessionJobFact>,
    tail_plan: Option<SessionPlanSnapshot>,
    tail_jobs: Vec<SessionJobFact>,
) {
    if plan.is_none() {
        *plan = tail_plan;
    }
    for row in tail_jobs {
        match jobs.iter_mut().find(|known| known.job_id == row.job_id) {
            Some(known) => {
                if known.kind.is_empty() {
                    known.kind = row.kind;
                }
                if known.started_at_ms.is_none() {
                    known.started_at_ms = row.started_at_ms;
                }
                if known.status.is_none() {
                    known.status = row.status;
                }
                if known.elapsed_ms.is_none() {
                    known.elapsed_ms = row.elapsed_ms;
                }
                if known.exit_code.is_none() {
                    known.exit_code = row.exit_code;
                }
            }
            None => jobs.push(row),
        }
    }
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
