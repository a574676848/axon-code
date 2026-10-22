use crate::remote_feishu::{
    RemoteFeishuAgentReplyArtifactInput, RemoteFeishuAgentReplyArtifactRuntime,
    RemoteFeishuAgentReplyLocalMediaHttpExecutionInput, RemoteFeishuDeliveryMediaExecutionInput,
    RemoteFeishuDeliveryMediaHttpExecutionInput, RemoteFeishuDeliveryRuntime,
    RemoteFeishuHttpResponse, RemoteFeishuMediaKind, RemoteFeishuMessageTarget,
};
use crate::remote_feishu_delivery_test_support::{accepted_record, dispatch_plan};

#[test]
fn feishu_delivery_media_uploads_then_sends_media_message() {
    let file_path = axon_test_support::test_temp_root().join(format!(
        "axon-feishu-delivery-media-{}.png",
        unique_suffix()
    ));
    std::fs::write(&file_path, b"image-bytes").expect("fixture");
    let mut json_requests = Vec::new();
    let mut multipart_called = false;
    let report = RemoteFeishuDeliveryRuntime::execute_media_with_http_and_secret(
        RemoteFeishuDeliveryMediaHttpExecutionInput {
            dispatch: &dispatch_plan(),
            app_secret: "secret-value",
            api_base_url: Some("https://open.feishu.cn"),
            accepted_record: &accepted_record(),
            media: RemoteFeishuDeliveryMediaExecutionInput {
                kind: RemoteFeishuMediaKind::Image,
                file_path: file_path.clone(),
                target: None,
            },
            sent_at_ms: 700,
        },
        |request| {
            json_requests.push((request.url.clone(), request.body_json.clone()));
            if request.url.contains("tenant_access_token") {
                Ok(RemoteFeishuHttpResponse {
                    status: 200,
                    body: r#"{"code":0,"tenant_access_token":"tenant-token"}"#.to_owned(),
                })
            } else {
                Ok(RemoteFeishuHttpResponse {
                    status: 200,
                    body: r#"{"code":0,"data":{"message_id":"om_media"}}"#.to_owned(),
                })
            }
        },
        |request| {
            multipart_called = true;
            assert!(request.url.contains("/open-apis/im/v1/images"));
            assert!(
                request
                    .body
                    .windows(b"image-bytes".len())
                    .any(|chunk| chunk == b"image-bytes")
            );
            Ok(RemoteFeishuHttpResponse {
                status: 200,
                body: r#"{"code":0,"data":{"image_key":"img_uploaded"}}"#.to_owned(),
            })
        },
    )
    .expect("media delivery");

    assert!(multipart_called);
    assert_eq!(json_requests.len(), 2);
    assert!(json_requests[1].1.contains("\"msg_type\":\"image\""));
    assert!(json_requests[1].1.contains("img_uploaded"));
    assert_eq!(report.adapter_message_id.as_deref(), Some("om_media"));
    let _ = std::fs::remove_file(file_path);
}

#[test]
fn feishu_delivery_agent_reply_local_file_uploads_media_to_reply_target() {
    let file_path = axon_test_support::test_temp_root().join(format!(
        "axon-feishu-agent-reply-media-{}.png",
        unique_suffix()
    ));
    std::fs::write(&file_path, b"reply-image-bytes").expect("fixture");
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
            body: format!("已生成图片：\nMEDIA:{}", file_path.display()),
            recorded_at_ms: 900,
        },
    )
    .expect("artifact");
    let mut json_requests = Vec::new();
    let mut multipart_called = false;
    let report = RemoteFeishuDeliveryRuntime::execute_agent_reply_local_media_with_http_and_secret(
        RemoteFeishuAgentReplyLocalMediaHttpExecutionInput {
            dispatch: &dispatch_plan(),
            app_secret: "secret-value",
            api_base_url: Some("https://open.feishu.cn"),
            accepted_record: &accepted_record(),
            artifact: &artifact,
            sent_at_ms: 901,
        },
        |request| {
            json_requests.push((request.url.clone(), request.body_json.clone()));
            if request.url.contains("tenant_access_token") {
                Ok(RemoteFeishuHttpResponse {
                    status: 200,
                    body: r#"{"code":0,"tenant_access_token":"tenant-token"}"#.to_owned(),
                })
            } else {
                Ok(RemoteFeishuHttpResponse {
                    status: 200,
                    body: r#"{"code":0,"data":{"message_id":"om_media_reply"}}"#.to_owned(),
                })
            }
        },
        |request| {
            multipart_called = true;
            assert!(request.url.contains("/open-apis/im/v1/images"));
            assert!(
                request
                    .body
                    .windows(b"reply-image-bytes".len())
                    .any(|chunk| chunk == b"reply-image-bytes")
            );
            Ok(RemoteFeishuHttpResponse {
                status: 200,
                body: r#"{"code":0,"data":{"image_key":"img_reply_uploaded"}}"#.to_owned(),
            })
        },
    )
    .expect("media delivery");

    assert!(multipart_called);
    assert_eq!(json_requests.len(), 2);
    assert!(json_requests[1].0.ends_with("/messages/om_parent/reply"));
    assert!(json_requests[1].1.contains("\"msg_type\":\"image\""));
    assert!(json_requests[1].1.contains("img_reply_uploaded"));
    assert_eq!(report.adapter_message_id.as_deref(), Some("om_media_reply"));
    let _ = std::fs::remove_file(file_path);
}

