//! Feishu 远控平台 adapter。
//!
//! 本 crate 承接 Feishu OpenAPI、WebSocket、delivery、gateway 与 worker 细节；
//! `axon-agent-runtime` 只保留 transport-agnostic 的远控合同。
pub mod adapter;

pub use adapter::{FeishuEventAdapter, FeishuOutboundEvent, interaction_from_gateway_report};
pub use axon_remote_runtime::*;
pub use axon_remote_runtime::{
    REMOTE_NOTIFICATION_ADAPTER_DISPATCH_REASON_ACCEPTED, RemoteNotificationAdapterDispatchItem,
    RemoteNotificationAdapterDispatchRuntime, RemoteNotificationAdapterEnvelope,
    RemoteNotificationAdapterEnvelopeReport, RemoteNotificationAdapterEnvelopeRuntime,
    RemoteNotificationAdapterEnvelopeSurface, RemoteNotificationAdapterSendInput,
    RemoteNotificationAdapterSendOutcome, RemoteNotificationAdapterSendReport,
    RemoteNotificationAdapterSendRuntime, RemoteNotificationAdapterSendStatus,
    RemoteNotificationCard, RemoteNotificationCardTone,
};
pub use remote_notification_adapter_registry::{
    RemoteNotificationAdapterDescriptor, RemoteNotificationAdapterKind,
    RemoteNotificationAdapterRegistryRuntime, RemoteNotificationRenderArtifact,
};
pub use remote_notification_adapter_render::RemoteNotificationFeishuRendererRuntime;

#[path = "remote_notification_adapter_registry.rs"]
mod remote_notification_adapter_registry;
#[path = "remote_notification_adapter_render.rs"]
mod remote_notification_adapter_render;

#[path = "card/remote_feishu_question_card.rs"]
pub mod remote_feishu_question_card;
pub use remote_feishu_question_card::{
    MAX_FEISHU_CARD_ACTION_VALUE_CHARS, MAX_FEISHU_CARD_CONTENT_CHARS,
    MAX_FEISHU_CARD_PAYLOAD_BYTES, MAX_FEISHU_QUESTION_CARD_SHARDS,
    build_settled_feishu_question_card, plan_feishu_question_cards,
};

