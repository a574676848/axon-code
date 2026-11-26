//! 投影落后时的读路径回源：上下文快照与会话累计指标共用一套落后判定与有界回源。
//!
//! 投影已经异步化：写路径只投递，worker 按批执行，写返回后立刻读投影可能读到上一批甚至
//! 空行。因此读路径上加一道闸门：只有「已投影到的末序号」低于「事件水位」时才回源，判定
//! 一次查询拿到两个数；回源只读不写，结果绝不倒退，读不全一律退回投影值。
//!
//! 快照按日志尾部定向取最近一条可用的用量帧，命中即停。累计指标按水位之后的序号区间
//! 分块定向读，逐轮口径与 `session_metrics_repository` 的累加完全一致：只认
//! `turn_metrics` 事件。已经在 `turn_activity` 里落过读数的轮次不再重复累加，这一条是
//! 必须的：水位记的是末序号，落后窗口恰好等于尚未投影的事件，区间里的指标全部累加即可。
//!
//! 回源前先确认边车索引可用：索引不新鲜时尾部扫描与定向读都会退化成整文件解帧，帧数
//! 上界与毫秒预算都管不到那一段，那种读法不如直接给投影值。
//!
//! 模块由同目录的 `context_snapshot_repository` 声明：两条读路径共用同一套水位、上界与
//! 留痕规则。

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use axon_domain::DaemonSessionId;
use axon_eventlog::{EventLogError, RuntimeEventRecord, TurnMetrics};
use rusqlite::{OptionalExtension, params};

use super::super::SqliteRuntimeStore;
use super::super::session_metrics_repository::{
    MetricsRow, SessionMetricsSnapshot, accumulate_turn_metrics, read_metrics,
};
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
/// 水位与事件尾相距很远（投影长时间停摆、日志被重建）时不再回源：那种情形下正确动作是
/// 重建投影，而不是让一次读背负整段历史。跨度内的定向读按批推进，代价与跨度成正比。
const METRICS_GAP_MAX_EVENTS: u64 = 8_192;

/// 累计指标回源的单批条数：一步只解一批的帧，单批的解压量与内存都有界。
const METRICS_CHUNK_EVENTS: usize = 2_048;

/// 累计指标回源的总耗时预算（毫秒）：还有整批要读时超出即退回投影值。
const METRICS_SCAN_BUDGET_MS: u128 = 50;

/// 累计指标回源保留的轮次指标字节上限（2 MiB）：条数上界管不住载荷大小，先到先停。
const METRICS_BYTES_MAX: usize = 2 * 1024 * 1024;

/// 回源留痕的会话级限流窗口（毫秒）。
const FALLBACK_LOG_WINDOW_MS: u64 = 60_000;

/// 限流状态的最大条目数；超出即整表清空，绝不让它随会话数无界增长。
const FALLBACK_LOG_MAX_ENTRIES: usize = 256;

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

