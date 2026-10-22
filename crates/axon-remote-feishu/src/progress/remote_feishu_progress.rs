//! Feishu 远控执行进度消息客户端。

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axon_display::normalize_markdown_source;
use axon_domain::SemanticOutput;
use axon_domain::{AxonError, AxonResult};

use crate::remote_feishu_delivery_http::{json_headers, parse_tenant_access_token_with_expiry};
use crate::remote_feishu_delivery_types::{
    REMOTE_FEISHU_API_BASE_URL, REMOTE_FEISHU_MESSAGE_CREATE_PATH,
    REMOTE_FEISHU_TENANT_ACCESS_TOKEN_PATH, RemoteFeishuHttpRequestPlan, RemoteFeishuHttpResponse,
    RemoteFeishuMessageTarget,
};
use crate::remote_feishu_oapi_executor::RemoteFeishuOapiExecutorRuntime;
use crate::remote_feishu_outbound::{
    CONTENT_TYPE_MARKDOWN, REMOTE_FEISHU_OUTBOUND_MSG_TYPE_INTERACTIVE,
    RemoteFeishuOutboundMessagePlan, RemoteFeishuOutboundMessageRuntime,
};
use crate::{RemoteNotificationAdapterKind, RemoteNotificationRenderArtifact, RemoteTransportKind};

const DEFAULT_TOKEN_TTL_SECS: u64 = 7_200;
const TOKEN_REFRESH_SKEW_SECS: u64 = 60;

/// 飞书 `im/v1/messages` 的 `uuid` 字段长度上限。
///
/// 这是**服务端硬约束**：超过该长度飞书直接返回 `code 99992402 /
/// field validation failed`，并附带 `field_violations: [{field: "uuid",
/// description: "the max len is 50"}]`。因此任何进入 `uuid` 的值都必须先被压到
/// 这个上界内，绝不能依赖调用方传入的 id 恰好够短。
const FEISHU_MESSAGE_UUID_MAX_CHARS: usize = 50;
/// 压缩后保留的前缀：保留可读来源，便于在飞书侧与本地 dispatch 对应。
const FEISHU_MESSAGE_UUID_PREFIX: &str = "fp-";
/// 压缩后保留的散列字符数：前缀 + 散列 = 恰好 `FEISHU_MESSAGE_UUID_MAX_CHARS`。
const FEISHU_MESSAGE_UUID_DIGEST_CHARS: usize =
    FEISHU_MESSAGE_UUID_MAX_CHARS - FEISHU_MESSAGE_UUID_PREFIX.len();

#[derive(Debug)]
struct TokenState {
    token: String,
    valid_until: Instant,
}

/// 可复用的 Feishu 执行进度消息客户端。
#[derive(Debug, Clone)]
pub struct RemoteFeishuProgressClient {
    base_url: String,
    app_id: String,
    app_secret: String,
    token_state: Arc<Mutex<TokenState>>,
    request_timeout: Duration,
}

impl RemoteFeishuProgressClient {
    /// 建立客户端并获取 tenant access token；请求前会按过期时间自动刷新。
    ///
    /// 进度投影是可降级的实时视图，单次请求严格受调用方 deadline 约束；
    /// 最终回复仍由 durable outbox 负责补偿。
    pub fn connect(
        app_id: &str,
        app_secret: &str,
        api_base_url: Option<&str>,
        request_timeout: Duration,
    ) -> AxonResult<Self> {
        let base_url = normalized_base_url(api_base_url.unwrap_or(REMOTE_FEISHU_API_BASE_URL))?;
        let (tenant_access_token, expires_in) =
            fetch_tenant_access_token(&base_url, app_id, app_secret, request_timeout)?;
        Ok(Self {
            base_url,
            app_id: app_id.to_owned(),
            app_secret: app_secret.to_owned(),
            token_state: Arc::new(Mutex::new(TokenState {
                token: tenant_access_token,
                valid_until: token_valid_until(expires_in),
            })),
            request_timeout,
        })
    }

    /// 创建首条执行进度消息并返回消息 ID。
    pub fn create(
        &self,
        target: &RemoteFeishuMessageTarget,
        body: &str,
        dispatch_id: &str,
    ) -> AxonResult<String> {
        let plan = progress_message_plan(body);
        self.create_message(target, &plan.msg_type, &plan.content_json, dispatch_id)
    }