#[path = "core/remote_feishu.rs"]
mod remote_feishu;
#[path = "reply/remote_feishu_agent_reply.rs"]
mod remote_feishu_agent_reply;
#[path = "reply/remote_feishu_agent_reply_delivery.rs"]
mod remote_feishu_agent_reply_delivery;
#[cfg(test)]
#[path = "delivery/remote_feishu_agent_reply_delivery_tests.rs"]
mod remote_feishu_agent_reply_delivery_tests;
#[path = "reply/remote_feishu_agent_reply_local_media.rs"]
mod remote_feishu_agent_reply_local_media;
#[path = "sdk/remote_feishu_card_action_payload.rs"]
mod remote_feishu_card_action_payload;
#[path = "delivery/remote_feishu_delivery.rs"]
mod remote_feishu_delivery;
#[path = "delivery/remote_feishu_delivery_http.rs"]
mod remote_feishu_delivery_http;
#[cfg(test)]
#[path = "delivery/remote_feishu_delivery_test_support.rs"]
mod remote_feishu_delivery_test_support;
#[cfg(test)]
#[path = "delivery/remote_feishu_delivery_tests.rs"]
mod remote_feishu_delivery_tests;
#[path = "delivery/remote_feishu_delivery_types.rs"]
mod remote_feishu_delivery_types;
#[path = "core/remote_feishu_executor.rs"]
mod remote_feishu_executor;
#[path = "feedback/remote_feishu_feedback_executor.rs"]
mod remote_feishu_feedback_executor;
#[path = "gateway/remote_feishu_gateway.rs"]
mod remote_feishu_gateway;
#[path = "gateway/remote_feishu_gateway_event_source.rs"]
mod remote_feishu_gateway_event_source;
#[path = "gateway/remote_feishu_gateway_sdk_loop.rs"]
mod remote_feishu_gateway_sdk_loop;
#[path = "sdk/remote_feishu_gateway_sdk_marker_content.rs"]
mod remote_feishu_gateway_sdk_marker_content;
#[path = "sdk/remote_feishu_gateway_sdk_payload.rs"]
mod remote_feishu_gateway_sdk_payload;
#[cfg(test)]
#[path = "sdk/remote_feishu_gateway_sdk_payload_card_action_tests.rs"]
mod remote_feishu_gateway_sdk_payload_card_action_tests;
#[cfg(test)]
#[path = "sdk/remote_feishu_gateway_sdk_payload_tests.rs"]
mod remote_feishu_gateway_sdk_payload_tests;
#[path = "sdk/remote_feishu_gateway_sdk_rich_content.rs"]
mod remote_feishu_gateway_sdk_rich_content;
#[path = "sdk/remote_feishu_gateway_sdk_text.rs"]
mod remote_feishu_gateway_sdk_text;
#[path = "gateway/remote_feishu_gateway_task_store.rs"]
mod remote_feishu_gateway_task_store;
#[cfg(test)]
#[path = "gateway/remote_feishu_gateway_tests.rs"]
mod remote_feishu_gateway_tests;
#[path = "governance/remote_feishu_governance.rs"]
mod remote_feishu_governance;
#[cfg(test)]
#[path = "governance/remote_feishu_governance_tests.rs"]
mod remote_feishu_governance_tests;
#[path = "identity/remote_feishu_identity.rs"]
mod remote_feishu_identity;
#[path = "media/remote_feishu_media_cache_budget.rs"]
mod remote_feishu_media_cache_budget;
#[cfg(test)]
#[path = "delivery/remote_feishu_media_delivery_tests.rs"]
mod remote_feishu_media_delivery_tests;
#[path = "media/remote_feishu_media_flow.rs"]
mod remote_feishu_media_flow;
#[path = "media/remote_feishu_media_transfer.rs"]
mod remote_feishu_media_transfer;
#[path = "media/remote_feishu_media_types.rs"]
mod remote_feishu_media_types;
#[path = "identity/remote_feishu_message_lookup.rs"]
mod remote_feishu_message_lookup;
#[path = "oapi/remote_feishu_oapi_executor.rs"]
mod remote_feishu_oapi_executor;
#[path = "oapi/remote_feishu_oapi_ws_frame_codec.rs"]
mod remote_feishu_oapi_ws_frame_codec;
#[path = "oapi/remote_feishu_oapi_ws_loop.rs"]
mod remote_feishu_oapi_ws_loop;
#[path = "oapi/remote_feishu_oapi_ws_ping.rs"]
mod remote_feishu_oapi_ws_ping;
#[path = "oapi/remote_feishu_oapi_ws_reconnect.rs"]
mod remote_feishu_oapi_ws_reconnect;
#[path = "oapi/remote_feishu_oapi_ws_sdk.rs"]
mod remote_feishu_oapi_ws_sdk;
#[path = "core/remote_feishu_outbound.rs"]
mod remote_feishu_outbound;
#[path = "processing/remote_feishu_processing.rs"]
mod remote_feishu_processing;
#[path = "progress/remote_feishu_progress.rs"]
mod remote_feishu_progress;
#[path = "core/remote_feishu_rate_limit.rs"]
mod remote_feishu_rate_limit;
#[path = "runtime/remote_feishu_runtime.rs"]
mod remote_feishu_runtime;
#[path = "runtime/remote_feishu_runtime_maintenance.rs"]
mod remote_feishu_runtime_maintenance;
#[path = "core/remote_feishu_scheduler.rs"]
mod remote_feishu_scheduler;
#[path = "websocket/remote_feishu_websocket_adapter.rs"]
mod remote_feishu_websocket_adapter;
#[path = "websocket/remote_feishu_websocket_ingress.rs"]
mod remote_feishu_websocket_ingress;
#[path = "websocket/remote_feishu_websocket_session.rs"]
mod remote_feishu_websocket_session;

pub use remote_feishu::*;
pub use remote_feishu_gateway_task_store::*;
pub use remote_feishu_progress::RemoteFeishuProgressClient;
#[path = "progress/remote_feishu_semantic_renderer.rs"]
mod remote_feishu_semantic_renderer;
pub use remote_feishu_semantic_renderer::RemoteFeishuSemanticRenderer;
