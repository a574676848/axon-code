//! Axon 跨 crate 协议类型。

pub mod daemon;
pub mod diff;
/// 主 turn Goal 工具协议。
pub mod goal;
/// Interaction 持久化入口协议。
pub mod interaction;
pub mod wire;

pub use daemon::{
    DAEMON_PROTOCOL_VERSION, DaemonCommand, DaemonEvent, DaemonRequest, DaemonResponse,
};
pub use goal::{GoalSource, GoalStartCommand, GoalStartRequest, GoalToolCommand, GoalToolRequest};
pub use interaction::{
    EnqueueDeliveryReceipt, EnqueueDeliveryRequest, SubmitInteractionReceipt,
    SubmitInteractionRequest,
};
