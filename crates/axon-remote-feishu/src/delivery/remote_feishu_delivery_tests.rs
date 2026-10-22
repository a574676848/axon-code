use axon_domain::AxonError;

use crate::remote_feishu::{
    REMOTE_FEISHU_API_BASE_URL, REMOTE_FEISHU_TENANT_ACCESS_TOKEN_PATH,
    RemoteFeishuDeliveryRuntime, RemoteFeishuHttpResponse, RemoteFeishuMessageTarget,
};
use crate::remote_feishu_delivery_test_support::{accepted_record, dispatch_plan};
use crate::{
    RemoteNotificationAdapterSendOutcome, RemoteNotificationAdapterSendStatus,
    RemoteNotificationDispatchStatus,
};

#[test]
fn feishu_delivery_plan_builds_token_and_message_requests() {
    let plan =
        RemoteFeishuDeliveryRuntime::plan_requests(&dispatch_plan(), None, &accepted_record())
            .expect("plan requests");

    assert_eq!(
        plan.token_request.url,
        format!("{REMOTE_FEISHU_API_BASE_URL}{REMOTE_FEISHU_TENANT_ACCESS_TOKEN_PATH}")
    );
    assert!(
        plan.token_request
            .body_json
            .contains("\"app_secret\":\"__APP_SECRET__\"")
    );
    assert!(plan.message_request.url.contains("receive_id_type=chat_id"));
    assert!(
        plan.message_request
            .body_json
            .contains("\"msg_type\":\"interactive\"")
    );
}

#[test]
fn feishu_delivery_feedback_maps_delivered_outcome() {
    let report = RemoteFeishuDeliveryRuntime::plan_feedback(
        accepted_record(),
        RemoteNotificationAdapterSendOutcome {
            dispatch_id: "dispatch-1".to_owned(),
            status: RemoteNotificationAdapterSendStatus::Delivered,
            sent_at_ms: 300,
            next_retry_after_ms: None,
            adapter_message_id: Some("om_xxx".to_owned()),
            error_summary: None,
            source_trace: "feishu_delivery:test".to_owned(),
        },
    )
    .expect("plan feedback");

    assert_eq!(report.delivered_count, 1);
    assert_eq!(
        report.records[0].status,
        RemoteNotificationDispatchStatus::Delivered
    );
    assert_eq!(
        report.records[0].adapter_message_id.as_deref(),
        Some("om_xxx")
    );
}

#[test]
fn feishu_delivery_execute_with_http_calls_token_then_message() {
    let mut requests = Vec::new();
    let report = RemoteFeishuDeliveryRuntime::execute_with_http_and_secret(
        &dispatch_plan(),
        "secret-value",
        Some("https://open.feishu.cn"),
        &accepted_record(),
        500,
        |request| {
            requests.push((request.url.clone(), request.body_json.clone()));
            if request.url.contains("tenant_access_token") {
                Ok(RemoteFeishuHttpResponse {
                    status: 200,
                    body: r#"{"code":0,"tenant_access_token":"tenant-token"}"#.to_owned(),
                })
            } else {
                Ok(RemoteFeishuHttpResponse {
                    status: 200,
                    body: r#"{"code":0,"data":{"message_id":"om_xxx"}}"#.to_owned(),
                })
            }
        },
    )
    .expect("execute with http");

    assert_eq!(requests.len(), 2);
    assert!(requests[0].0.contains("tenant_access_token"));
    assert!(requests[1].0.contains("/im/v1/messages"));
    assert!(requests[1].1.contains("\"msg_type\":\"interactive\""));
    assert_eq!(report.adapter_message_id.as_deref(), Some("om_xxx"));
    assert_eq!(
        report.feedback.records[0].status,
        RemoteNotificationDispatchStatus::Delivered
    );
}

#[test]
fn feishu_delivery_execute_retries_retryable_http_failures() {
    let mut attempts = 0_u8;
    let report = RemoteFeishuDeliveryRuntime::execute_with_http_and_secret(
        &dispatch_plan(),
        "secret-value",
        Some("https://open.feishu.cn"),
        &accepted_record(),
        500,
        |request| {
            attempts = attempts.saturating_add(1);
            if request.url.contains("tenant_access_token") && attempts < 3 {
                return Err(AxonError::runtime_failure("temporary network failure"));
            }
            if request.url.contains("tenant_access_token") {
                Ok(RemoteFeishuHttpResponse {
                    status: 200,
                    body: r#"{"code":0,"tenant_access_token":"tenant-token"}"#.to_owned(),
                })
            } else {
                Ok(RemoteFeishuHttpResponse {
                    status: 200,
                    body: r#"{"code":0,"data":{"message_id":"om_retry"}}"#.to_owned(),
                })
            }
        },
    )
    .expect("execute with retry");

    assert_eq!(attempts, 4);
    assert_eq!(report.adapter_message_id.as_deref(), Some("om_retry"));
    assert_eq!(
        report.send_status,
        RemoteNotificationAdapterSendStatus::Delivered
    );
}

#[test]
fn feishu_delivery_plan_supports_reply_with_create_fallback() {
    let target = RemoteFeishuMessageTarget {
        receive_id: "oc_chat_1".to_owned(),
        receive_id_type: "chat_id".to_owned(),
        reply_to_message_id: Some("om_parent".to_owned()),
    };
    let plan = RemoteFeishuDeliveryRuntime::plan_requests_for_target(
        &dispatch_plan(),
        None,
        &accepted_record(),
        &target,
    )
    .expect("plan");

    assert!(
        plan.message_request
            .url
            .ends_with("/messages/om_parent/reply")
    );
    assert!(plan.fallback_message_request.is_some());
    assert!(
        plan.fallback_message_request
            .as_ref()
            .expect("fallback")
            .url
            .contains("receive_id_type=chat_id")
    );
}
