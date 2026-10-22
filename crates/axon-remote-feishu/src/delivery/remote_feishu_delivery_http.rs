//! Feishu delivery HTTP 请求和反馈映射 helper。

use axon_domain::{AxonError, AxonResult, SessionId};
use std::{thread, time::Duration};

use crate::remote_feishu_delivery_types::{
    REMOTE_FEISHU_DELIVERY_REASON_EXECUTED, REMOTE_FEISHU_DELIVERY_REASON_EXECUTION_FAILED,
    REMOTE_FEISHU_DELIVERY_REASON_FEEDBACK_PLANNED, REMOTE_FEISHU_DELIVERY_REASON_REQUEST_PLANNED,
    REMOTE_FEISHU_DELIVERY_SEND_ATTEMPTS, REMOTE_FEISHU_MESSAGE_CREATE_PATH,
    REMOTE_FEISHU_MESSAGE_REPLY_PATH_SUFFIX, REMOTE_FEISHU_REPLY_FALLBACK_CODES,
    RemoteFeishuDeliveryExecutionReport, RemoteFeishuHttpRequestPlan, RemoteFeishuHttpResponse,
    RemoteFeishuMessageTarget,
};
use crate::{
    REMOTE_NOTIFICATION_ADAPTER_DISPATCH_REASON_ACCEPTED, RemoteFeishuDeliveryRuntime,
    RemoteFeishuOutboundMessagePlan, RemoteNotificationAdapterSendOutcome,
    RemoteNotificationAdapterSendStatus, RemoteNotificationDispatchStatus,
    RemoteNotificationOutboxRecord,
};

pub(crate) fn execute_with_retry<F>(
    request: &RemoteFeishuHttpRequestPlan,
    http: &mut F,
) -> AxonResult<RemoteFeishuHttpResponse>
where
    F: FnMut(&RemoteFeishuHttpRequestPlan) -> AxonResult<RemoteFeishuHttpResponse>,
{
    let mut last_error = None;
    for attempt in 0..REMOTE_FEISHU_DELIVERY_SEND_ATTEMPTS {
        match http(request) {
            Ok(response) => return Ok(response),
            Err(error) => {
                last_error = Some(error);
                if attempt + 1 == REMOTE_FEISHU_DELIVERY_SEND_ATTEMPTS {
                    break;
                }
                thread::sleep(Duration::from_millis(
                    100_u64.saturating_mul(1_u64 << attempt),
                ));
            }
        }
    }
    Err(last_error.unwrap_or(AxonError::runtime_failure("Feishu delivery 重试执行失败")))
}

pub(crate) fn execute_message_request_with_fallback<F>(
    request: &RemoteFeishuHttpRequestPlan,
    fallback_request: Option<&RemoteFeishuHttpRequestPlan>,
    http: &mut F,
) -> AxonResult<RemoteFeishuHttpResponse>
where
    F: FnMut(&RemoteFeishuHttpRequestPlan) -> AxonResult<RemoteFeishuHttpResponse>,
{
    let response = execute_with_retry(request, http)?;
    if let Some(fallback_request) = fallback_request
        && should_fallback_from_reply(&response)
    {
        return execute_with_retry(fallback_request, http);
    }
    Ok(response)
}

pub(crate) fn json_headers(token: Option<&str>) -> Vec<(String, String)> {
    let mut headers = vec![("Content-Type".to_owned(), "application/json".to_owned())];
    if let Some(token) = token {
        headers.push(("Authorization".to_owned(), format!("Bearer {token}")));
    }
    headers
}

pub(crate) fn parse_tenant_access_token(response: &RemoteFeishuHttpResponse) -> AxonResult<String> {
    parse_tenant_access_token_with_expiry(response).map(|(token, _)| token)
}

