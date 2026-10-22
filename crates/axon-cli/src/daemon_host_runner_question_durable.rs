//! 问答事件在「没有活跃 run」时的 durable 落点。
//!
//! 问答帧平时由当轮 run 的事件记录器写进 `session_events`。迟到作答发生在回合之外，
//! 那时车道上没有任何 run 可以承接这一帧，若就地丢弃，历史回放里就永远没有「已结算」
//! 这一步：任何按事件流重建界面的客户端都会把已经答完的提问一直摆成待答，并给出注定
//! 被服务端拒绝的回入口。
//!
//! 这里补的正是那条落盘路径：以宿主自己的 host 记录过代际围栏追加到**车道会话**的
//! 事件流（与当轮 run 写的是同一条流、同一个事件类型），再把落好的事件推给实时订阅者。
//! 顺序与子代理会话的 sink 一致：先 durable，再实时。

use std::sync::Arc;

use axon_domain::DaemonSessionId;
use axon_protocol::wire::WireEventEnvelope;
use axon_session::{DaemonHostRecord, RuntimeStore, SqliteRuntimeStore};

use crate::run_commands::DurableQuestionEventWriter;

/// 会话事件类型；与当轮 run 写入的类型一致，回放不必为它另开一条解析路径。
const WIRE_EVENT_TYPE: &str = "wire_event";

/// 造一个 durable 落点：把问答帧写进车道会话的事件流并推给订阅者。
///
/// 落点是纯函数式闭包：它只依赖构造时给定的存储、宿主身份与订阅桥，因此可以在
/// 守护进程的任何线程上调用。
pub(super) fn durable_question_event_writer(
    store: SqliteRuntimeStore,
    host: DaemonHostRecord,
    bridge: Arc<axon_daemon::EventStreamBridge>,
) -> DurableQuestionEventWriter {
    Arc::new(move |lane_id: &str, event: &WireEventEnvelope| {
        let session_id =
            DaemonSessionId::new(lane_id).map_err(|error| format!("车道标识非法：{error:?}"))?;
        let now_ms = crate::daemon_commands::now_ms();
        let payload =
            serde_json::to_string(event).map_err(|error| format!("问答事件序列化失败：{error}"))?;
        let records = store
            .append_events(
                &session_id,
                &host,
                &[(WIRE_EVENT_TYPE.to_owned(), payload)],
                now_ms,
            )
            .map_err(|error| format!("问答事件追加失败：{error:?}"))?;
        // 落库之后再发布：订阅者看到的每一帧都已经是 durable 事实。
        for record in &records {
            bridge.publish(record, &record.cursor());
        }
        Ok(())
    })
}
