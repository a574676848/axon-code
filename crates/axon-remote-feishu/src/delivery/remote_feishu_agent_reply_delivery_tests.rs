use crate::RemoteNotificationAdapterSendStatus;
use crate::remote_feishu::{
    REMOTE_FEISHU_AGENT_REPLY_MESSAGE_MAX_CHARS, RemoteFeishuAgentReplyArtifactInput,
    RemoteFeishuAgentReplyArtifactRuntime, RemoteFeishuDeliveryRuntime,
    RemoteFeishuHttpRequestPlan, RemoteFeishuHttpResponse, RemoteFeishuMessageTarget,
};
use crate::remote_feishu_agent_reply_delivery::split_agent_reply_body;
use crate::remote_feishu_delivery_test_support::{accepted_record, dispatch_plan};

#[test]
fn feishu_delivery_agent_reply_plan_sends_full_markdown_to_reply_target() {
    let body = "这是一段超过通知摘要限制但需要完整发送给飞书用户的 Agent 回复。".repeat(8);
    let artifact = RemoteFeishuAgentReplyArtifactRuntime::record_from_input(
        RemoteFeishuAgentReplyArtifactInput {
            dispatch_id: "dispatch-1".to_owned(),
            run_session_id: "session-1".to_owned(),
            processing_message_id: None,
            target: RemoteFeishuMessageTarget {
                receive_id: "oc_chat_1".to_owned(),
                receive_id_type: "chat_id".to_owned(),
                reply_to_message_id: Some("om_parent".to_owned()),
            },
            thread_id: Some("root_1".to_owned()),
            body: body.clone(),
            recorded_at_ms: 900,
        },
    )
    .expect("artifact");
    let plans = RemoteFeishuDeliveryRuntime::plan_agent_reply_requests(
        &dispatch_plan(),
        None,
        &accepted_record(),
        &artifact,
    )
    .expect("plan");

    assert_eq!(plans.len(), 1);
    let plan = &plans[0];
    assert!(
        plan.message_request
            .url
            .ends_with("/messages/om_parent/reply")
    );
    assert!(
        plan.message_request
            .body_json
            .contains("\"msg_type\":\"post\"")
    );
    assert!(
        plan.message_request
            .body_json
            .contains("\\\"tag\\\":\\\"md\\\"")
    );
    assert!(plan.message_request.body_json.contains(&body));
    assert!(plan.fallback_message_request.is_some());
}

#[test]
fn feishu_delivery_agent_reply_plan_splits_long_text_without_loss() {
    let body = "段".repeat(REMOTE_FEISHU_AGENT_REPLY_MESSAGE_MAX_CHARS * 2 + 17);
    let artifact = RemoteFeishuAgentReplyArtifactRuntime::record_from_input(
        RemoteFeishuAgentReplyArtifactInput {
            dispatch_id: "dispatch-long".to_owned(),
            run_session_id: "session-long".to_owned(),
            processing_message_id: None,
            target: RemoteFeishuMessageTarget {
                receive_id: "oc_chat_1".to_owned(),
                receive_id_type: "chat_id".to_owned(),
                reply_to_message_id: Some("om_parent".to_owned()),
            },
            thread_id: Some("root_1".to_owned()),
            body: body.clone(),
            recorded_at_ms: 900,
        },
    )
    .expect("artifact");
    let segments = split_agent_reply_body(&artifact.body).expect("segments");
    let plans = RemoteFeishuDeliveryRuntime::plan_agent_reply_requests(
        &dispatch_plan(),
        None,
        &accepted_record(),
        &artifact,
    )
    .expect("plans");

    assert_eq!(segments.len(), 3);
    assert_eq!(segments.concat(), body);
    assert!(
        segments
            .iter()
            .all(|segment| segment.chars().count() <= REMOTE_FEISHU_AGENT_REPLY_MESSAGE_MAX_CHARS)
    );
    assert_eq!(plans.len(), 3);
    assert!(plans[0].message_request.url.ends_with("/om_parent/reply"));
    assert!(
        plans[1]
            .message_request
            .url
            .contains("receive_id_type=chat_id")
    );
    assert_ne!(
        plans[0].message_request.dispatch_id,
        plans[1].message_request.dispatch_id
    );
}

