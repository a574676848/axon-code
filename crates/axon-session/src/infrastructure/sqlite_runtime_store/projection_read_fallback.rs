//! 投影落后时的读路径回源：上下文快照与会话累计指标共用一套落后判定与有界回源。
//!
//! 投影已经异步化：写路径只投递，进程内单线程 worker 按批执行。写返回后立刻读投影，可能
//! 读到上一批甚至空行。因此在投影读路径上加一道闸门：只有「已投影到的末序号」低于
//! 「事件水位」时才回源事件日志，并且回源必须同时满足三条约束：
//!
//! 1. 落后判定一次查询拿到两个数，不为判定多走一次数据库往返。
//! 2. 回源只读不写：读路径回写投影表会把一次读变成写放大，还要抢写锁。
//! 3. 回源结果绝不倒退：日志读数与投影行按时刻取较新的一份；读不全或读失败一律退回
//!    投影值。
//!
//! 快照按日志尾部定向取最近一条可用的用量帧，命中即停。累计指标按水位之后的序号区间
//! 定向读，逐轮口径与 `session_metrics_repository` 的累加完全一致：只认 `turn_metrics`
//! 事件。已经在 `turn_activity` 里落过读数的轮次不再重复累加，这一条是必须的：水位记的
//! 是末序号，落后窗口恰好等于尚未投影的事件，区间里的指标全部累加即可。
//!
//! 模块由同目录的 `context_snapshot_repository` 声明：投影读回源是从快照读路径长出来的
//! 共用能力，两条读路径因此共用同一套水位、上界与留痕规则。

use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use axon_domain::DaemonSessionId;
use axon_eventlog::{EventLogError, RuntimeEventRecord, TurnMetrics, TurnOutcome};
use rusqlite::{OptionalExtension, params};

use super::super::SqliteRuntimeStore;
use super::super::session_metrics_repository::{SessionMetricsSnapshot, read_metrics};
use super::{SOURCE_USAGE, UsageReading};
use crate::domain::ContextSnapshot;
use crate::ports::RuntimeStoreError;

/// 快照回源最多回看的帧数。
///
/// 用量帧是流式帧里最密的一类，最近一条几乎总在日志最末几帧里。上界只用来兜住异常日志：
/// 尾部连续若干帧都不是用量帧时到此为止，退回投影值。
const SNAPSHOT_SCAN_MAX_FRAMES: usize = 256;

/// 快照回源的耗时预算（毫秒）：超出即放弃本次回源。
const SNAPSHOT_SCAN_BUDGET_MS: u128 = 20;

/// 累计指标回源允许的最大序号跨度（事件条数）。
///
/// 水位与事件尾相距很远（投影长时间停摆、日志被重建）时不再逐条回源：那种情形下正确动作
/// 是重建投影，而不是让一次读背负整段历史。跨度内的定向读与阈值成正比，与日志总长无关。
const METRICS_GAP_MAX_EVENTS: u64 = 2_048;

/// 累计指标回源的健康阈值（毫秒）：超出只留痕，结果仍采用。
///
/// 硬上界是上面的序号跨度：读完再丢掉只会同时牺牲延迟与正确性，退回的恰好是落后值。
/// 这一条只用来标记病态 IO（冷缓存、磁盘排队），供线上判断回源该不该降级。
const METRICS_SCAN_WARN_MS: u128 = 25;

/// 回源留痕的会话级限流窗口（毫秒）。
const FALLBACK_LOG_WINDOW_MS: u64 = 60_000;

/// 限流状态的最大条目数；超出即整表清空，绝不让它随会话数无界增长。
const FALLBACK_LOG_MAX_ENTRIES: usize = 1_024;

/// 用量帧的事件类型，与 `context_snapshot_projection` 取同一口径。
const WIRE_EVENT_TYPE: &str = "wire_event";
/// 用量帧的 `kind.type`。
const USAGE_KIND: &str = "usageUpdate";
/// 帧记录时刻字段。
const RECORDED_AT_FIELD: &str = "recordedAtMs";
/// 当前上下文占用字段。
const CURRENT_CONTEXT_FIELD: &str = "currentContextTokens";
/// 上下文窗口字段。
const CONTEXT_WINDOW_FIELD: &str = "contextWindow";
/// 上下文构成的三个字段。
const COMPOSITION_FIELDS: [&str; 3] = [
    "systemPromptTokens",
    "toolDefinitionTokens",
    "conversationTokens",
];