pub(crate) fn parse_tenant_access_token_with_expiry(
    response: &RemoteFeishuHttpResponse,
) -> AxonResult<(String, Option<u64>)> {
    if response.status != 200 {
        return Err(AxonError::runtime_failure(format!(
            "Feishu token API 返回状态码 {}",
            response.status
        )));
    }
    let value = serde_json::from_str::<serde_json::Value>(&response.body)
        .map_err(|_| AxonError::runtime_failure("Feishu token API 响应 JSON 无效"))?;
    if value["code"].as_i64().unwrap_or(0) != 0 {
        return Err(AxonError::runtime_failure(format!(
            "Feishu token API 返回错误：{}",
            value["msg"].as_str().unwrap_or("unknown")
        )));
    }
    let Some(token) = value["tenant_access_token"].as_str() else {
        return Err(AxonError::runtime_failure(
            "Feishu token API 缺少 tenant_access_token",
        ));
    };
    Ok((token.to_owned(), value["expire"].as_u64()))
}

pub(crate) fn media_message_request(
    base: &str,
    accepted_record: &RemoteNotificationOutboxRecord,
    target: &RemoteFeishuMessageTarget,
    message: &RemoteFeishuOutboundMessagePlan,
    tenant_access_token: &str,
) -> RemoteFeishuHttpRequestPlan {
    let mut request = if let Some(reply_to_message_id) = target.reply_to_message_id.as_deref() {
        plan_reply_message_request(
            base,
            accepted_record,
            reply_to_message_id,
            &message.msg_type,
            &message.content_json,
        )
    } else {
        plan_create_message_request(
            base,
            accepted_record,
            target,
            &message.msg_type,
            &message.content_json,
        )
    };
    request.headers = json_headers(Some(tenant_access_token));
    request
}

pub(crate) fn execution_report_from_message_response(
    accepted_record: &RemoteNotificationOutboxRecord,
    sent_at_ms: u64,
    message_response: &RemoteFeishuHttpResponse,
) -> AxonResult<RemoteFeishuDeliveryExecutionReport> {
    let outcome = outcome_from_message_response(
        accepted_record.dispatch_id.clone(),
        sent_at_ms,
        message_response,
    );
    let mut accepted = accepted_record.clone();
    accepted.status = RemoteNotificationDispatchStatus::Attempting;
    REMOTE_NOTIFICATION_ADAPTER_DISPATCH_REASON_ACCEPTED.clone_into(&mut accepted.reason);
    let feedback = RemoteFeishuDeliveryRuntime::plan_feedback(accepted, outcome.clone())?;
    Ok(RemoteFeishuDeliveryExecutionReport {
        session_id: SessionId(accepted_record.session_id),
        dispatch_id: accepted_record.dispatch_id.clone(),
        network_send_attempted: true,
        token_obtained: true,
        adapter_message_id: outcome.adapter_message_id,
        send_status: outcome.status,
        feedback,
        reasons: vec![
            REMOTE_FEISHU_DELIVERY_REASON_REQUEST_PLANNED.to_owned(),
            REMOTE_FEISHU_DELIVERY_REASON_EXECUTED.to_owned(),
            REMOTE_FEISHU_DELIVERY_REASON_FEEDBACK_PLANNED.to_owned(),
        ],
    })
}

pub(crate) fn plan_create_message_request(
    base: &str,
    accepted_record: &RemoteNotificationOutboxRecord,
    target: &RemoteFeishuMessageTarget,
    msg_type: &str,
    content_json: &str,
) -> RemoteFeishuHttpRequestPlan {
    RemoteFeishuHttpRequestPlan {
        dispatch_id: accepted_record.dispatch_id.clone(),
        url: format!(
            "{base}{REMOTE_FEISHU_MESSAGE_CREATE_PATH}?receive_id_type={}",
            target.receive_id_type
        ),
        method: "POST".to_owned(),
        headers: json_headers(Some("__TOKEN__")),
        body_json: serde_json::json!({
            "receive_id": target.receive_id,
            "msg_type": msg_type,
            "content": content_json,
            "uuid": accepted_record.dispatch_id,
        })
        .to_string(),
    }
}