#[test]
fn feishu_delivery_agent_reply_keeps_split_fenced_code_valid() {
    let code = "let value = 1;\n".repeat(REMOTE_FEISHU_AGENT_REPLY_MESSAGE_MAX_CHARS);
    let body = format!("```rust\n{code}```\n");
    let segments = split_agent_reply_body(&body).expect("segments");

    assert!(segments.len() > 1);
    assert!(
        segments
            .iter()
            .all(|segment| segment.chars().count() <= REMOTE_FEISHU_AGENT_REPLY_MESSAGE_MAX_CHARS)
    );
    assert!(
        segments
            .iter()
            .all(|segment| segment.starts_with("```rust\n") && segment.ends_with("```\n"))
    );
    let restored = segments
        .iter()
        .map(|segment| {
            segment
                .strip_prefix("```rust\n")
                .and_then(|value| value.strip_suffix("```\n"))
                .expect("segment fence")
        })
        .collect::<String>();
    assert_eq!(restored, code);
}

#[test]
fn feishu_delivery_execute_agent_reply_uses_reply_api() {
    let artifact = RemoteFeishuAgentReplyArtifactRuntime::record_from_input(
        RemoteFeishuAgentReplyArtifactInput {
            dispatch_id: "dispatch-1".to_owned(),
            run_session_id: "session-1".to_owned(),
            processing_message_id: None,
            target: RemoteFeishuMessageTarget {
                receive_id: "oc_chat_1".to_owned(),
                receive_id_type: "chat_id".to_owned(),
                reply_to_message_id: Some("om_parent".to_owned()),
            },
            thread_id: None,
            body: "完整回复".to_owned(),
            recorded_at_ms: 900,
        },
    )
    .expect("artifact");
    let mut requests = Vec::new();
    let report = RemoteFeishuDeliveryRuntime::execute_agent_reply_with_http_and_secret(
        &dispatch_plan(),
        "secret-value",
        Some("https://open.feishu.cn"),
        &accepted_record(),
        &artifact,
        901,
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
                    body: r#"{"code":0,"data":{"message_id":"om_reply"}}"#.to_owned(),
                })
            }
        },
    )
    .expect("execute");

    assert_eq!(requests.len(), 2);
    assert!(requests[1].0.ends_with("/messages/om_parent/reply"));
    assert!(requests[1].1.contains("完整回复"));
    assert_eq!(report.adapter_message_id.as_deref(), Some("om_reply"));
}

#[test]
fn feishu_delivery_execute_agent_reply_sends_all_segments_in_order() {
    let body = "答".repeat(REMOTE_FEISHU_AGENT_REPLY_MESSAGE_MAX_CHARS + 1);
    let artifact = RemoteFeishuAgentReplyArtifactRuntime::record_from_input(
        RemoteFeishuAgentReplyArtifactInput {
            dispatch_id: "dispatch-long".to_owned(),
            run_session_id: "session-long".to_owned(),
            processing_message_id: None,
            target: RemoteFeishuMessageTarget {
                receive_id: "oc_chat_1".to_owned(),
                receive_id_type: "chat_id".to_owned(),
                reply_to_message_id: Some("om_parent".to_owned()),
            },
            thread_id: None,
            body,
            recorded_at_ms: 900,
        },
    )
    .expect("artifact");
    let mut message_requests = Vec::new();
    let report = RemoteFeishuDeliveryRuntime::execute_agent_reply_with_http_and_secret(
        &dispatch_plan(),
        "secret-value",
        Some("https://open.feishu.cn"),
        &accepted_record(),
        &artifact,
        901,
        |request| {
            if request.url.contains("tenant_access_token") {
                return Ok(RemoteFeishuHttpResponse {
                    status: 200,
                    body: r#"{"code":0,"tenant_access_token":"tenant-token"}"#.to_owned(),
                });
            }
            message_requests.push((request.url.clone(), request.body_json.clone()));
            Ok(RemoteFeishuHttpResponse {
                status: 200,
                body: format!(
                    r#"{{"code":0,"data":{{"message_id":"om_reply_{}"}}}}"#,
                    message_requests.len()
                ),
            })
        },
    )
    .expect("execute");

    assert_eq!(message_requests.len(), 2);
    assert!(message_requests[0].0.ends_with("/messages/om_parent/reply"));
    assert!(message_requests[1].0.contains("receive_id_type=chat_id"));
    let first_uuid = serde_json::from_str::<serde_json::Value>(&message_requests[0].1)
        .expect("first body")["uuid"]
        .as_str()
        .expect("first uuid")
        .to_owned();
    let second_uuid = serde_json::from_str::<serde_json::Value>(&message_requests[1].1)
        .expect("second body")["uuid"]
        .as_str()
        .expect("second uuid")
        .to_owned();
    assert_ne!(first_uuid, second_uuid);
    assert_eq!(report.adapter_message_id.as_deref(), Some("om_reply_2"));
}

