//! 轮次工具名累积与终态帧标注。
//!
//! 工具调用帧是 wire 帧（`kind.type == "toolCall"`），名字在 `kind.payload.toolName`。
//! 逐轮事实投影需要这份清单，过去靠回扫本轮全部事件帧凑，写路径因此多一次解压读。
//! 现在由事件 sink 在投递每一帧时顺手累积，轮次收口时把清单挂到终态帧上，投影直接读。

use std::sync::Mutex;

use axon_session_runtime::RuntimeEvent;

/// 记录本帧里的工具名；重复名字只留第一次，顺序即首次出现顺序。
///
/// 只认 `kind.type == "toolCall"` 的 wire 帧：正文增量与终态帧不参与。
pub(super) fn observe(tool_names: &Mutex<Vec<String>>, event: &RuntimeEvent) {
    let Ok(payload) = serde_json::from_str::<serde_json::Value>(&event.payload_json) else {
        return;
    };
    let Some(kind) = payload.get("kind") else {
        return;
    };
    if kind.get("type").and_then(serde_json::Value::as_str) != Some("toolCall") {
        return;
    }
    let Some(name) = kind
        .get("payload")
        .and_then(|payload| payload.get("toolName"))
        .and_then(serde_json::Value::as_str)
        .filter(|name| !name.is_empty())
    else {
        return;
    };
    let mut names = tool_names
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !names.iter().any(|known| known == name) {
        names.push(name.to_owned());
    }
}

/// 本轮已累积工具名的快照；失败终态与执行器都用它补载荷。
pub(super) fn snapshot(tool_names: &Mutex<Vec<String>>) -> Vec<String> {
    tool_names
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// 终态帧带上本轮工具名，供逐轮事实投影直接读取，不必回扫事件日志。
///
/// 载荷仍是 JSON 对象，只多一个 `toolNames` 数组；旧读者忽略未知字段，新读者拿它
/// 直接写投影。本轮没有工具调用时原样返回，不额外标注。
pub(super) fn terminal_frame(
    tool_names: &Mutex<Vec<String>>,
    event: &RuntimeEvent,
) -> Result<RuntimeEvent, String> {
    let names = tool_names
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    if names.is_empty() {
        return Ok(event.clone());
    }
    let mut payload: serde_json::Value = serde_json::from_str(&event.payload_json)
        .map_err(|error| format!("解析终态载荷失败：{error}"))?;
    let serde_json::Value::Object(map) = &mut payload else {
        return Ok(event.clone());
    };
    map.insert(
        "toolNames".to_owned(),
        serde_json::Value::Array(names.into_iter().map(serde_json::Value::String).collect()),
    );
    let mut annotated = event.clone();
    annotated.payload_json =
        serde_json::to_string(&payload).map_err(|error| format!("序列化终态载荷失败：{error}"))?;
    Ok(annotated)
}