#[test]
fn feishu_delivery_agent_reply_uploads_all_local_files_in_order() {
    let first_path = axon_test_support::test_temp_root().join(format!(
        "axon-feishu-agent-reply-media-a-{}.png",
        unique_suffix()
    ));
    let second_path = axon_test_support::test_temp_root().join(format!(
        "axon-feishu-agent-reply-media-b-{}.txt",
        unique_suffix()
    ));
    std::fs::write(&first_path, b"reply-image-a").expect("first fixture");
    std::fs::write(&second_path, b"reply-file-b").expect("second fixture");
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
            body: format!(
                "已生成多个文件：\nMEDIA:{}\nMEDIA:{}",
                first_path.display(),
                second_path.display()
            ),
            recorded_at_ms: 900,
        },
    )
    .expect("artifact");
    let mut json_message_requests = Vec::new();
    let mut multipart_urls = Vec::new();
    let report =
        RemoteFeishuDeliveryRuntime::execute_agent_reply_local_media_all_with_http_and_secret(
            RemoteFeishuAgentReplyLocalMediaHttpExecutionInput {
                dispatch: &dispatch_plan(),
                app_secret: "secret-value",
                api_base_url: Some("https://open.feishu.cn"),
                accepted_record: &accepted_record(),
                artifact: &artifact,
                sent_at_ms: 901,
            },
            |request| {
                if request.url.contains("tenant_access_token") {
                    Ok(RemoteFeishuHttpResponse {
                        status: 200,
                        body: r#"{"code":0,"tenant_access_token":"tenant-token"}"#.to_owned(),
                    })
                } else {
                    json_message_requests.push((request.url.clone(), request.body_json.clone()));
                    Ok(RemoteFeishuHttpResponse {
                        status: 200,
                        body: format!(
                            r#"{{"code":0,"data":{{"message_id":"om_media_reply_{}"}}}}"#,
                            json_message_requests.len()
                        ),
                    })
                }
            },
            |request| {
                multipart_urls.push(request.url.clone());
                let body = if request.url.contains("/images") {
                    r#"{"code":0,"data":{"image_key":"img_reply_uploaded"}}"#
                } else {
                    r#"{"code":0,"data":{"file_key":"file_reply_uploaded"}}"#
                };
                Ok(RemoteFeishuHttpResponse {
                    status: 200,
                    body: body.to_owned(),
                })
            },
        )
        .expect("media delivery");

    assert_eq!(multipart_urls.len(), 2);
    assert!(multipart_urls[0].contains("/open-apis/im/v1/images"));
    assert!(multipart_urls[1].contains("/open-apis/im/v1/files"));
    assert_eq!(json_message_requests.len(), 2);
    assert!(
        json_message_requests[0]
            .1
            .contains("\"msg_type\":\"image\"")
    );
    assert!(json_message_requests[1].1.contains("\"msg_type\":\"file\""));
    assert_eq!(report.feedback.records.len(), 2);
    assert_eq!(
        report.adapter_message_id.as_deref(),
        Some("om_media_reply_2")
    );
    let _ = std::fs::remove_file(first_path);
    let _ = std::fs::remove_file(second_path);
}