    /// 创建一条经过共享语义规范化的 Markdown 回合消息。
    pub fn create_markdown(
        &self,
        target: &RemoteFeishuMessageTarget,
        body: &str,
        dispatch_id: &str,
    ) -> AxonResult<String> {
        let plan = markdown_message_plan(body)?;
        self.create_message(target, &plan.msg_type, &plan.content_json, dispatch_id)
    }

    /// 创建语义输出消息或进度卡片。
    pub fn create_semantic_output(
        &self,
        target: &RemoteFeishuMessageTarget,
        output: &SemanticOutput,
        dispatch_id: &str,
    ) -> AxonResult<String> {
        let plan = crate::RemoteFeishuSemanticRenderer::render(output)?;
        self.create_message(target, &plan.msg_type, &plan.content_json, dispatch_id)
    }

    fn create_message(
        &self,
        target: &RemoteFeishuMessageTarget,
        message_type: &str,
        content_json: &str,
        dispatch_id: &str,
    ) -> AxonResult<String> {
        // 幂等键必须在**所有创建分支**上满足飞书的 uuid 长度上限：进度卡、工具卡与
        // 语义输出都汇聚到这里，任何一条超限都会被飞书整条拒绝。
        let request =
            self.create_message_request(target, message_type, content_json, dispatch_id)?;
        let response = self.execute_progress_request(&request)?;
        parse_message_id(response)
    }

    /// 构造一条创建消息请求；幂等键在此统一压到飞书 uuid 上限内。
    fn create_message_request(
        &self,
        target: &RemoteFeishuMessageTarget,
        message_type: &str,
        content_json: &str,
        dispatch_id: &str,
    ) -> AxonResult<RemoteFeishuHttpRequestPlan> {
        let uuid = feishu_message_uuid(dispatch_id);
        if let Some(reply_to_message_id) = target.reply_to_message_id.as_deref() {
            Ok(RemoteFeishuHttpRequestPlan {
                dispatch_id: dispatch_id.to_owned(),
                url: format!(
                    "{}{}/{}/reply",
                    self.base_url, REMOTE_FEISHU_MESSAGE_CREATE_PATH, reply_to_message_id
                ),
                method: "POST".to_owned(),
                headers: json_headers(Some(&self.tenant_access_token()?)),
                body_json: message_body(message_type, content_json, &uuid),
            })
        } else {
            Ok(RemoteFeishuHttpRequestPlan {
                dispatch_id: dispatch_id.to_owned(),
                url: format!(
                    "{}{}?receive_id_type={}",
                    self.base_url, REMOTE_FEISHU_MESSAGE_CREATE_PATH, target.receive_id_type
                ),
                method: "POST".to_owned(),
                headers: json_headers(Some(&self.tenant_access_token()?)),
                body_json: serde_json::json!({
                    "receive_id": target.receive_id,
                    "msg_type": message_type,
                    "content": content_json,
                    "uuid": uuid,
                })
                .to_string(),
            })
        }
    }

    /// 创建一张交互卡片（问答卡等）并返回消息 ID。
    ///
    /// 与进度投影共用同一幂等键压缩：同一 dispatch id 重复建卡在飞书侧只落一条，
    /// 因此网络失败后的重试不会产生第二张卡片。
    pub fn create_interactive_card(
        &self,
        target: &RemoteFeishuMessageTarget,
        card: &serde_json::Value,
        dispatch_id: &str,
    ) -> AxonResult<String> {
        let content_json = serde_json::to_string(card)
            .map_err(|_| AxonError::runtime_failure("Feishu 卡片序列化失败"))?;
        let request = self.create_message_request(
            target,
            REMOTE_FEISHU_OUTBOUND_MSG_TYPE_INTERACTIVE,
            &content_json,
            dispatch_id,
        )?;
        let response =
            crate::remote_feishu_delivery_http::execute_with_retry(&request, &mut |request| {
                self.execute_progress_request(request)
            })?;
        parse_message_id(response)
    }

