//! 事件帧载荷的形状统一。
//!
//! 归属的权威是事件的**归属列**，但载荷里也要带同一个标识：主、cron、目标、子代理
//! 四种会话写出来的帧形状因此完全一致，读侧不必按会话类型判断「这一帧属于哪一轮」。
//! 缺这一步时只有子代理的帧是「列里有、载荷里没有」，任何按载荷解析的消费者都会把
//! 它整批丢掉。
//!
//! 单列成模块的理由与同目录其它小模块相同：主仓库文件的职责是写入流程，形状规整
//! 是一段与流程无关的纯函数。

use serde_json::Value;

/// 给载荷补上归属标识；已经带了非空值、或载荷不是对象时原样返回。
pub(super) fn with_interaction(payload: Value, interaction_id: Option<&str>) -> Value {
    let Some(owner) = interaction_id else {
        return payload;
    };
    let Value::Object(object) = payload else {
        return payload;
    };
    let declared = object
        .get("interactionId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !declared.is_empty() {
        return Value::Object(object);
    }
    let mut patched = object;
    patched.insert("interactionId".to_owned(), Value::from(owner.to_owned()));
    Value::Object(patched)
}
