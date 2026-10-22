//! Feishu 远控 adapter 聚合边界。
//!
//! 通用远控模块只保留协议、治理和本机 runtime 语义；Feishu 的 OpenAPI、
//! WebSocket、delivery、processing 和 worker 细节都应从这个 adapter 边界进入。

pub use crate::remote_feishu_agent_reply::{
    REMOTE_FEISHU_AGENT_REPLY_EMPTY_FALLBACK, REMOTE_FEISHU_AGENT_REPLY_FILE,
    RemoteFeishuAgentReplyArtifactInput, RemoteFeishuAgentReplyArtifactRecord,
    RemoteFeishuAgentReplyArtifactRuntime,
};
pub use crate::remote_feishu_agent_reply_delivery::REMOTE_FEISHU_AGENT_REPLY_MESSAGE_MAX_CHARS;
pub use crate::remote_feishu_agent_reply_local_media::RemoteFeishuAgentReplyLocalMediaHttpExecutionInput;
pub use crate::remote_feishu_delivery::RemoteFeishuDeliveryMediaExecutionInput;
pub use crate::remote_feishu_delivery::RemoteFeishuDeliveryMediaHttpExecutionInput;
pub use crate::remote_feishu_delivery::RemoteFeishuDeliveryRuntime;
pub use crate::remote_feishu_delivery_types::{
    REMOTE_FEISHU_API_BASE_URL, REMOTE_FEISHU_DELIVERY_REASON_FEEDBACK_PLANNED,
    REMOTE_FEISHU_DELIVERY_REASON_REQUEST_PLANNED, REMOTE_FEISHU_MESSAGE_CREATE_PATH,
    REMOTE_FEISHU_TENANT_ACCESS_TOKEN_PATH, RemoteFeishuDeliveryExecutionReport,
    RemoteFeishuDeliveryPlan, RemoteFeishuHttpRequestPlan, RemoteFeishuHttpResponse,
    RemoteFeishuMessageTarget,
};
pub use crate::remote_feishu_executor::{
    RemoteFeishuExecutorRuntime, RemoteFeishuExecutorTickInput, RemoteFeishuExecutorTickReport,
    process_processing_state,
};
pub use crate::remote_feishu_feedback_executor::{
    REMOTE_FEISHU_FEEDBACK_ALL_SESSIONS, REMOTE_FEISHU_FEEDBACK_BACKGROUND_INTERVAL_MS,
    REMOTE_FEISHU_FEEDBACK_MAX_ATTEMPTS, REMOTE_FEISHU_FEEDBACK_RETRY_DELAY_MS,
    REMOTE_FEISHU_FEEDBACK_TASK_FILE, RemoteFeishuFeedbackBackgroundInput,
    RemoteFeishuFeedbackBackgroundWorker, RemoteFeishuFeedbackExecutionReport,
    RemoteFeishuFeedbackExecutorRuntime, RemoteFeishuFeedbackExecutorTickInput,
    RemoteFeishuFeedbackExecutorTickReport, RemoteFeishuFeedbackTaskRecord,
    RemoteFeishuFeedbackTaskStatus, RemoteFeishuFeedbackTaskStoreRuntime,
    RemoteFeishuProcessingCleanupTaskInput,
};
pub use crate::remote_feishu_gateway::{
    REMOTE_FEISHU_GATEWAY_DEFAULT_PENDING_DEPTH, REMOTE_FEISHU_GATEWAY_REASON_DEFERRED_BY_CHAT,
    REMOTE_FEISHU_GATEWAY_REASON_DRAINED, REMOTE_FEISHU_GATEWAY_REASON_ENQUEUED,
    REMOTE_FEISHU_GATEWAY_REASON_SHUTTING_DOWN, RemoteFeishuGatewayConnectionState,
    RemoteFeishuGatewayDrainReport, RemoteFeishuGatewayGovernanceEventReport,
    RemoteFeishuGatewayGovernanceInput, RemoteFeishuGatewayGovernanceReport,
    RemoteFeishuGatewayInboundEvent, RemoteFeishuGatewayInboundMeta,
    RemoteFeishuGatewayQueuedEvent, RemoteFeishuGatewayRuntime, RemoteFeishuGatewayTickInput,
    RemoteFeishuGatewayTickReport, RemoteFeishuGatewayTurnCompleteReport,
};
pub use crate::remote_feishu_gateway_event_source::{
    REMOTE_FEISHU_GATEWAY_EVENT_SOURCE_FILE, REMOTE_FEISHU_GATEWAY_EVENT_SOURCE_MAX_BYTES,
    REMOTE_FEISHU_GATEWAY_EVENT_SOURCE_MAX_READ_EVENTS,
    REMOTE_FEISHU_GATEWAY_EVENT_SOURCE_MAX_RECORD_BYTES, RemoteFeishuGatewayEventSourceAppendInput,
    RemoteFeishuGatewayEventSourceAppendReport, RemoteFeishuGatewayEventSourceWriterRuntime,
    RemoteFeishuGatewayRawReactionAppendInput,
};
pub use crate::remote_feishu_gateway_sdk_loop::{
    REMOTE_FEISHU_GATEWAY_SDK_LOOP_REASON_EVENT_SOURCE_APPENDED,
    REMOTE_FEISHU_GATEWAY_SDK_LOOP_REASON_NORMALIZED, RemoteFeishuGatewaySdkLoopAdapterRuntime,
    RemoteFeishuGatewaySdkLoopAppendInput, RemoteFeishuGatewaySdkLoopAppendReport,
    RemoteFeishuGatewaySdkLoopCardActionEvent, RemoteFeishuGatewaySdkLoopEvent,
    RemoteFeishuGatewaySdkLoopMediaDownloadContext, RemoteFeishuGatewaySdkLoopMessageEvent,
    RemoteFeishuGatewaySdkLoopRunnerInput, RemoteFeishuGatewaySdkLoopRunnerReport,
};
pub use crate::remote_feishu_gateway_sdk_payload::{
    REMOTE_FEISHU_GATEWAY_SDK_PAYLOAD_REASON_CARD_ACTION_MAPPED,
    REMOTE_FEISHU_GATEWAY_SDK_PAYLOAD_REASON_MESSAGE_MAPPED,
    RemoteFeishuGatewaySdkPayloadMapperRuntime,
};
pub use crate::remote_feishu_gateway_sdk_text::RemoteFeishuGatewaySdkPayloadTextContext;
pub use crate::remote_feishu_governance::{
    REMOTE_FEISHU_GOVERNANCE_FILE, REMOTE_FEISHU_GOVERNANCE_REASON_ACCEPTED,
    REMOTE_FEISHU_GOVERNANCE_REASON_ALLOWLIST_REJECTED,
    REMOTE_FEISHU_GOVERNANCE_REASON_BOT_NOT_MENTIONED, REMOTE_FEISHU_GOVERNANCE_REASON_DUPLICATE,
    REMOTE_FEISHU_GOVERNANCE_REASON_MENTION_REQUIRED,
    REMOTE_FEISHU_GOVERNANCE_REASON_SELF_EVENT_DROPPED,
    REMOTE_FEISHU_GOVERNANCE_REASON_SELF_IDS_UNKNOWN, RemoteFeishuChatGovernancePolicy,
    RemoteFeishuGovernanceConfig, RemoteFeishuGovernanceDecision,
    RemoteFeishuGovernanceDecisionStatus, RemoteFeishuGovernanceRecord,
    RemoteFeishuGovernanceRuntime, RemoteFeishuGovernanceStoreRuntime, RemoteFeishuThreadState,
};
pub use crate::remote_feishu_identity::{
    REMOTE_FEISHU_BOT_INFO_PATH, RemoteFeishuBotIdentity, RemoteFeishuBotInfoRequestPlan,
    RemoteFeishuIdentityRuntime,
};
pub use crate::remote_feishu_media_flow::{
    RemoteFeishuInboundMediaDownloadInput, RemoteFeishuInboundMediaDownloadReport,
    RemoteFeishuMediaFlowRuntime, RemoteFeishuMediaUploadExecutionInput,
    RemoteFeishuMediaUploadExecutionReport, RemoteFeishuMultipartHttpRequest,
    RemoteFeishuOutboundMediaMessageInput, RemoteFeishuOutboundMediaMessageReport,
};
pub use crate::remote_feishu_media_transfer::{
    REMOTE_FEISHU_FILE_DOWNLOAD_PATH, REMOTE_FEISHU_FILE_MAX_BYTES, REMOTE_FEISHU_FILE_UPLOAD_PATH,
    REMOTE_FEISHU_FILE_UPLOAD_TYPE_STREAM, REMOTE_FEISHU_IMAGE_DOWNLOAD_PATH,
    REMOTE_FEISHU_IMAGE_MAX_BYTES, REMOTE_FEISHU_IMAGE_UPLOAD_PATH,
    REMOTE_FEISHU_IMAGE_UPLOAD_TYPE_MESSAGE, REMOTE_FEISHU_MESSAGE_RESOURCE_DOWNLOAD_PATH,
    RemoteFeishuBinaryHttpResponse, RemoteFeishuMediaDownloadExecutionInput,
    RemoteFeishuMediaDownloadExecutionReport, RemoteFeishuMediaDownloadInput,
    RemoteFeishuMediaMessageResourceDownloadInput, RemoteFeishuMediaTransferRuntime,
    RemoteFeishuMediaUploadInput, RemoteFeishuMediaUploadPlan,
};
pub use crate::remote_feishu_media_types::{RemoteFeishuMediaKind, RemoteFeishuMediaRef};
pub use crate::remote_feishu_message_lookup::{
    RemoteFeishuMessageLookupInfo, RemoteFeishuMessageLookupRuntime,
    RemoteFeishuReplyLookupFailure, RemoteFeishuReplyLookupFailureStage,
    RemoteFeishuReplyLookupOutcome,
};
pub use crate::remote_feishu_oapi_executor::RemoteFeishuOapiExecutorRuntime;
pub use crate::remote_feishu_oapi_ws_loop::{
    RemoteFeishuOapiWsClientConfigState, RemoteFeishuOapiWsLoopControl,
    RemoteFeishuOapiWsLoopReport,
};
pub use crate::remote_feishu_oapi_ws_sdk::{
    REMOTE_FEISHU_OAPI_DEFAULT_DOMAIN, REMOTE_FEISHU_OAPI_WS_DEVICE_ID_QUERY,
    REMOTE_FEISHU_OAPI_WS_ENDPOINT_PATH, REMOTE_FEISHU_OAPI_WS_SERVICE_ID_QUERY,
    RemoteFeishuOapiWsBootstrapRequest, RemoteFeishuOapiWsClientConfig, RemoteFeishuOapiWsEndpoint,
    RemoteFeishuOapiWsEndpointResponse, RemoteFeishuOapiWsFrame,
    RemoteFeishuOapiWsFrameHandleReport, RemoteFeishuOapiWsHeader,
    RemoteFeishuOapiWsIncomingPayload, RemoteFeishuOapiWsReconnectPolicy,
    RemoteFeishuOapiWsSdkConfig, RemoteFeishuOapiWsSdkRuntime,
    decode_frame as decode_feishu_oapi_ws_frame, encode_frame as encode_feishu_oapi_ws_frame,
};
pub use crate::remote_feishu_outbound::{
    REMOTE_FEISHU_OUTBOUND_MSG_TYPE_AUDIO, REMOTE_FEISHU_OUTBOUND_MSG_TYPE_FILE,
    REMOTE_FEISHU_OUTBOUND_MSG_TYPE_IMAGE, REMOTE_FEISHU_OUTBOUND_MSG_TYPE_INTERACTIVE,
    REMOTE_FEISHU_OUTBOUND_MSG_TYPE_MEDIA, REMOTE_FEISHU_OUTBOUND_MSG_TYPE_POST,
    REMOTE_FEISHU_OUTBOUND_MSG_TYPE_TEXT, RemoteFeishuOutboundMessagePlan,
    RemoteFeishuOutboundMessageRuntime,
};
pub use crate::remote_feishu_processing::{
    REMOTE_FEISHU_PROCESSING_REACTION_FAILURE, REMOTE_FEISHU_PROCESSING_REACTION_IN_PROGRESS,
    REMOTE_FEISHU_PROCESSING_STATE_FILE, RemoteFeishuProcessingAction,
    RemoteFeishuProcessingActionKind, RemoteFeishuProcessingCompleteHttpInput,
    RemoteFeishuProcessingOutcome, RemoteFeishuProcessingPlan, RemoteFeishuProcessingRuntime,
    RemoteFeishuProcessingStateRecord, RemoteFeishuProcessingStateStoreRuntime,
};
pub use crate::remote_feishu_rate_limit::{
    REMOTE_FEISHU_RATE_LIMIT_MAX_EVENTS, REMOTE_FEISHU_RATE_LIMIT_MAX_KEYS,
    REMOTE_FEISHU_RATE_LIMIT_WINDOW_MS, RemoteFeishuRateLimitDecision, RemoteFeishuRateLimiter,
};
pub use crate::remote_feishu_runtime::remote_feishu_runtime_root;
pub use crate::remote_feishu_runtime_maintenance::{
    REMOTE_FEISHU_MAINTENANCE_INTERVAL_MS, RemoteFeishuRuntimeMaintenanceReport,
    RemoteFeishuRuntimeMaintenanceRuntime,
};
pub use crate::remote_feishu_scheduler::{
    REMOTE_FEISHU_SCHEDULER_STATE_FILE, RemoteFeishuSchedulerRuntime,
    RemoteFeishuSchedulerStateRecord, RemoteFeishuSchedulerStoreRuntime,
    RemoteFeishuSerialSchedule,
};
pub use crate::remote_feishu_websocket_adapter::{
    REMOTE_FEISHU_WEBSOCKET_DEFAULT_URL, REMOTE_FEISHU_WEBSOCKET_REASON_DEFAULT_URL,
    REMOTE_FEISHU_WEBSOCKET_REASON_DISPATCH_PLANNED,
    REMOTE_FEISHU_WEBSOCKET_REASON_NETWORK_DISABLED, RemoteFeishuWebSocketAdapterConfig,
    RemoteFeishuWebSocketAdapterRuntime, RemoteFeishuWebSocketDispatchPlan,
};
pub use crate::remote_feishu_websocket_ingress::{
    REMOTE_FEISHU_CARD_ACTION_TAG_MAX_CHARS, REMOTE_FEISHU_CARD_ACTION_VALUE_MAX_CHARS,
    REMOTE_FEISHU_CHAT_TYPE_GROUP, REMOTE_FEISHU_CHAT_TYPE_P2P, REMOTE_FEISHU_CHAT_TYPE_UNKNOWN,
    REMOTE_FEISHU_INCOMING_MESSAGE_KIND_COMMAND, REMOTE_FEISHU_INCOMING_MESSAGE_KIND_TEXT,
    REMOTE_FEISHU_WEBSOCKET_INGRESS_REASON_CARD_ACTION_NORMALIZED,
    REMOTE_FEISHU_WEBSOCKET_INGRESS_REASON_MESSAGE_NORMALIZED, RemoteFeishuCardActionEvent,
    RemoteFeishuChatType, RemoteFeishuIncomingMessage, RemoteFeishuIncomingMessageKind,
    RemoteFeishuReplyContext, RemoteFeishuWebSocketIngressRuntime,
};
pub use crate::remote_feishu_websocket_session::RemoteFeishuWebSocketSessionRuntime;