    /// 用卡片 JSON 整体替换一条已创建的交互卡片。
    ///
    /// 结算回写必须幂等：同一结算结论重复 PATCH 只覆盖同一张卡片，不产生新消息。
    pub fn edit_interactive_card(
        &self,
        message_id: &str,
        card: &serde_json::Value,
        dispatch_id: &str,
    ) -> AxonResult<()> {
        let content_json = serde_json::to_string(card)
            .map_err(|_| AxonError::runtime_failure("Feishu 卡片序列化失败"))?;
        let request = RemoteFeishuHttpRequestPlan {
            dispatch_id: dispatch_id.to_owned(),
            url: format!(
                "{}{}/{}",
                self.base_url, REMOTE_FEISHU_MESSAGE_CREATE_PATH, message_id
            ),
            method: "PATCH".to_owned(),
            headers: json_headers(Some(&self.tenant_access_token()?)),
            body_json: card_update_body(&content_json),
        };
        let response =
            crate::remote_feishu_delivery_http::execute_with_retry(&request, &mut |request| {
                self.execute_progress_request(request)
            })?;
        ensure_success(&response)
    }

    /// 编辑已创建的进度消息。
    pub fn edit(&self, message_id: &str, body: &str, dispatch_id: &str) -> AxonResult<()> {
        let plan = progress_message_plan(body);
        let request = RemoteFeishuHttpRequestPlan {
            dispatch_id: dispatch_id.to_owned(),
            url: format!(
                "{}{}/{}",
                self.base_url, REMOTE_FEISHU_MESSAGE_CREATE_PATH, message_id
            ),
            method: "PATCH".to_owned(),
            headers: json_headers(Some(&self.tenant_access_token()?)),
            body_json: update_message_body(&plan),
        };
        let response = self.execute_progress_request(&request)?;
        ensure_success(&response)
    }

    fn tenant_access_token(&self) -> AxonResult<String> {
        let mut state = self
            .token_state
            .lock()
            .map_err(|_| AxonError::runtime_failure("Feishu token 状态锁已损坏"))?;
        if Instant::now() < state.valid_until {
            return Ok(state.token.clone());
        }
        let (token, expires_in) = fetch_tenant_access_token(
            &self.base_url,
            &self.app_id,
            &self.app_secret,
            self.request_timeout,
        )?;
        state.token = token.clone();
        state.valid_until = token_valid_until(expires_in);
        Ok(token)
    }

    fn execute_progress_request(
        &self,
        request: &RemoteFeishuHttpRequestPlan,
    ) -> AxonResult<RemoteFeishuHttpResponse> {
        RemoteFeishuOapiExecutorRuntime::execute_json_with_timeout(request, self.request_timeout)
    }
}

fn markdown_message_plan(body: &str) -> AxonResult<RemoteFeishuOutboundMessagePlan> {
    RemoteFeishuOutboundMessageRuntime::plan_from_artifact(&RemoteNotificationRenderArtifact {
        adapter: RemoteNotificationAdapterKind::FeishuWebSocket,
        transport: RemoteTransportKind::Feishu,
        content_type: CONTENT_TYPE_MARKDOWN.to_owned(),
        payload: normalize_markdown_source(body).trim().to_owned(),
        network_send_allowed: true,
    })
}

fn progress_message_plan(body: &str) -> RemoteFeishuOutboundMessagePlan {
    let normalized = normalize_markdown_source(body).trim().to_owned();
    RemoteFeishuOutboundMessagePlan {
        msg_type: REMOTE_FEISHU_OUTBOUND_MSG_TYPE_INTERACTIVE.to_owned(),
        content_json: serde_json::json!({
            "schema": "2.0",
            "body": {
                "elements": [{
                    "tag": "markdown",
                    "content": normalized,
                }]
            }
        })
        .to_string(),
    }
}

fn message_body(message_type: &str, content_json: &str, dispatch_id: &str) -> String {
    serde_json::json!({
        "msg_type": message_type,
        "content": content_json,
        "uuid": dispatch_id,
    })
    .to_string()
}

