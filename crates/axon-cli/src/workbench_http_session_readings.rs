//! 车道的会话读数：把「这条会话一共花了多少」和「每一轮花了多少」组装成协议载荷。
//!
//! 两类事实各有一个落点，且都不是车道表本身：
//! - **会话累计**：`session_metrics` 投影，按 `session_id` 一行。它是 `turn_metrics`
//!   与作业事件的投影，可由事件日志整体重建（见 `docs/dev/storage-architecture.md`
//!   §4.11），因此读取只做一次主键查，不回放正文、不解析单轮记录。
//! - **逐轮读数**：每条 Interaction 自己的 `turn_metrics` 事件。索引页要为十几轮各给
//!   一行读数（用时、token、首字、速率），逐轮整段回放会把一页正文全部解压一遍，因此
//!   按帧索引的交互指纹从尾部定向取，一条交互通常只解压它自己的最后几帧。
//!
//! 两个读数都不变量子化：会话累计与分页无关，逐轮读数只服务本页。

use std::collections::HashMap;
use std::path::Path;

use axon_domain::DaemonSessionId;
use axon_eventlog::TurnMetrics;

use super::runtime_store;

/// 一页历史里每条 Interaction 的读数。
///
/// 字段与 `turn_metrics` 事件一一对应：未测得的事实保持为空，绝不用 0 冒充。
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct TurnReadings {
    /// 本轮墙钟用时（毫秒）。
    pub active_ms: Option<u64>,
    /// 本轮工具调用次数。
    pub tool_calls: u32,
    /// 首字延迟；未测得时为空。
    pub ttft_ms: Option<u64>,
    /// 生成速率（token/s）；未测得时为空。
    pub generation_speed_tps: Option<f64>,
    /// 未缓存输入。
    pub input_tokens: u64,
    /// 缓存读取。
    pub cache_read_tokens: u64,
    /// 缓存写入。
    pub cache_write_tokens: u64,
    /// 输出。
    pub output_tokens: u64,
    /// 推理输出。
    pub reasoning_tokens: u64,
}

impl TurnReadings {
    /// 由一条 `turn_metrics` 事件得到读数。
    fn from_metrics(metrics: &TurnMetrics) -> Self {
        Self {
            active_ms: Some(metrics.active_ms),
            tool_calls: metrics.tool_calls,
            ttft_ms: metrics.ttft_ms,
            generation_speed_tps: measured_or_derived_speed(metrics),
            input_tokens: metrics.input_tokens,
            cache_read_tokens: metrics.cache_read_tokens,
            cache_write_tokens: metrics.cache_write_tokens,
            output_tokens: metrics.output_tokens,
            reasoning_tokens: metrics.reasoning_tokens,
        }
    }

    /// 是否还有可展示的读数：一个事实都没有时整段不上屏，不画空指标行。
    pub(super) fn has_facts(&self) -> bool {
        self.active_ms.is_some_and(|value| value > 0)
            || self.ttft_ms.is_some()
            || self.generation_speed_tps.is_some()
            || self.input_tokens > 0
            || self.cache_read_tokens > 0
            || self.cache_write_tokens > 0
            || self.output_tokens > 0
            || self.reasoning_tokens > 0
    }
}

/// 生成速率：优先用 Provider 实测值，缺测时按本轮自己的解码窗口还原。
///
/// 有的轮次采不到 Provider 速率（CLI 直跑、遥测被关掉），但同一条事件里既有输出 token
/// 也有总耗时与首字延迟，解码窗口就是两者之差。不还原的话同一份历史里「有的轮有速率、
/// 有的轮没有」，读者只会把它读成读数丢了；还原值取自本轮实测计数，不掺任何别的轮次。
fn measured_or_derived_speed(metrics: &TurnMetrics) -> Option<f64> {
    if let Some(measured) = metrics.generation_speed_tps
        && measured.is_finite()
        && measured > 0.0
    {
        return Some(measured);
    }
    let emitted = metrics
        .output_tokens
        .saturating_add(metrics.reasoning_tokens);
    if emitted == 0 {
        return None;
    }
    let decode_ms = metrics
        .active_ms
        .saturating_sub(metrics.ttft_ms.unwrap_or(0));
    if decode_ms == 0 {
        return None;
    }
    let speed = (emitted as f64) * 1_000.0 / (decode_ms as f64);
    if speed.is_finite() && speed > 0.0 {
        Some(speed)
    } else {
        None
    }
}

/// 单条历史的正文上限（字符）。
///
/// 索引只为一页历史服务，单条正文再长也不该把整页载荷撑到几 MB：超出即截断并如实
/// 标注，想看全文的人可以点开过程回放那条记录。
const ANSWER_TEXT_LIMIT_CHARS: usize = 16 * 1024;