fn outcome_from_message_response(
    dispatch_id: String,
    sent_at_ms: u64,
    response: &RemoteFeishuHttpResponse,
) -> RemoteNotificationAdapterSendOutcome {
    let value = serde_json::from_str::<serde_json::Value>(&response.body).unwrap_or_default();
    let business_code = value["code"].as_i64().unwrap_or_default();
    let status = if response.status == 200 && business_code == 0 {
        RemoteNotificationAdapterSendStatus::Delivered
    } else if response.status == 429 || response.status >= 500 {
        RemoteNotificationAdapterSendStatus::RetryableFailure
    } else {
        RemoteNotificationAdapterSendStatus::PermanentFailure
    };
    let adapter_message_id = value["data"]["message_id"].as_str().map(ToOwned::to_owned);
    let error_summary = if matches!(status, RemoteNotificationAdapterSendStatus::Delivered) {
        None
    } else {
        Some(
            value["msg"]
                .as_str()
                .unwrap_or("Feishu message API failed")
                .to_owned(),
        )
    };
    RemoteNotificationAdapterSendOutcome {
        dispatch_id,
        status,
        sent_at_ms,
        next_retry_after_ms: matches!(
            status,
            RemoteNotificationAdapterSendStatus::RetryableFailure
        )
        .then_some(sent_at_ms.saturating_add(60_000)),
        adapter_message_id,
        error_summary,
        source_trace: if matches!(status, RemoteNotificationAdapterSendStatus::Delivered) {
            REMOTE_FEISHU_DELIVERY_REASON_EXECUTED.to_owned()
        } else {
            REMOTE_FEISHU_DELIVERY_REASON_EXECUTION_FAILED.to_owned()
        },
    }
}

pub(crate) fn plan_reply_message_request(
    base: &str,
    accepted_record: &RemoteNotificationOutboxRecord,
    reply_to_message_id: &str,
    msg_type: &str,
    content_json: &str,
) -> RemoteFeishuHttpRequestPlan {
    RemoteFeishuHttpRequestPlan {
        dispatch_id: accepted_record.dispatch_id.clone(),
        url: format!(
            "{base}{REMOTE_FEISHU_MESSAGE_CREATE_PATH}/{reply_to_message_id}{REMOTE_FEISHU_MESSAGE_REPLY_PATH_SUFFIX}"
        ),
        method: "POST".to_owned(),
        headers: json_headers(Some("__TOKEN__")),
        body_json: serde_json::json!({
            "msg_type": msg_type,
            "content": content_json,
            "uuid": accepted_record.dispatch_id,
        })
        .to_string(),
    }
}

fn should_fallback_from_reply(response: &RemoteFeishuHttpResponse) -> bool {
    let code = serde_json::from_str::<serde_json::Value>(&response.body)
        .ok()
        .and_then(|value| value["code"].as_i64());
    code.is_some_and(|value| REMOTE_FEISHU_REPLY_FALLBACK_CODES.contains(&value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_lost_response_keeps_the_same_feishu_uuid() {
        let request = RemoteFeishuHttpRequestPlan {
            dispatch_id: "dispatch-idempotent".to_owned(),
            url: "https://open.feishu.cn/open-apis/im/v1/messages".to_owned(),
            method: "POST".to_owned(),
            headers: Vec::new(),
            body_json: serde_json::json!({
                "receive_id": "oc_chat",
                "msg_type": "text",
                "content": "{\\\"text\\\":\\\"hello\\\"}",
                "uuid": "dispatch-idempotent",
            })
            .to_string(),
        };
        let mut request_bodies = Vec::new();

        let response =
            execute_with_retry(&request, &mut |attempt: &RemoteFeishuHttpRequestPlan| {
                request_bodies.push(attempt.body_json.clone());
                if request_bodies.len() == 1 {
                    return Err(AxonError::runtime_failure("模拟服务端已接收但响应丢失"));
                }
                Ok(RemoteFeishuHttpResponse {
                    status: 200,
                    body: r#"{"code":0,"data":{"message_id":"om_idempotent"}}"#.to_owned(),
                })
            })
            .expect("retry succeeds");

        assert_eq!(response.status, 200);
        assert_eq!(request_bodies.len(), 2);
        assert_eq!(request_bodies[0], request_bodies[1]);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&request_bodies[1]).expect("request json")["uuid"],
            "dispatch-idempotent"
        );
    }
}
