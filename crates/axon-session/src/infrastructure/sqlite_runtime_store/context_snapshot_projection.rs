//! 会话级上下文快照的投影：从本批追加的帧里取占用与换窗读数。
//!
//! 写入点只有一处：事件追加的唯一投影入口 `apply_projections` 调用本模块。逐帧实时投递
//! 的用量读数在这里被归并成一条会话级事实，界面不再各自累加最后一条帧。
//!
//! 两条取数规则：
//!
//! 1. 只认真正带读数的那一帧。流式帧常常只报增量、不带 `currentContextTokens`；这类帧
//!    不得覆盖已有快照，也不得刷新时间戳（否则界面会显示「刚刚更新」，而数字其实是旧的）。
//! 2. 未知就是未知。换窗前后读数优先取帧里的结构化字段；帧里只有展示文本时，只认精确
//!    整数，被压缩成 `825K` 这类单位的一律留空，绝不反推出一个近似值冒充读数。

use serde_json::Value;

use super::SqliteRuntimeStore;
use super::context_snapshot_repository::{
    ContextSnapshotUpdate, RolloverReading, UsageReading, apply_update,
};
use axon_domain::DaemonSessionId;

/// 逐轮正文的事件类型。
const WIRE_EVENT_TYPE: &str = "wire_event";
/// 用量帧的 `kind.type`。
const USAGE_KIND: &str = "usageUpdate";
/// 生命周期帧的 `kind.type`。
const LIFECYCLE_KIND: &str = "turnLifecycle";
/// 换窗开始的类别。
const COMPACTION_STARTED: &str = "CompactionStarted";
/// 换窗完成的类别。
const COMPACTION_COMPLETED: &str = "CompactionCompleted";
/// 换窗失败的类别。
const COMPACTION_FAILED: &str = "CompactionFailed";
/// 生命周期状态里的失败取值。
const STATUS_FAILED: &str = "failed";
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
/// 结构化换窗读数的字段名。
const BEFORE_FIELD: &str = "beforeTokens";
/// 结构化换窗读数的字段名。
const AFTER_FIELD: &str = "afterTokens";
/// 生命周期帧里承载结构化指标的字段。
const METADATA_FIELD: &str = "metadata";
/// 生命周期帧的展示摘要。
const SUMMARY_FIELD: &str = "summary";
/// 摘要里「换窗前读数」的标签，按优先级排列。
const BEFORE_SUMMARY_LABELS: [&str; 3] = ["当前上下文 ", "切换前 ", "tokens "];
/// 摘要里「换窗后读数」的标签，只在换窗读数那一段里找。
const AFTER_SUMMARY_LABELS: [&str; 3] = ["切换后估算 ", "切换后 ", "→ "];

/// 把一批事件里的上下文读数投影成会话级快照。
///
/// 投影失败只记日志、不阻断落盘：与 `session_metrics` 同款。写失败留下的旧行不会被
/// 声称为最新读数（时间戳不变），下一份真实读数会覆盖修正。
pub(super) fn project_context_snapshot(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    events: &[(String, String)],
    now_ms: u64,
) {
    let Some(update) = read_update(events, now_ms) else {
        return;
    };
    if let Err(error) = apply_update(store, session_id, &update) {
        eprintln!(
            "上下文快照投影写入失败，下一份读数会覆盖修正: session={} error={error}",
            session_id.as_str()
        );
    }
}

/// 读出一批事件里的读数；这一批没有可用读数时返回 `None`，调用方据此不写任何行。
fn read_update(events: &[(String, String)], now_ms: u64) -> Option<ContextSnapshotUpdate> {
    let mut update = ContextSnapshotUpdate::default();
    for (event_type, payload_json) in events {
        if event_type != WIRE_EVENT_TYPE {
            continue;
        }
        let Ok(envelope) = serde_json::from_str::<Value>(payload_json) else {
            continue;
        };
        let Some(kind) = envelope.get("kind") else {
            continue;
        };
        let Some(kind_type) = kind.get("type").and_then(Value::as_str) else {
            continue;
        };
        let payload = kind.get("payload");
        let observed_at_ms = frame_time_ms(&envelope, now_ms);
        match kind_type {
            USAGE_KIND => {
                if let Some(reading) = usage_reading(payload, observed_at_ms) {
                    update.usage = Some(reading);
                }
            }
            LIFECYCLE_KIND => {
                if let Some(reading) = rollover_reading(payload, observed_at_ms) {
                    update.rollover = Some(reading);
                }
            }
            _ => {}
        }
    }
    (!update.is_empty()).then_some(update)
}

/// 帧自己的记录时刻；缺失或非法时用本次落盘时刻，绝不编造时刻。
fn frame_time_ms(envelope: &Value, fallback_ms: u64) -> u64 {
    envelope
        .get(RECORDED_AT_FIELD)
        .and_then(Value::as_i64)
        .and_then(|value| u64::try_from(value).ok())
        .unwrap_or(fallback_ms)
}