/// 单轮指标的事件类型。
const TURN_METRICS_EVENT_TYPE: &str = "turn_metrics";

/// 直方图桶边界（毫秒）：取值与 `session_metrics_repository` 完全一致，回源与投影才能
/// 落到同一排桶里。
const HISTOGRAM_BOUNDS_MS: [u64; 15] = [
    1, 2, 5, 10, 20, 50, 100, 200, 500, 1_000, 2_000, 5_000, 10_000, 30_000, 60_000,
];

/// 直方图桶数：15 个上界加一个溢出桶。
const HISTOGRAM_BUCKETS: usize = 16;

/// 回源留痕的限流表：键是「类别加会话标识」，值是上次留痕时刻。
static FALLBACK_LOG: LazyLock<Mutex<HashMap<String, u64>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// 投影水位：已投影到的末序号与事件水位。
///
/// 批首序号不大于它的事件批次都已被投影过，因此「批首序号小于事件水位」只说明投影没有
/// 覆盖到日志尾，是一个偏保守的落后判据（最后一批已经投影完时也会命中）。保守是有意的：
/// 漏判会让读路径继续给旧值，误判的代价只是一次有界回源，而回源结果本身要过时刻取新。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ProjectionLag {
    /// 已投影到的末序号；从未投影过时为 0。
    last_projected_seq: u64,
    /// 事件水位：控制面已分配的最后一条序号。
    last_event_sequence: u64,
}

impl ProjectionLag {
    /// 由两个水位构造：事件水位在前，已投影到的末序号在后。
    pub(super) const fn new(last_event_sequence: u64, last_projected_seq: u64) -> Self {
        Self {
            last_projected_seq,
            last_event_sequence,
        }
    }

    /// 投影是否落后于事件水位。
    pub(super) fn is_behind(&self) -> bool {
        self.last_projected_seq < self.last_event_sequence
    }

    /// 水位之后还有多少条事件。
    fn pending_events(&self) -> u64 {
        self.last_event_sequence
            .saturating_sub(self.last_projected_seq)
    }
}

/// 一次查询同时取回「已投影到的末序号」与「事件水位」。
///
/// 判定只走这一条语句：分两次查会让两个数落在不同的读事务里，判定结果介于两者之间时
/// 反而更容易误判。会话行不存在时返回 `None`。
pub(super) fn projection_lag(
    connection: &rusqlite::Connection,
    session_id: &DaemonSessionId,
) -> Result<Option<ProjectionLag>, RuntimeStoreError> {
    connection
        .query_row(
            "SELECT s.last_event_sequence, COALESCE(p.last_projected_seq, 0) \
               FROM sessions AS s \
               LEFT JOIN projection_state AS p ON p.session_id = s.session_id \
              WHERE s.session_id = ?1",
            params![session_id.as_str()],
            |row| {
                Ok(ProjectionLag {
                    last_event_sequence: count_of(row.get(0)?),
                    last_projected_seq: count_of(row.get(1)?),
                })
            },
        )
        .optional()
        .map_err(|error| RuntimeStoreError::Storage(error.to_string()))
}

/// 投影落后时按日志尾部补齐上下文快照。
///
/// 只取最近一条可用用量帧：它比投影行新才采用，否则原样返回投影值。读失败与读不到都退回
/// 投影值，绝不为了补一个读数让请求变慢或失败。
pub(super) fn freshen_snapshot(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    projected: Option<ContextSnapshot>,
    lag: &ProjectionLag,
) -> Option<ContextSnapshot> {
    report_observed(session_id, "上下文快照", lag);
    let path = store.event_log(session_id).path();
    match latest_usage_reading(&path) {
        Ok(Some(reading)) => Some(with_reading(projected, reading)),
        Ok(None) => projected,
        Err(error) => {
            report_failed(session_id, "上下文快照", &error.to_string());
            projected
        }
    }
}

