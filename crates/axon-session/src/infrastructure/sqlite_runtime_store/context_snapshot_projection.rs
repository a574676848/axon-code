//! 会话级上下文快照的投影：从本批追加的帧里取占用读数。
//!
//! 写入点只有一处：事件追加的唯一投影入口 `apply_projections` 调用本模块。逐帧实时投递
//! 的用量读数在这里被归并成一条会话级事实，界面不再各自累加最后一条帧。
//!
//! 两条取数规则：
//!
//! 1. 只认真正带读数的那一帧。流式帧常常只报增量、不带 `currentContextTokens`；这类帧
//!    不得覆盖已有快照，也不得刷新时间戳（否则界面会显示「刚刚更新」，而数字其实是旧的）。
//! 2. 未知就是未知。换窗不再产生任何 token 读数，因此本模块只处理用量帧：换窗前后的
//!    数字一律不解析、不回落。

use serde_json::Value;

use super::SqliteRuntimeStore;
use super::context_snapshot_repository::{ContextSnapshotUpdate, UsageReading, apply_update};
use crate::ports::RuntimeStoreError;
use axon_domain::DaemonSessionId;

/// 逐轮正文的事件类型。
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

/// 把一批事件里的上下文读数投影成会话级快照。
///
/// 写入失败原样返回错误：水位只在三个投影都成功后推进，失败批次留给重放或下一份真实读数
/// 覆盖修正。写失败留下的旧行不会被声称为最新读数（时间戳不变）。
pub(super) fn project_context_snapshot(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    events: &[(String, String)],
    now_ms: u64,
) -> Result<(), RuntimeStoreError> {
    let Some(update) = read_update(events, now_ms) else {
        return Ok(());
    };
    apply_update(store, session_id, &update)
}

/// 读出一批事件里的读数；这一批没有可用读数时返回 `None`，调用方据此不写任何行。
fn read_update(events: &[(String, String)], now_ms: u64) -> Option<ContextSnapshotUpdate> {
    let mut update = ContextSnapshotUpdate::default();
    for (event_type, payload_json) in events {
        if event_type != WIRE_EVENT_TYPE {
            continue;
        }
        // 字节级预筛：批内绝大多数事件不是用量帧，先做一次子串判断，避免为每条事件
        // 都建一棵完整的 serde_json DOM。
        if !payload_json.contains(USAGE_KIND) {
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
        if kind_type != USAGE_KIND {
            continue;
        }
        let observed_at_ms = frame_time_ms(&envelope, now_ms);
        if let Some(reading) = usage_reading(kind.get("payload"), observed_at_ms) {
            update.usage = Some(reading);
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
