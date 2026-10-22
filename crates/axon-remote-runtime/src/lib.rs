//! 与 Agent 生命周期无关的远控协议、策略、adapter 合同和运行时合同。
//!
//! 本 crate 不依赖 `axon-agent-runtime`，也不启动监听器、不访问网络。
//! Server、Feishu、Web 和 Mobile adapter 均通过这里的结构化合同接入。

pub use axon_protocol::wire::WireLifecycleStatus;

#[path = "remote/remote_adapter_registry.rs"]
mod remote_adapter_registry;
#[path = "remote/remote_artifact_view.rs"]
mod remote_artifact_view;
#[path = "remote/remote_control.rs"]
mod remote_control;
#[path = "remote/remote_event_cursor.rs"]
mod remote_event_cursor;
#[path = "remote/remote_notification_adapter_card.rs"]
mod remote_notification_adapter_card;
#[path = "remote/remote_notification_adapter_dispatch.rs"]
mod remote_notification_adapter_dispatch;
#[path = "remote/remote_output_ref.rs"]
mod remote_output_ref;
#[path = "remote/remote_projection.rs"]
mod remote_projection;
#[path = "remote/remote_protocol.rs"]
mod remote_protocol;
#[path = "remote/remote_thread_control.rs"]
mod remote_thread_control;
#[path = "remote/remote_transport.rs"]
mod remote_transport;
#[path = "remote/remote_upload_ingress.rs"]
mod remote_upload_ingress;

#[path = "remote/remote_delivery_store.rs"]
mod remote_delivery_store;
#[path = "remote/remote_durable_store.rs"]
mod remote_durable_store;
#[path = "remote/remote_notification_fanout.rs"]
mod remote_notification_fanout;
#[path = "remote/remote_notification_outbox.rs"]
mod remote_notification_outbox;
#[path = "remote/remote_state_connection.rs"]
mod remote_state_connection;
#[path = "remote/remote_state_store.rs"]
mod remote_state_store;
#[path = "remote/remote_subscription_store.rs"]
mod remote_subscription_store;

#[cfg(test)]
#[path = "remote/remote_delivery_soak_tests.rs"]
mod remote_delivery_soak_tests;
#[cfg(test)]
#[path = "remote/remote_state_store_lru_tests.rs"]
mod remote_state_store_lru_tests;

pub use remote_adapter_registry::*;
pub use remote_artifact_view::*;
pub use remote_control::*;
pub use remote_delivery_store::*;
pub use remote_durable_store::*;
pub use remote_event_cursor::*;
pub use remote_notification_adapter_card::*;
pub use remote_notification_adapter_dispatch::envelope::*;
pub use remote_notification_adapter_dispatch::feedback::*;
pub use remote_notification_adapter_dispatch::send::*;
pub use remote_notification_adapter_dispatch::*;
pub use remote_notification_fanout::*;
pub use remote_notification_outbox::*;
pub use remote_output_ref::*;
pub use remote_projection::*;
pub use remote_protocol::*;
pub use remote_state_store::*;
pub use remote_subscription_store::*;
pub use remote_thread_control::*;
pub use remote_transport::*;
pub use remote_upload_ingress::*;