/// 从日志尾部倒序找最近一条可用用量帧；帧数上界与耗时预算任一触顶即收工。
fn latest_usage_reading(path: &std::path::Path) -> Result<Option<UsageReading>, EventLogError> {
    let mut found: Option<UsageReading> = None;
    let started = Instant::now();
    axon_eventlog::read_tail_scan(path, SNAPSHOT_SCAN_MAX_FRAMES, |record| {
        if record.event_type.as_str() == WIRE_EVENT_TYPE
            && let Some(reading) = usage_reading_of(record)
        {
            // 倒扫里先到的是较新的记录：取到即止，不必再往前翻。
            found = Some(reading);
            return false;
        }
        started.elapsed().as_millis() < SNAPSHOT_SCAN_BUDGET_MS
    })?;
    Ok(found)
}

/// 一条用量帧是否构成可用的上下文读数。
///
/// 与投影同口径：只认真正带当前占用的帧，0 与缺失同义（那不是「占用为 0」）。帧自己的
/// 记录时刻缺失时用记录的产生时刻顶替，不编造时刻。
fn usage_reading_of(record: &RuntimeEventRecord) -> Option<UsageReading> {
    let kind = record.payload.get("kind")?;
    if kind.get("type").and_then(serde_json::Value::as_str) != Some(USAGE_KIND) {
        return None;
    }
    let payload = kind.get("payload")?;
    let current_context_tokens = positive(payload.get(CURRENT_CONTEXT_FIELD))?;
    Some(UsageReading {
        current_context_tokens,
        context_window: positive(payload.get(CONTEXT_WINDOW_FIELD)),
        composition: composition_of(payload),
        observed_at_ms: recorded_at_ms(&record.payload).unwrap_or(record.emitted_at_ms),
    })
}

/// 把一条日志读数并进投影行：只有更晚的读数才覆盖，构成与窗口随读数一起替换。
fn with_reading(projected: Option<ContextSnapshot>, reading: UsageReading) -> ContextSnapshot {
    let mut snapshot = projected.unwrap_or_default();
    let fresher = snapshot
        .updated_at_ms
        .is_none_or(|known| reading.observed_at_ms >= known);
    if snapshot.current_context_tokens.is_some() && !fresher {
        return snapshot;
    }
    snapshot.current_context_tokens = Some(reading.current_context_tokens);
    snapshot.context_window = reading.context_window;
    // 构成与占用是同一份读数：新读数没带构成时留空，不拿旧构成拼一个新数字。
    snapshot.composition = reading.composition;
    snapshot.updated_at_ms = Some(
        snapshot
            .updated_at_ms
            .map_or(reading.observed_at_ms, |known| {
                known.max(reading.observed_at_ms)
            }),
    );
    snapshot.source = Some(SOURCE_USAGE.to_owned());
    snapshot
}

/// 投影落后时按事件日志补齐会话累计指标。
///
/// 投影行缺失但日志里有未投影轮次时同样给出结果：那正是「投影还没跑起来就读」的场景。
pub(super) fn freshen_metrics(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    projected: Option<SessionMetricsSnapshot>,
    lag: &ProjectionLag,
) -> Option<SessionMetricsSnapshot> {
    report_observed(session_id, "会话累计指标", lag);
    let Some(delta) = metrics_delta(store, session_id, lag) else {
        return projected;
    };
    match projected {
        Some(mut snapshot) => {
            delta.apply(&mut snapshot);
            Some(snapshot)
        }
        None if !delta.is_empty() => {
            let mut snapshot = SessionMetricsSnapshot::default();
            delta.apply(&mut snapshot);
            Some(snapshot)
        }
        None => None,
    }
}