/// 尾部扫描的硬上限：防住异常日志把一次翻页拖成整条会话解压。
const PAGE_TAIL_MAX_FRAMES: usize = 16_384;

/// 一页历史的逐轮事实：读数、最终正文与动作摘要。
#[derive(Debug, Clone, Default)]
pub(super) struct PageFacts {
    /// 逐轮读数。
    pub readings: HashMap<String, TurnReadings>,
    /// 逐轮最终交付正文。
    pub answers: HashMap<String, String>,
    /// 逐轮用过的工具名，按出现顺序、已去重。
    pub tool_names: HashMap<String, Vec<String>>,
    /// 会话级后台作业清单：启动与终态各一条事件，属于整条会话而不是某一轮。
    pub jobs: Vec<SessionJobFact>,
}

/// 会话级后台作业的一行事实。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SessionJobFact {
    /// 服务端作业标识。
    pub job_id: String,
    /// 作业类型（shell / subagent 等）。
    pub kind: String,
    /// 启动时刻（Unix 毫秒）；只有终态事件时为空。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at_ms: Option<u64>,
    /// 终态名称：completed / failed / killed / timeout / refusal / unresponsive。
    /// 只有启动事件时缺省，界面按「在跑」呈现。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// 从启动到终态的耗时；未测得时缺省，不用 0 冒充。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u64>,
    /// 退出码（若可得）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