/// 把 dispatch id 压成飞书可接受的 `uuid`。
///
/// 契约：
/// - **确定性**：同一 `dispatch_id` 恒定映射到同一 `uuid`，因此飞书侧的去重语义
///   （同 uuid 重复创建只落一条）不会被破坏；
/// - **有界**：输出恰好 `FEISHU_MESSAGE_UUID_MAX_CHARS` 字符，与输入长度无关：
///   真机事故正是 `feishu-progress-{24 位 dispatch}-{16 位工具哈希}` = 57 字符，
///   越过飞书的 50 上界后被逐条拒绝（954 次投影全部失败）；
/// - **保形**：只使用小写十六进制与连字符，避免不同版本对 uuid 字符集的额外限制。
fn feishu_message_uuid(dispatch_id: &str) -> String {
    if dispatch_id.chars().count() <= FEISHU_MESSAGE_UUID_MAX_CHARS {
        return dispatch_id.to_owned();
    }
    let digest =
        axon_crypto::sha256_hex_prefix(dispatch_id.as_bytes(), FEISHU_MESSAGE_UUID_DIGEST_CHARS);
    format!("{FEISHU_MESSAGE_UUID_PREFIX}{digest}")
}

fn update_message_body(plan: &RemoteFeishuOutboundMessagePlan) -> String {
    serde_json::json!({
        "content": plan.content_json,
    })
    .to_string()
}

/// 卡片整体替换请求体：只带 `content`，不带 `msg_type` 与 `uuid`。
fn card_update_body(content_json: &str) -> String {
    serde_json::json!({
        "content": content_json,
    })
    .to_string()
}

fn parse_message_id(response: RemoteFeishuHttpResponse) -> AxonResult<String> {
    ensure_success(&response)?;
    serde_json::from_str::<serde_json::Value>(&response.body)
        .ok()
        .and_then(|value| value["data"]["message_id"].as_str().map(ToOwned::to_owned))
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AxonError::runtime_failure("Feishu 进度消息响应缺少 message_id"))
}

fn ensure_success(response: &RemoteFeishuHttpResponse) -> AxonResult<()> {
    let value = serde_json::from_str::<serde_json::Value>(&response.body).unwrap_or_default();
    if response.status == 200 && value["code"].as_i64().unwrap_or_default() == 0 {
        return Ok(());
    }
    Err(AxonError::runtime_failure(format!(
        "Feishu 进度消息 API 失败：{}",
        value["msg"].as_str().unwrap_or("unknown")
    )))
}

fn normalized_base_url(value: &str) -> AxonResult<String> {
    let trimmed = value.trim().trim_end_matches('/');
    if trimmed.starts_with("https://") || trimmed.starts_with("http://") {
        return Ok(trimmed.to_owned());
    }
    Err(AxonError::runtime_failure(
        "Feishu API base URL 必须为 http/https",
    ))
}

fn fetch_tenant_access_token(
    base_url: &str,
    app_id: &str,
    app_secret: &str,
    request_timeout: Duration,
) -> AxonResult<(String, Option<u64>)> {
    let request = RemoteFeishuHttpRequestPlan {
        dispatch_id: "feishu-progress-token".to_owned(),
        url: format!("{base_url}{REMOTE_FEISHU_TENANT_ACCESS_TOKEN_PATH}"),
        method: "POST".to_owned(),
        headers: json_headers(None),
        body_json: serde_json::json!({
            "app_id": app_id,
            "app_secret": app_secret,
        })
        .to_string(),
    };
    let response =
        RemoteFeishuOapiExecutorRuntime::execute_json_with_timeout(&request, request_timeout)?;
    parse_tenant_access_token_with_expiry(&response)
}

fn token_valid_until(expires_in: Option<u64>) -> Instant {
    let ttl = expires_in.unwrap_or(DEFAULT_TOKEN_TTL_SECS);
    Instant::now() + Duration::from_secs(ttl.saturating_sub(TOKEN_REFRESH_SKEW_SECS).max(1))
}

#[cfg(test)]
mod tests {
    use axon_display::normalize_markdown_source;

    use super::{
        FEISHU_MESSAGE_UUID_MAX_CHARS, feishu_message_uuid, markdown_message_plan,
        progress_message_plan, update_message_body,
    };
    use crate::remote_feishu_delivery_http::parse_tenant_access_token_with_expiry;
    use crate::{
        REMOTE_FEISHU_OUTBOUND_MSG_TYPE_INTERACTIVE, REMOTE_FEISHU_OUTBOUND_MSG_TYPE_POST,
        RemoteFeishuHttpResponse,
    };