/// 水位之后那一小段事件里的累计增量；读不全、跨度越界或读失败时返回 `None`。
fn metrics_delta(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    lag: &ProjectionLag,
) -> Option<MetricsDelta> {
    let pending = lag.pending_events();
    if pending == 0 {
        return Some(MetricsDelta::default());
    }
    if pending > METRICS_GAP_MAX_EVENTS {
        report_failed(
            session_id,
            "会话累计指标",
            &format!("水位跨度 {pending} 超出回源上界 {METRICS_GAP_MAX_EVENTS}"),
        );
        return None;
    }
    let limit = usize::try_from(pending).ok()?;
    let path = store.event_log(session_id).path();
    let started = Instant::now();
    let records = match axon_eventlog::read_after(&path, lag.last_projected_seq, limit) {
        Ok(records) => records,
        Err(error) => {
            report_failed(session_id, "会话累计指标", &error.to_string());
            return None;
        }
    };
    /*
     * 读全才用：首尾序号必须正好卡住水位区间。日志被截断、序号被重排或水位超前于日志时，
     * 缺一段就是缺一轮，宁可退回投影值也不能给出偏小的总量。
     */
    let complete = records.len() == limit
        && records
            .first()
            .is_some_and(|record| record.sequence == lag.last_projected_seq.saturating_add(1))
        && records
            .last()
            .is_some_and(|record| record.sequence == lag.last_event_sequence);
    if !complete {
        report_failed(
            session_id,
            "会话累计指标",
            &format!("水位区间读不全：期望 {limit} 条，实得 {}", records.len()),
        );
        return None;
    }
    if started.elapsed().as_millis() > METRICS_SCAN_WARN_MS {
        report_slow(
            session_id,
            &format!("耗时超出健康阈值 {METRICS_SCAN_WARN_MS}ms"),
        );
    }
    let turns: Vec<(String, TurnMetrics)> = records.iter().filter_map(turn_metrics_of).collect();
    if turns.is_empty() {
        return Some(MetricsDelta::default());
    }
    let owners: Vec<&str> = turns.iter().map(|(owner, _)| owner.as_str()).collect();
    let Some(counted) = counted_interactions(store, session_id, &owners) else {
        report_failed(session_id, "会话累计指标", "已计入轮次的查证失败");
        return None;
    };
    let mut delta = MetricsDelta::default();
    for (owner, metrics) in &turns {
        // 已经在投影里计过数的轮次不再累加：水位是批首序号，最后一批通常早已投影完。
        if counted.contains(owner) {
            continue;
        }
        delta.add(metrics);
    }
    Some(delta)
}

/// 一条事件是否是一轮的指标；是则给出归属轮次与指标。
///
/// 归属取记录的交互列，老记录没有该列时回落到载荷里的 `turnId`。两者都取不到时丢掉这条
/// 记录：无法证明它有没有被投影计入，宁可少算也不能重复累加。
fn turn_metrics_of(record: &RuntimeEventRecord) -> Option<(String, TurnMetrics)> {
    if record.event_type.as_str() != TURN_METRICS_EVENT_TYPE {
        return None;
    }
    let metrics = serde_json::from_value::<TurnMetrics>(record.payload.clone()).ok()?;
    let owner = record
        .interaction_id
        .as_deref()
        .filter(|owner| !owner.is_empty())
        .map(str::to_owned)
        .or_else(|| (!metrics.turn_id.is_empty()).then(|| metrics.turn_id.clone()))?;
    Some((owner, metrics))
}

/// 已经在投影里计过数的轮次。
///
/// 判据是 `turn_activity` 里已经有这一轮的读数：三个投影同批执行，`turn_activity` 先于
/// 累计指标，因此「累计指标计过这一轮」必然推出「`turn_activity` 有它的读数」。反过来的
/// 缺口（读数写了但累计指标写失败）本轮次不会被补，那属于重建入口的职责。
///
/// 查证失败返回 `None`：拿不到已计入集合就无法证明不会重复累加，此时放弃回源。
fn counted_interactions(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    owners: &[&str],
) -> Option<HashSet<String>> {
    if owners.is_empty() {
        return Some(HashSet::new());
    }
    let unique: Vec<&str> = owners
        .iter()
        .copied()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let mut bindings: Vec<&str> = Vec::with_capacity(unique.len() + 1);
    bindings.push(session_id.as_str());
    bindings.extend(unique.iter().copied());
    let placeholders = vec!["?"; unique.len()].join(",");
    let sql = format!(
        "SELECT interaction_id FROM turn_activity \
          WHERE session_id = ?1 AND metrics_json IS NOT NULL \
            AND interaction_id IN ({placeholders})"
    );
    let found = store.with_read_connection(|connection| {
        let mut statement = connection.prepare(&sql)?;
        let rows = statement.query_map(rusqlite::params_from_iter(bindings.iter()), |row| {
            row.get::<_, String>(0)
        })?;
        let mut found = HashSet::new();
        for interaction_id in rows.flatten() {
            found.insert(interaction_id);
        }
        Ok(found)
    });
    found.ok()
}