#[test]
fn feishu_delivery_agent_reply_stops_after_failed_segment() {
    let body = "答".repeat(REMOTE_FEISHU_AGENT_REPLY_MESSAGE_MAX_CHARS * 2 + 1);
    let artifact = RemoteFeishuAgentReplyArtifactRuntime::record_from_input(
        RemoteFeishuAgentReplyArtifactInput {
            dispatch_id: "dispatch-long-failure".to_owned(),
            run_session_id: "session-long-failure".to_owned(),
            processing_message_id: None,
            target: RemoteFeishuMessageTarget {
                receive_id: "oc_chat_1".to_owned(),
                receive_id_type: "chat_id".to_owned(),
                reply_to_message_id: Some("om_parent".to_owned()),
            },
            thread_id: None,
            body,
            recorded_at_ms: 900,
        },
    )
    .expect("artifact");
    let mut message_calls = 0;
    let report = RemoteFeishuDeliveryRuntime::execute_agent_reply_with_http_and_secret(
        &dispatch_plan(),
        "secret-value",
        Some("https://open.feishu.cn"),
        &accepted_record(),
        &artifact,
        901,
        |request| {
            if request.url.contains("tenant_access_token") {
                return Ok(RemoteFeishuHttpResponse {
                    status: 200,
                    body: r#"{"code":0,"tenant_access_token":"tenant-token"}"#.to_owned(),
                });
            }
            message_calls += 1;
            if message_calls == 1 {
                Ok(RemoteFeishuHttpResponse {
                    status: 200,
                    body: r#"{"code":0,"data":{"message_id":"om_reply_1"}}"#.to_owned(),
                })
            } else {
                Ok(RemoteFeishuHttpResponse {
                    status: 500,
                    body: r#"{"code":999,"msg":"temporary failure"}"#.to_owned(),
                })
            }
        },
    )
    .expect("execute");

    assert_eq!(message_calls, 2);
    assert_eq!(
        report.send_status,
        RemoteNotificationAdapterSendStatus::RetryableFailure
    );
}

#[test]
fn feishu_delivery_reply_failure_falls_back_to_create_message() {
    let mut requests = Vec::new();
    let response = crate::remote_feishu_delivery_http::execute_message_request_with_fallback(
        &RemoteFeishuHttpRequestPlan {
            dispatch_id: "dispatch-1".to_owned(),
            url: "https://open.feishu.cn/open-apis/im/v1/messages/om_parent/reply".to_owned(),
            method: "POST".to_owned(),
            headers: vec![],
            body_json: "{}".to_owned(),
        },
        Some(&RemoteFeishuHttpRequestPlan {
            dispatch_id: "dispatch-1".to_owned(),
            url: "https://open.feishu.cn/open-apis/im/v1/messages?receive_id_type=chat_id"
                .to_owned(),
            method: "POST".to_owned(),
            headers: vec![],
            body_json: "{}".to_owned(),
        }),
        &mut |request| {
            requests.push(request.url.clone());
            if request.url.ends_with("/reply") {
                Ok(RemoteFeishuHttpResponse {
                    status: 200,
                    body: r#"{"code":230011,"msg":"message withdrawn"}"#.to_owned(),
                })
            } else {
                Ok(RemoteFeishuHttpResponse {
                    status: 200,
                    body: r#"{"code":0,"data":{"message_id":"om_fallback"}}"#.to_owned(),
                })
            }
        },
    )
    .expect("response");

    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1],
        "https://open.feishu.cn/open-apis/im/v1/messages?receive_id_type=chat_id"
    );
    assert!(response.body.contains("om_fallback"));
}