    /// 真机事故守卫：工具卡的 dispatch id 是
    /// `feishu-progress-{24 位 dispatch}-{16 位工具哈希}` = 57 字符，越过飞书 `uuid`
    /// 的 50 字符上界后被逐条拒绝（实测 954 次投影全部失败、飞书消息一条都没出去）。
    #[test]
    fn every_progress_uuid_stays_within_the_feishu_limit() {
        let dispatch_24 = "ae4c31ea28a4e1fb772a3d0f";
        let tool_hash_16 = "5ab3219bbf754c41";
        let tool_card = format!("feishu-progress-{dispatch_24}-{tool_hash_16}");
        assert!(
            tool_card.chars().count() > FEISHU_MESSAGE_UUID_MAX_CHARS,
            "该用例的前提是原始 id 确实超限，否则它不再守任何东西"
        );

        for dispatch_id in [
            tool_card.as_str(),
            "feishu-progress-ae4c31ea28a4e1fb772a3d0f",
            "short",
            "",
            &"x".repeat(4_096),
        ] {
            let uuid = feishu_message_uuid(dispatch_id);
            assert!(
                uuid.chars().count() <= FEISHU_MESSAGE_UUID_MAX_CHARS,
                "uuid 必须始终不超过飞书上限：{} 字符 -> {uuid}",
                uuid.chars().count()
            );
        }
    }

    /// 压缩必须是确定性的：飞书按 uuid 去重，同一回合的重复投影必须映射到同一 uuid。
    #[test]
    fn progress_uuid_compaction_is_deterministic() {
        let long = format!("feishu-progress-{}-{}", "a".repeat(64), "b".repeat(32));
        assert_eq!(feishu_message_uuid(&long), feishu_message_uuid(&long));
        assert_ne!(
            feishu_message_uuid(&long),
            feishu_message_uuid(&format!("{long}c")),
            "不同 dispatch id 不应压缩成同一 uuid"
        );
    }

    /// 未超限的 id 必须原样保留，避免给既有幂等键引入无谓变更。
    #[test]
    fn progress_uuid_preserves_ids_within_the_limit() {
        let exact = "u".repeat(FEISHU_MESSAGE_UUID_MAX_CHARS);
        assert_eq!(feishu_message_uuid(&exact), exact);
        assert_eq!(
            feishu_message_uuid("feishu-progress-ae4c31ea28a4e1fb772a3d0f"),
            "feishu-progress-ae4c31ea28a4e1fb772a3d0f"
        );
    }

    #[test]
    fn tenant_access_token_parser_keeps_expire_seconds() {
        let response = RemoteFeishuHttpResponse {
            status: 200,
            body: r#"{"code":0,"tenant_access_token":"token","expire":7200}"#.to_owned(),
        };

        assert_eq!(
            parse_tenant_access_token_with_expiry(&response).expect("token 响应应有效"),
            ("token".to_owned(), Some(7_200))
        );
    }

    #[test]
    fn mutable_progress_uses_interactive_card_from_creation() {
        let plan = progress_message_plan("Axon 执行中\n• 运行 shell_command");
        let content: serde_json::Value =
            serde_json::from_str(&plan.content_json).expect("content json");

        assert_eq!(plan.msg_type, REMOTE_FEISHU_OUTBOUND_MSG_TYPE_INTERACTIVE);
        assert_eq!(content["schema"], "2.0");
        assert_eq!(content["body"]["elements"][0]["tag"], "markdown");
        assert_eq!(
            content["body"]["elements"][0]["content"],
            "Axon 执行中\n• 运行 shell_command"
        );
    }

    #[test]
    fn progress_update_uses_card_api_body_without_create_uuid() {
        let body = update_message_body(&progress_message_plan("Axon 执行中"));
        let request: serde_json::Value = serde_json::from_str(&body).expect("request json");

        assert!(request["content"].as_str().is_some());
        assert!(request.get("msg_type").is_none());
        assert!(request.get("uuid").is_none());
    }

    #[test]
    fn progress_markdown_reuses_shared_semantic_projection() {
        let source = "### 任务标题\u{3000}-\u{00a0}状态：进行中 - 任务目标：核验消息链路";

        let plan = markdown_message_plan(source).expect("plan");
        let content: serde_json::Value =
            serde_json::from_str(&plan.content_json).expect("content json");

        assert_eq!(plan.msg_type, REMOTE_FEISHU_OUTBOUND_MSG_TYPE_POST);
        assert_eq!(
            content["zh_cn"]["content"][0][0]["text"],
            normalize_markdown_source(source).trim()
        );
    }
}