/// 投影落后时按日志补齐的累计增量。
///
/// 字段与 `session_metrics_repository::accumulate_turn_metrics` 一一对应：同一批
/// `turn_metrics` 交给投影与交给这里，得到的总量必须一致。首末轮时刻、渠道分布与
/// 「更新于」不参与回源：读路径不展示它们，事件面上的渠道对老记录也不可得。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct MetricsDelta {
    turn_count: u64,
    turn_succeeded: u64,
    turn_failed: u64,
    turn_interrupted: u64,
    total_active_ms: u64,
    total_provider_ms: u64,
    total_tool_ms: u64,
    max_turn_ms: u64,
    ttft_sum_ms: u64,
    ttft_samples: u64,
    ttft_max_ms: u64,
    ttft_histogram: Vec<u64>,
    turn_histogram: Vec<u64>,
    total_input_tokens: u64,
    total_output_tokens: u64,
    total_reasoning_tokens: u64,
    total_cache_read_tokens: u64,
    total_cache_write_tokens: u64,
    total_tool_calls: u64,
}

impl MetricsDelta {
    /// 这一批增量是否什么都没加；轮次数是唯一入口，它为 0 就说明一条轮次也没加。
    fn is_empty(&self) -> bool {
        self.turn_count == 0
    }

    /// 把一轮指标叠加进增量；逐字段口径与投影的累加完全一致。
    fn add(&mut self, metrics: &TurnMetrics) {
        self.turn_count = self.turn_count.saturating_add(1);
        match metrics.outcome {
            TurnOutcome::Succeeded => {
                self.turn_succeeded = self.turn_succeeded.saturating_add(1);
            }
            TurnOutcome::Failed => self.turn_failed = self.turn_failed.saturating_add(1),
            TurnOutcome::Interrupted => {
                self.turn_interrupted = self.turn_interrupted.saturating_add(1);
            }
        }
        self.total_active_ms = self.total_active_ms.saturating_add(metrics.active_ms);
        self.total_provider_ms = self
            .total_provider_ms
            .saturating_add(metrics.provider_duration_ms);
        self.total_tool_ms = self.total_tool_ms.saturating_add(metrics.tool_ms);
        self.max_turn_ms = self.max_turn_ms.max(metrics.active_ms);
        if let Some(ttft) = metrics.ttft_ms {
            self.ttft_sum_ms = self.ttft_sum_ms.saturating_add(ttft);
            self.ttft_samples = self.ttft_samples.saturating_add(1);
            self.ttft_max_ms = self.ttft_max_ms.max(ttft);
            add_bucket(&mut self.ttft_histogram, ttft);
        }
        add_bucket(&mut self.turn_histogram, metrics.active_ms);
        self.total_input_tokens = self.total_input_tokens.saturating_add(metrics.input_tokens);
        self.total_output_tokens = self
            .total_output_tokens
            .saturating_add(metrics.output_tokens);
        self.total_reasoning_tokens = self
            .total_reasoning_tokens
            .saturating_add(metrics.reasoning_tokens);
        self.total_cache_read_tokens = self
            .total_cache_read_tokens
            .saturating_add(metrics.cache_read_tokens);
        self.total_cache_write_tokens = self
            .total_cache_write_tokens
            .saturating_add(metrics.cache_write_tokens);
        self.total_tool_calls = self
            .total_tool_calls
            .saturating_add(u64::from(metrics.tool_calls));
    }