#[test]
fn feishu_agent_reply_delivery_sends_visible_text_and_attachment_together() {
    let file_path = axon_test_support::test_temp_root().join(format!(
        "axon-feishu-agent-reply-text-and-media-{}.png",
        unique_suffix()
    ));
    std::fs::write(&file_path, b"reply-image-bytes").expect("fixture");
    let artifact = RemoteFeishuAgentReplyArtifactRuntime::record_from_input(
        RemoteFeishuAgentReplyArtifactInput {
            dispatch_id: "dispatch-1".to_owned(),
            run_session_id: "session-1".to_owned(),
            processing_message_id: None,
            target: RemoteFeishuMessageTarget {
                receive_id: "oc_chat_1".to_owned(),
                receive_id_type: "chat_id".to_owned(),
                reply_to_message_id: None,
            },
            thread_id: None,
            body: format!("已生成图片：\nMEDIA:{}", file_path.display()),
            recorded_at_ms: 900,
        },
    )
    .expect("artifact");
    let mut json_requests = Vec::new();
    let mut multipart_urls = Vec::new();
    let report = RemoteFeishuDeliveryRuntime::execute_agent_reply_delivery_with_http_and_secret(
        RemoteFeishuAgentReplyLocalMediaHttpExecutionInput {
            dispatch: &dispatch_plan(),
            app_secret: "secret-value",
            api_base_url: Some("https://open.feishu.cn"),
            accepted_record: &accepted_record(),
            artifact: &artifact,
            sent_at_ms: 901,
        },
        |request| {
            if request.url.contains("tenant_access_token") {
                return Ok(RemoteFeishuHttpResponse {
                    status: 200,
                    body: r#"{"code":0,"tenant_access_token":"tenant-token"}"#.to_owned(),
                });
            }
            json_requests.push((request.url.clone(), request.body_json.clone()));
            Ok(RemoteFeishuHttpResponse {
                status: 200,
                body: format!(
                    r#"{{"code":0,"data":{{"message_id":"om_reply_{}"}}}}"#,
                    json_requests.len()
                ),
            })
        },
        |request| {
            multipart_urls.push(request.url.clone());
            Ok(RemoteFeishuHttpResponse {
                status: 200,
                body: r#"{"code":0,"data":{"image_key":"img_reply_uploaded"}}"#.to_owned(),
            })
        },
    )
    .expect("agent reply delivery");

    // 命中附件时正文不得被吞掉：先投可见正文，再投附件。
    assert_eq!(json_requests.len(), 2);
    assert!(json_requests[0].1.contains("已生成图片"));
    assert!(!json_requests[0].1.contains("MEDIA:"));
    assert!(json_requests[1].1.contains("\"msg_type\":\"image\""));
    assert_eq!(multipart_urls.len(), 1);
    assert_eq!(report.feedback.records.len(), 2);
    let _ = std::fs::remove_file(file_path);
}

#[test]
fn feishu_agent_reply_delivery_falls_back_to_visible_text_when_path_is_missing() {
    let artifact = RemoteFeishuAgentReplyArtifactRuntime::record_from_input(
        RemoteFeishuAgentReplyArtifactInput {
            dispatch_id: "dispatch-1".to_owned(),
            run_session_id: "session-1".to_owned(),
            processing_message_id: None,
            target: RemoteFeishuMessageTarget {
                receive_id: "oc_chat_1".to_owned(),
                receive_id_type: "chat_id".to_owned(),
                reply_to_message_id: None,
            },
            thread_id: None,
            body: "说明与结论\nMEDIA:C:\\missing\\report.png".to_owned(),
            recorded_at_ms: 900,
        },
    )
    .expect("artifact");
    let mut json_requests = Vec::new();
    let mut multipart_called = false;
    let report = RemoteFeishuDeliveryRuntime::execute_agent_reply_delivery_with_http_and_secret(
        RemoteFeishuAgentReplyLocalMediaHttpExecutionInput {
            dispatch: &dispatch_plan(),
            app_secret: "secret-value",
            api_base_url: Some("https://open.feishu.cn"),
            accepted_record: &accepted_record(),
            artifact: &artifact,
            sent_at_ms: 901,
        },
        |request| {
            if request.url.contains("tenant_access_token") {
                return Ok(RemoteFeishuHttpResponse {
                    status: 200,
                    body: r#"{"code":0,"tenant_access_token":"tenant-token"}"#.to_owned(),
                });
            }
            json_requests.push((request.url.clone(), request.body_json.clone()));
            Ok(RemoteFeishuHttpResponse {
                status: 200,
                body: r#"{"code":0,"data":{"message_id":"om_text_only"}}"#.to_owned(),
            })
        },
        |_request| {
            multipart_called = true;
            Ok(RemoteFeishuHttpResponse {
                status: 200,
                body: r#"{"code":0,"data":{"image_key":"unused"}}"#.to_owned(),
            })
        },
    )
    .expect("agent reply delivery");

    // 路径非法时不得整条回复失败：退化为纯正文投递，且不泄漏 MEDIA 指令。
    assert!(!multipart_called);
    assert_eq!(json_requests.len(), 1);
    assert!(json_requests[0].1.contains("说明与结论"));
    assert!(!json_requests[0].1.contains("MEDIA:"));
    assert_eq!(report.feedback.records.len(), 1);
}

fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time")
        .as_nanos()
}
