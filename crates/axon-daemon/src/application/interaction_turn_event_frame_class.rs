//! wire 帧的投递分类。
//!
//! 只做纯函数判定：一帧是正文增量、瞬态读数、可回放的过程读数，还是语义事件。
//! 与 sink 的存储和发布通道解耦，便于单独回归。

use axon_domain::InteractionId;
use serde_json::Value;

/// 累积中的正文 part 类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LivePartKind {
    /// assistant 正文片段。
    Assistant,
    /// 模型推理片段。
    Reasoning,
}

/// 一帧的投递分类。
pub(super) enum FrameClass {
    /// 正文增量：实时投递并累积成边界快照。
    LivePart { key: String, kind: LivePartKind },
    /// 仅实时投递：供应商状态这类瞬态读数，不进快照。
    ///
    /// `remember` 表示同时留一份可补发的副本：这类读数没有 durable 副本，
    /// 订阅建立之前发出的那些本来会永久丢失，界面只能挂一句中性占位。
    LiveOnly { remember: bool },
    /// 实时投递并补一份持久副本：用量是会话事实，切回会话时必须能回放。
    Snapshot,
    /// 必须落库的语义事件。
    Durable,
}

/// 正文 part 的累积键。
pub(super) fn assistant_part_key(part_id: &str) -> String {
    format!("assistant:{part_id}")
}

/// 推理 part 的累积键。
fn reasoning_part_key(part_id: &str) -> String {
    format!("reasoning:{part_id}")
}

/// 按 wire 载荷给一帧分类：哪些只走实时、哪些必须落库。
pub(super) fn classify_frame(payload_json: &str) -> FrameClass {
    let Ok(envelope) = serde_json::from_str::<Value>(payload_json) else {
        return FrameClass::Durable;
    };
    let Some(kind) = envelope.get("kind") else {
        return FrameClass::Durable;
    };
    let kind_type = kind.get("type").and_then(Value::as_str).unwrap_or_default();
    let payload = kind.get("payload");
    let part_id = payload
        .and_then(|payload| payload.get("partId"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    match kind_type {
        "reasoningDelta" => FrameClass::LivePart {
            key: reasoning_part_key(part_id),
            kind: LivePartKind::Reasoning,
        },
        "assistantMessage"
            if payload
                .and_then(|payload| payload.get("delta"))
                .and_then(Value::as_bool)
                == Some(true) =>
        {
            FrameClass::LivePart {
                key: assistant_part_key(part_id),
                kind: LivePartKind::Assistant,
            }
        }
        // 用量是会话事实：实时投递之外必须留一份可回放的副本，否则切回会话时这一轮读数无处可取。
        // 供应商状态是瞬态，仍然只走实时。
        "usageUpdate" => FrameClass::Snapshot,
        "providerStatus" => FrameClass::LiveOnly { remember: true },
        _ => FrameClass::Durable,
    }
}

/// 从正文 part 的信封里取出这一帧的文本。
pub(super) fn live_part_text(envelope: &Value, kind: LivePartKind) -> Option<String> {
    let payload = envelope.get("kind")?.get("payload")?;
    let text = match kind {
        LivePartKind::Assistant => payload.get("content")?,
        LivePartKind::Reasoning => payload.get("delta")?,
    };
    Some(text.as_str()?.to_owned())
}

/// 给实时帧补上 interactionId：回放按它过滤到本交互。
pub(super) fn annotate_live_event(
    payload_json: &str,
    interaction_id: &InteractionId,
) -> Result<String, String> {
    let mut payload: Value =
        serde_json::from_str(payload_json).map_err(|error| error.to_string())?;
    let object = payload
        .as_object_mut()
        .ok_or_else(|| "Runtime 事件 payload 必须是 JSON 对象".to_owned())?;
    object.insert(
        "interactionId".to_owned(),
        Value::String(interaction_id.as_str().to_owned()),
    );
    serde_json::to_string(&payload).map_err(|error| error.to_string())
}