    /// 把增量叠加到投影行上；只做累加与取大，不覆盖任何单调量。
    fn apply(&self, snapshot: &mut SessionMetricsSnapshot) {
        snapshot.turn_count = snapshot.turn_count.saturating_add(self.turn_count);
        snapshot.turn_succeeded = snapshot.turn_succeeded.saturating_add(self.turn_succeeded);
        snapshot.turn_failed = snapshot.turn_failed.saturating_add(self.turn_failed);
        snapshot.turn_interrupted = snapshot
            .turn_interrupted
            .saturating_add(self.turn_interrupted);
        snapshot.total_active_ms = snapshot
            .total_active_ms
            .saturating_add(self.total_active_ms);
        snapshot.total_provider_ms = snapshot
            .total_provider_ms
            .saturating_add(self.total_provider_ms);
        snapshot.total_tool_ms = snapshot.total_tool_ms.saturating_add(self.total_tool_ms);
        snapshot.max_turn_ms = snapshot.max_turn_ms.max(self.max_turn_ms);
        snapshot.ttft_sum_ms = snapshot.ttft_sum_ms.saturating_add(self.ttft_sum_ms);
        snapshot.ttft_samples = snapshot.ttft_samples.saturating_add(self.ttft_samples);
        snapshot.ttft_max_ms = snapshot.ttft_max_ms.max(self.ttft_max_ms);
        merge_histogram(&mut snapshot.ttft_histogram, &self.ttft_histogram);
        merge_histogram(&mut snapshot.turn_histogram, &self.turn_histogram);
        snapshot.total_input_tokens = snapshot
            .total_input_tokens
            .saturating_add(self.total_input_tokens);
        snapshot.total_output_tokens = snapshot
            .total_output_tokens
            .saturating_add(self.total_output_tokens);
        snapshot.total_reasoning_tokens = snapshot
            .total_reasoning_tokens
            .saturating_add(self.total_reasoning_tokens);
        snapshot.total_cache_read_tokens = snapshot
            .total_cache_read_tokens
            .saturating_add(self.total_cache_read_tokens);
        snapshot.total_cache_write_tokens = snapshot
            .total_cache_write_tokens
            .saturating_add(self.total_cache_write_tokens);
        snapshot.total_tool_calls = snapshot
            .total_tool_calls
            .saturating_add(self.total_tool_calls);
    }
}

/// 把一段直方图并进另一段；长度不足时补齐到统一桶数。
fn merge_histogram(target: &mut Vec<u64>, delta: &[u64]) {
    if delta.is_empty() {
        return;
    }
    if target.len() < HISTOGRAM_BUCKETS {
        target.resize(HISTOGRAM_BUCKETS, 0);
    }
    for (index, count) in delta.iter().enumerate() {
        if let Some(slot) = target.get_mut(index) {
            *slot = slot.saturating_add(*count);
        }
    }
}

/// 记一个样本进直方图；桶边界与投影一致，落在哪个桶由「首个大于它的上界」决定。
fn add_bucket(histogram: &mut Vec<u64>, value_ms: u64) {
    if histogram.len() < HISTOGRAM_BUCKETS {
        histogram.resize(HISTOGRAM_BUCKETS, 0);
    }
    let index = HISTOGRAM_BOUNDS_MS
        .iter()
        .position(|bound| value_ms < *bound)
        .unwrap_or(HISTOGRAM_BUCKETS - 1);
    if let Some(slot) = histogram.get_mut(index) {
        *slot = slot.saturating_add(1);
    }
}

impl SqliteRuntimeStore {
    /// 读取会话累计指标：投影优先，投影落后时按事件日志补齐水位之后的轮次。
    ///
    /// 落后判定与投影读各用一条语句；只有真的落后时才碰事件日志。回源有三重上界：帧数
    /// 上限、序号跨度上限与耗时预算，读不全就退回投影值。读路径不回写投影表：补齐只对本次
    /// 调用生效，投影仍由投影 worker 或重建入口推进。
    pub fn session_metrics_with_fallback(
        &self,
        session_id: &DaemonSessionId,
    ) -> Result<Option<SessionMetricsSnapshot>, RuntimeStoreError> {
        let lag = {
            let connection = self.connection()?;
            projection_lag(&connection, session_id)?
        };
        let projected = read_metrics(self, session_id)?;
        let Some(lag) = lag.filter(ProjectionLag::is_behind) else {
            return Ok(projected);
        };
        Ok(freshen_metrics(self, session_id, projected, &lag))
    }
}

