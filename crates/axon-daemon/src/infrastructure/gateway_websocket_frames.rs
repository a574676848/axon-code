//! WebSocket 线协议帧构造：事件信封、错误帧与投影失败的兜底回执。

use super::*;

/// 带订阅标识的连接级事件信封：socket 循环据此把帧写回正确的订阅。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TaggedEvent {
    pub(super) subscribe_id: String,
    pub(super) event: GatewayEvent,
}

#[derive(Serialize)]
pub(super) struct EventMessage<'a> {
    #[serde(rename = "type")]
    message_type: &'static str,
    #[serde(rename = "subscribeId")]
    subscribe_id: &'a str,
    event_type: &'a str,
    /// 事件载荷：直接内嵌成 JSON 对象，而不是装进字符串再转义一层。
    ///
    /// 载荷本身就是 JSON，装成字符串会让引号与换行各转义一次，帧体量涨到原来的
    /// 一倍半以上（真机实测 44 KB 事件撑成 76 KB 帧），客户端还要多解析一次。
    payload: serde_json::Value,
    cursor: &'a Option<String>,
    replay: bool,
}

pub(super) fn event_json(tagged: &TaggedEvent) -> String {
    // 静默丢帧是明确的违约：投影失败时改为显式错误帧回执。
    let payload =
        serde_json::from_str(&tagged.event.payload_json).unwrap_or(serde_json::Value::Null);
    serde_json::to_string(&EventMessage {
        message_type: "event",
        subscribe_id: &tagged.subscribe_id,
        event_type: &tagged.event.event_type,
        payload,
        cursor: &tagged.event.cursor,
        replay: tagged.event.replay,
    })
    .unwrap_or_else(|_| EVENT_PROJECTION_FAILED_FRAME.to_owned())
}

pub(super) fn error_json(code: &str) -> String {
    serde_json::json!({ "type": "error", "error": code }).to_string()
}

/// 订阅级错误帧：带回 `subscribeId`，让客户端能定位是哪一路订阅被拒。
pub(super) fn subscription_error_json(code: &str, subscribe_id: Option<&str>) -> String {
    let mut payload = serde_json::json!({ "type": "error", "error": code });
    if let Some(subscribe_id) = subscribe_id {
        payload["subscribeId"] = serde_json::Value::String(subscribe_id.to_owned());
    }
    payload.to_string()
}

pub(super) fn error_response(status: u16, code: &str) -> ErrorResponse {
    Response::builder()
        .status(status)
        .body(Some(error_json(code)))
        .unwrap_or_else(|_| HttpResponse::new(None))
}
