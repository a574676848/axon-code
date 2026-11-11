//! 实时订阅的通知词汇：durable 事件、瞬态帧与状态跃迁。
//!
//! 与 `event_stream_bridge` 的订阅注册表分开成文件，是为了让「通知长什么样」与
//! 「怎么分发给订阅者」各自独立演进：新增一种帧类型只动这里，不动分发逻辑。

use axon_domain::{
    ConsumerCursor, DaemonSessionId, InteractionId, InteractionStatus, RuntimeObservationFrame,
};
use axon_session::SessionEventRecord;

/// 一个已提交事件及其 durable cursor。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamEvent {
    /// 事件日志记录。
    pub event: SessionEventRecord,
    /// 事件提交后的 cursor。
    pub cursor: ConsumerCursor,
}

/// 一条不进 durable 日志的实时帧。
///
/// 与 [`StreamEvent`] 的差别只有两点：没有 durable cursor，也不推进订阅者的重放水位。
/// 客户端把它当作「这一轮正在进行中」的瞬时投影；断线重连后由边界快照补齐，
/// 因此它可以在慢消费者上被丢弃而不影响最终一致。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveStreamEvent {
    /// 事件所属会话。
    pub session_id: DaemonSessionId,
    /// 事件所属 Interaction；会话级订阅与按 Interaction 订阅都按它过滤。
    pub interaction_id: InteractionId,
    /// wire 事件类型：与 durable 事件同名，客户端沿用同一套投影。
    pub event_type: String,
    /// 已带 interactionId 标注的载荷。
    pub payload_json: String,
}

/// 一个实时订阅通知。
#[derive(Debug, Clone, PartialEq)]
pub enum StreamNotification {
    /// 已提交的 Session 事件。
    SessionEvent(StreamEvent),
    /// 瞬态实时帧：立即投递、零落盘、不推进 durable cursor。
    LiveEvent(LiveStreamEvent),
    /// 实时流被裁剪，客户端应携带 cursor 重连并按 durable replay 重建。
    StreamReset {
        /// 客户端最近确认的 durable cursor。
        cursor: Option<ConsumerCursor>,
        /// 裁剪原因。
        reason: &'static str,
    },
    /// Interaction 状态已完成 durable 提交。
    InteractionStatus(InteractionStatus),
    /// 运行时非持久观测帧（零落盘、可丢弃、不推进 durable cursor）。
    Observation(RuntimeObservationFrame),
}