/// 回源留痕的类别：同一会话每一类各自限流，互不吞掉。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LogKind {
    /// 回源发生。
    Observed,
    /// 回源失败，本次退回投影值。
    Failed,
    /// 回源完成但耗时超出健康阈值，结果仍采用。
    Slow,
}

impl LogKind {
    /// 限流键里的类别前缀。
    const fn tag(self) -> &'static str {
        match self {
            Self::Observed => "observed",
            Self::Failed => "failed",
            Self::Slow => "slow",
        }
    }
}

/// 回源发生留痕：同一会话在同一窗口内只记一条，避免每请求刷屏。
fn report_observed(session_id: &DaemonSessionId, subject: &str, lag: &ProjectionLag) {
    if !allow_log(LogKind::Observed, session_id.as_str()) {
        return;
    }
    axon_logger::debug(&format!(
        "投影落后，已按事件日志回源{subject}：session={} projected={} tail={}",
        session_id.as_str(),
        lag.last_projected_seq,
        lag.last_event_sequence
    ));
}

/// 回源失败留痕：读不全或读失败，本次已退回投影值。
fn report_failed(session_id: &DaemonSessionId, subject: &str, reason: &str) {
    if !allow_log(LogKind::Failed, session_id.as_str()) {
        return;
    }
    axon_logger::warning(&format!(
        "投影回源失败，已退回投影值：subject={subject} session={} reason={reason}",
        session_id.as_str()
    ));
}

/// 回源偏慢留痕：结果仍然采用，只标记这一次回源不便宜。
fn report_slow(session_id: &DaemonSessionId, reason: &str) {
    if !allow_log(LogKind::Slow, session_id.as_str()) {
        return;
    }
    axon_logger::warning(&format!(
        "投影回源偏慢，结果仍采用：session={} reason={reason}",
        session_id.as_str()
    ));
}

/// 限流闸门：同一会话的同一类留痕在一个窗口内只放行一条。
fn allow_log(kind: LogKind, session_id: &str) -> bool {
    let now = now_ms();
    let key = format!("{}:{session_id}", kind.tag());
    let mut seen = FALLBACK_LOG.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(last) = seen.get(&key)
        && now.saturating_sub(*last) < FALLBACK_LOG_WINDOW_MS
    {
        return false;
    }
    if seen.len() >= FALLBACK_LOG_MAX_ENTRIES {
        seen.clear();
    }
    seen.insert(key, now);
    true
}

/// 当前时刻（Unix 毫秒）；系统时钟早于纪元时按 0 处理。
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

/// 帧记录时刻；缺失或非法时返回 `None`。
fn recorded_at_ms(envelope: &serde_json::Value) -> Option<u64> {
    envelope
        .get(RECORDED_AT_FIELD)
        .and_then(serde_json::Value::as_i64)
        .and_then(|value| u64::try_from(value).ok())
}

/// 上限读数：0 与缺失同义（没有读到），不能当成「上下文占用为 0」。
fn positive(value: Option<&serde_json::Value>) -> Option<u64> {
    value
        .and_then(serde_json::Value::as_u64)
        .filter(|value| *value > 0)
}

/// 上下文构成：只收帧里真实出现的三个字段，全是缺失时留空。
fn composition_of(payload: &serde_json::Value) -> Option<serde_json::Value> {
    let mut fields = serde_json::Map::new();
    for field in COMPOSITION_FIELDS {
        if let Some(value) = payload.get(field).and_then(serde_json::Value::as_u64) {
            fields.insert(field.to_owned(), serde_json::Value::from(value));
        }
    }
    (!fields.is_empty()).then_some(serde_json::Value::Object(fields))
}

/// 序号列是 `INTEGER`：负数与溢出按 0 处理。
fn count_of(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

#[cfg(test)]
#[path = "projection_read_fallback_tests.rs"]
mod tests;