/// 一条用量帧是否构成可用的上下文读数。
fn usage_reading(payload: Option<&Value>, observed_at_ms: u64) -> Option<UsageReading> {
    let payload = payload?;
    // 不带当前占用的流式帧不是上下文读数：不覆盖、不刷新时间戳。
    let current_context_tokens = positive(payload.get(CURRENT_CONTEXT_FIELD))?;
    Some(UsageReading {
        current_context_tokens,
        context_window: positive(payload.get(CONTEXT_WINDOW_FIELD)),
        composition: composition_of(payload),
        observed_at_ms,
    })
}

/// 上限读数：0 与缺失同义（没有读到），不能当成「上下文占用为 0」。
fn positive(value: Option<&Value>) -> Option<u64> {
    value.and_then(Value::as_u64).filter(|value| *value > 0)
}

/// 上下文构成：只收帧里真实出现的三个字段，全是缺失时留空。
fn composition_of(payload: &Value) -> Option<Value> {
    let mut fields = serde_json::Map::new();
    for field in COMPOSITION_FIELDS {
        if let Some(value) = payload.get(field).and_then(Value::as_u64) {
            fields.insert(field.to_owned(), Value::from(value));
        }
    }
    (!fields.is_empty()).then_some(Value::Object(fields))
}

/// 一条轮转生命周期帧的换窗读数。
fn rollover_reading(payload: Option<&Value>, observed_at_ms: u64) -> Option<RolloverReading> {
    let payload = payload?;
    let lifecycle = payload.get("kind").and_then(Value::as_str)?;
    if !matches!(
        lifecycle,
        COMPACTION_STARTED | COMPACTION_COMPLETED | COMPACTION_FAILED
    ) {
        return None;
    }
    let before_tokens = structured_tokens(payload, BEFORE_FIELD)
        .or_else(|| summary_tokens(payload, TokenSlot::Before));
    // 失败帧没有新窗口：换窗后读数一律未知，不能把帧里的 0 读成「上下文降到 0」。
    let failed = lifecycle == COMPACTION_FAILED
        || payload.get("status").and_then(Value::as_str) == Some(STATUS_FAILED);
    let after_tokens = if failed {
        None
    } else {
        structured_tokens(payload, AFTER_FIELD)
            .or_else(|| summary_tokens(payload, TokenSlot::After))
    };
    Some(RolloverReading {
        before_tokens,
        after_tokens,
        observed_at_ms,
    })
}

/// 帧里的结构化 token 字段：先看 `metadata`（运行时审计事件的形状），再看载荷顶层。
fn structured_tokens(payload: &Value, field: &str) -> Option<u64> {
    let from_metadata = payload
        .get(METADATA_FIELD)
        .and_then(|metadata| metadata.get(field))
        .and_then(Value::as_u64);
    positive_u64(from_metadata).or_else(|| positive_u64(payload.get(field).and_then(Value::as_u64)))
}

fn positive_u64(value: Option<u64>) -> Option<u64> {
    value.filter(|value| *value > 0)
}

/// 摘要文本里的换窗读数；只认精确整数。
fn summary_tokens(payload: &Value, slot: TokenSlot) -> Option<u64> {
    let summary = payload.get(SUMMARY_FIELD).and_then(Value::as_str)?;
    let segment = remainder_after(summary, &BEFORE_SUMMARY_LABELS)?;
    match slot {
        TokenSlot::Before => exact_count(segment),
        TokenSlot::After => remainder_after(segment, &AFTER_SUMMARY_LABELS).and_then(exact_count),
    }
}

/// 摘要里取哪一个读数。
#[derive(Debug, Clone, Copy)]
enum TokenSlot {
    /// 换窗前。
    Before,
    /// 换窗后。
    After,
}

/// 某个标签之后的剩余文本；先命中先返回。
fn remainder_after<'a>(text: &'a str, labels: &[&str]) -> Option<&'a str> {
    labels
        .iter()
        .find_map(|label| text.find(label).map(|index| &text[index + label.len()..]))
}

/// 取紧邻位置的一个精确整数。
///
/// 只接受数字与千位分隔符。紧跟小数点或单位后缀说明这是压缩后的展示值
/// （`31.7K`、`1.2M`、`3万`），一律返回 `None`：宁可显示未知，也不反推近似读数。
fn exact_count(text: &str) -> Option<u64> {
    let run_end = text
        .char_indices()
        .take_while(|(_, ch)| ch.is_ascii_digit() || *ch == ',' || *ch == '_')
        .map(|(index, ch)| index + ch.len_utf8())
        .last()?;
    let (run, rest) = text.split_at(run_end);
    let digits: String = run.chars().filter(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    let compressed = rest.starts_with('.')
        || rest
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '万' || ch == '亿');
    if compressed {
        return None;
    }
    digits.parse::<u64>().ok()
}