/// 回源留痕的限流表：键是「会话标识加类别」，值是上次留痕时刻。
static FALLBACK_LOG: LazyLock<Mutex<HashMap<(String, &'static str), u64>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// 投影水位：已投影到的末序号与事件水位。
///
/// 批首序号不大于它的事件批次都已被投影过，因此「小于事件水位」只是保守的落后判据：
/// 漏判会让读路径继续给旧值，误判的代价只是一次有界回源。
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
/// 判定只走这一条语句：分两次查会让两个数落在不同的读事务里。会话行不存在时返回 `None`。
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
/// 只取最近一条可用用量帧：它比投影行新才采用，否则原样返回投影值。边车索引不可用时直接
/// 跳过回源：此时尾部扫描会退化成整文件解帧，20ms 预算管不到那一段。
pub(super) fn freshen_snapshot(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    projected: Option<ContextSnapshot>,
    lag: &ProjectionLag,
) -> Option<ContextSnapshot> {
    let started = Instant::now();
    let path = store.event_log(session_id).path();
    if !axon_eventlog::index_is_fresh(&path) {
        report(LogKind::Skipped, session_id, lag, started, "边车索引不可用");
        return projected;
    }
    match latest_usage_reading(&path) {
        Ok(Some(reading)) => {
            report(LogKind::Observed, session_id, lag, started, "命中用量帧");
            Some(with_reading(projected, reading))
        }
        Ok(None) => {
            report(LogKind::Observed, session_id, lag, started, "尾部无用量帧");
            projected
        }
        Err(error) => {
            let reason = error.to_string();
            report(LogKind::Failed, session_id, lag, started, &reason);
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
/// 与投影同口径：只认真正带当前占用的帧，0 与缺失同义。帧自己的记录时刻缺失时用记录的
/// 产生时刻顶替，不编造时刻。
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
/// 累加口径复用 `session_metrics_repository`：回源与投影只有一份实现，不会分叉。
pub(super) fn freshen_metrics(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    projected: Option<SessionMetricsSnapshot>,
    lag: &ProjectionLag,
) -> Option<SessionMetricsSnapshot> {
    let started = Instant::now();
    if lag.pending_events() == 0 {
        return projected;
    }
    let path = store.event_log(session_id).path();
    // 边车索引不可用时定向读会退化成整文件解帧，直接按投影值返回。
    if !axon_eventlog::index_is_fresh(&path) {
        report(LogKind::Skipped, session_id, lag, started, "边车索引不可用");
        return projected;
    }
    match metrics_after_watermark(store, session_id, lag, projected.as_ref()) {
        Ok(snapshot) => {
            report(
                LogKind::Observed,
                session_id,
                lag,
                started,
                "水位区间已读完",
            );
            Some(snapshot)
        }
        Err(reason) => {
            report(LogKind::Failed, session_id, lag, started, &reason);
            projected
        }
    }
}

/// 从投影值出发，把水位之后区间里的指标按同一套累加口径叠加一遍。
fn metrics_after_watermark(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    lag: &ProjectionLag,
    projected: Option<&SessionMetricsSnapshot>,
) -> Result<SessionMetricsSnapshot, String> {
    let pending = lag.pending_events();
    if pending > METRICS_GAP_MAX_EVENTS {
        return Err(format!("跨度 {pending} 超出上界 {METRICS_GAP_MAX_EVENTS}"));
    }
    let base = projected.cloned().unwrap_or_default();
    let mut row = MetricsRow::from_snapshot(&base);
    let limit = usize::try_from(pending).map_err(|_| "跨度无法换算".to_owned())?;
    let path = store.event_log(session_id).path();
    let started = Instant::now();
    let mut cursor = lag.last_projected_seq;
    let mut first: Option<u64> = None;
    let mut total = 0_usize;
    let mut retained = 0_usize;
    while total < limit {
        let remaining = limit - total;
        /*
         * 预算只挡整批的追加工作：收尾那一批不足单批条数，把这一小段读完比丢掉整段回源
         * 更划算，越界幅度也仍以单批为上界。读不全一律退回投影值。
         */
        if remaining >= METRICS_CHUNK_EVENTS
            && started.elapsed().as_millis() >= METRICS_SCAN_BUDGET_MS
        {
            return Err(format!(
                "预算 {METRICS_SCAN_BUDGET_MS}ms 用尽，已读 {total}/{limit}"
            ));
        }
        let want = METRICS_CHUNK_EVENTS.min(remaining);
        let records = axon_eventlog::read_after(&path, cursor, want).map_err(|e| e.to_string())?;
        let exhausted = records.len() < want;
        // 一批读完立即释放整条记录：只留累加需要的字段，不把区间与副本同时留在内存里。
        for record in records {
            first.get_or_insert(record.sequence);
            cursor = record.sequence;
            total = total.saturating_add(1);
            let Some((metrics, channel, emitted_at_ms)) = turn_metrics_of(record) else {
                continue;
            };
            retained += std::mem::size_of_val(&metrics) + metrics.turn_id.len();
            if retained > METRICS_BYTES_MAX {
                return Err(format!("保留字节超 {METRICS_BYTES_MAX}，{total}/{limit}"));
            }
            accumulate_turn_metrics(&mut row, &metrics, channel.as_deref(), emitted_at_ms);
        }
        if exhausted {
            break;
        }
    }
    // 读全才用：首尾序号必须正好卡住水位区间，缺一段就是缺一轮。
    let head = lag.last_projected_seq.saturating_add(1);
    let complete = total == limit && cursor == lag.last_event_sequence && first == Some(head);
    if !complete {
        return Err(format!("水位区间读不全：期望 {limit} 条，实得 {total} 条"));
    }
    Ok(SessionMetricsSnapshot::from_row(&row))
}

/// 一条事件是否是一轮的指标；是则给出指标、渠道与记录时刻。
///
/// 不再按「已计入轮次」去重：水位记的是已投影到的最后一条序号，区间里的事件都还没进
/// 累计值，全部累加才对；去重会在部分批次失败时把该轮永久少算。
fn turn_metrics_of(record: RuntimeEventRecord) -> Option<(TurnMetrics, Option<String>, u64)> {
    if record.event_type.as_str() != TURN_METRICS_EVENT_TYPE {
        return None;
    }
    let emitted_at_ms = record.emitted_at_ms;
    let channel = record.origin_channel;
    let metrics = serde_json::from_value::<TurnMetrics>(record.payload).ok()?;
    Some((metrics, channel, emitted_at_ms))
}

impl SqliteRuntimeStore {
    /// 读取会话累计指标：投影优先，投影落后时按事件日志补齐水位之后的轮次。
    ///
    /// 落后判定与投影读各用一条语句；只有真的落后时才碰事件日志。回源受索引可用性、
    /// 序号跨度、单批条数、保留字节与耗时五重约束，读不全就退回投影值。读路径不回写
    /// 投影表：补齐只对本次调用生效，投影仍由投影 worker 或重建入口推进。
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
#[derive(Debug, Clone, Copy)]
enum LogKind {
    /// 回源发生。
    Observed,
    /// 索引不可用，本次按投影值返回。
    Skipped,
    /// 回源失败或读不全，本次退回投影值。
    Failed,
}

impl LogKind {
    /// 限流键与日志行的短标签。
    const fn tag(self) -> &'static str {
        match self {
            Self::Observed => "完成",
            Self::Skipped => "跳过",
            Self::Failed => "失败",
        }
    }
}

/// 一次回源留痕：同一会话同一类别在窗口内只记一条，并带够定位的水位与耗时。
fn report(
    kind: LogKind,
    session_id: &DaemonSessionId,
    lag: &ProjectionLag,
    started: Instant,
    detail: &str,
) {
    if !allow_log(kind, session_id.as_str()) {
        return;
    }
    let elapsed = started.elapsed().as_millis();
    let line = format!(
        "投影回源{}：session={} projected={} tail={} gap={} 耗时{elapsed}ms {detail}",
        kind.tag(),
        session_id.as_str(),
        lag.last_projected_seq,
        lag.last_event_sequence,
        lag.pending_events()
    );
    match kind {
        LogKind::Failed => axon_logger::warning(&line),
        LogKind::Observed | LogKind::Skipped => axon_logger::debug(&line),
    }
}

/// 限流闸门：同一会话的同一类留痕在窗口内只放行一条。
fn allow_log(kind: LogKind, session_id: &str) -> bool {
    let now = now_ms();
    let key = (session_id.to_owned(), kind.tag());
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