/// 读一页历史的逐轮事实：一次有界的尾部读取，不随会话体量增长。
pub(super) fn page_facts(
    axon_root: &Path,
    session_id: &str,
    interaction_ids: &[&str],
) -> PageFacts {
    let mut facts = PageFacts::default();
    if interaction_ids.is_empty() {
        return facts;
    }
    let wanted: std::collections::HashSet<&str> = interaction_ids.iter().copied().collect();
    let store = runtime_store(axon_root);
    let Ok(session) = DaemonSessionId::new(session_id.to_owned()) else {
        return facts;
    };
    let path = store.event_log(&session).path();
    if path.exists() {
        // 只扫到「本页最早那一轮」为止：代价正比于这一页真实占用的帧数，与日志总长无关。
        let mut current: Option<String> = None;
        let mut covered: std::collections::HashSet<String> = std::collections::HashSet::new();
        let _ = axon_eventlog::read_tail_scan(&path, PAGE_TAIL_MAX_FRAMES, |record| {
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
                    let Some(owner) = current.as_deref().filter(|owner| wanted.contains(owner))
                    else {
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
    // 反向攒出来的工具名要翻回模型真实动作顺序，折叠摘要才按发生先后读。
    for names in facts.tool_names.values_mut() {
        names.reverse();
    }
    // 没有指标事件的轮次（CLI 直跑、指标写入失败、老会话）只补用时：工作项的创建与
    // 终态时间差就是这一轮的墙钟，与指标事件里的 `activeMs` 同一口径。
    let missing: Vec<&str> = interaction_ids
        .iter()
        .copied()
        .filter(|id| !facts.readings.contains_key(*id))
        .collect();
    for (interaction_id, elapsed) in work_item_elapsed_ms(axon_root, &missing) {
        facts.readings.entry(interaction_id).or_default().active_ms = Some(elapsed);
    }
    facts
}

/// 收下一条作业事件；启动与终态合并成同一行。
fn collect_job_fact(record: &axon_eventlog::RuntimeEventRecord, jobs: &mut Vec<SessionJobFact>) {
    match record.event_type.as_str() {
        "job_started" => {
            let Ok(reference) =
                serde_json::from_value::<axon_eventlog::JobRef>(record.payload.clone())
            else {
                return;
            };
            let row = job_row(jobs, &reference.job_id);
            row.kind = reference.kind;
            row.started_at_ms = Some(reference.started_at_ms);
        }
        "job_completed" => {
            let Ok(outcome) =
                serde_json::from_value::<axon_eventlog::JobOutcome>(record.payload.clone())
            else {
                return;
            };
            let row = job_row(jobs, &outcome.job_id);
            row.status = Some(outcome.status);
            row.elapsed_ms = outcome.elapsed_ms;
            row.exit_code = outcome.exit_code;
        }
        _ => {}
    }
}

/// 取（必要时新建）某个作业标识对应的那一行。
fn job_row<'a>(jobs: &'a mut Vec<SessionJobFact>, job_id: &str) -> &'a mut SessionJobFact {
    if let Some(position) = jobs.iter().position(|row| row.job_id == job_id) {
        return &mut jobs[position];
    }
    jobs.push(SessionJobFact {
        job_id: job_id.to_owned(),
        ..SessionJobFact::default()
    });
    let last = jobs.len() - 1;
    &mut jobs[last]
}
/// 工作项终态时间差：没有指标事件时唯一还能如实给出的用时。
fn work_item_elapsed_ms(axon_root: &Path, interaction_ids: &[&str]) -> HashMap<String, u64> {
    if interaction_ids.is_empty() {
        return HashMap::new();
    }
    let store = runtime_store(axon_root);
    let Ok(map) = store.with_read_connection(|connection| {
        let placeholders = vec!["?"; interaction_ids.len()].join(",");
        let sql = format!(
            "SELECT inbox_id, finished_at_ms - started_at_ms FROM work_item_history \
             WHERE kind='turn' AND inbox_id IN ({placeholders})"
        );
        let mut statement = connection.prepare(&sql)?;
        let rows =
            statement.query_map(rusqlite::params_from_iter(interaction_ids.iter()), |row| {
                let elapsed: i64 = row.get(1)?;
                Ok((row.get::<_, String>(0)?, elapsed))
            })?;
        let mut map = HashMap::new();
        for (interaction_id, value) in rows.flatten() {
            if let Ok(value) = u64::try_from(value) {
                map.insert(interaction_id, value);
            }
        }
        Ok(map)
    }) else {
        return HashMap::new();
    };
    map
}

/// 一帧里是否是一次工具调用；是则给出工具名。
fn tool_call_name(payload: &serde_json::Value) -> Option<String> {
    let kind = payload.get("kind")?;
    if kind.get("type").and_then(serde_json::Value::as_str) != Some("toolCall") {
        return None;
    }
    let call = kind.get("payload")?;
    let name = call
        .get("toolName")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())?;
    Some(name.to_owned())
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
/// `inbox` 已不再持有处理状态，逐轮耗时由 `turn_metrics` 投影到
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
/// 按车道 `session_id` 直接读 `session_metrics`（会话投影就是这个键）：账号账本在
/// 该来源已不在读路径上，绕经 run session 只会读到一行不存在的键并恒给全零。
///
/// 总量口径与界面一致：输入、缓存读写、输出与推理相加。运维与计费的分桶仍各自保留。
///
/// 还没有任何轮次的会话读作全零而不是「未知」：新建会话在第一轮跑起来之前就是这个
/// 状态，把读数藏起来会让「这一轮已经花了多少」在页面上完全看不见。真的读失败
///（库打不开、投影损坏）才给 null，由界面按「未知」处理。
pub(super) fn session_usage_payload(axon_root: &Path, session_id: &str) -> serde_json::Value {
    let Ok(session) = DaemonSessionId::new(session_id.to_owned()) else {
        return serde_json::Value::Null;
    };
    let store = runtime_store(axon_root);
    let metrics = match store.session_metrics(&session) {
        Ok(metrics) => metrics,
        Err(_) => return serde_json::Value::Null,
    };
    let usage = metrics.unwrap_or_default();
    let total_tokens = usage
        .total_input_tokens
        .saturating_add(usage.total_cache_read_tokens)
        .saturating_add(usage.total_cache_write_tokens)
        .saturating_add(usage.total_output_tokens)
        .saturating_add(usage.total_reasoning_tokens);
    serde_json::json!({
        "totalTokens": total_tokens,
        "inputTokens": usage.total_input_tokens,
        "cacheReadTokens": usage.total_cache_read_tokens,
        "cacheWriteTokens": usage.total_cache_write_tokens,
        "outputTokens": usage.total_output_tokens,
        "reasoningTokens": usage.total_reasoning_tokens,
    })
}

/// 车道当前模型的权威事实：只读该车道运行会话的**最新一条**轮次记录。
///
/// 性能约束（CRITICAL）：不遍历历史轮次，也不回放 Interaction 事件帧。一次
/// `sessions` 单行查 + 一次 `sessions` 最新行查就够；解析不到运行会话、
/// 或该会话还没有任何一轮时给 null，由界面回退配置默认值（那也是运行时会选的模型）。
pub(super) fn effective_model_payload(axon_root: &Path, session_id: &str) -> serde_json::Value {
    let Some(run_session_id) = session_run_session_id(axon_root, session_id) else {
        return serde_json::Value::Null;
    };
    let Ok(Some(facts)) =
        crate::run_commands::run_thread_store::session_model_at_root(axon_root, &run_session_id)
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
/// 只有「最新一轮记录」还留在运行会话名下（`sessions.run_session_id`）：模型事实与
/// 反思提示按它对 thread 反查。会话累计与逐轮读数不再经这一层，它们直接按车道键读
/// 投影与事件。
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
