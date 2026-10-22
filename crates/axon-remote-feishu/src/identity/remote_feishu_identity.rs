//! Feishu bot identity hydrate 运行时。
//!
//! 使用 `/open-apis/bot/v3/info` 探测实现精确 @mention。
//! 门禁与后续 bot 自身消息识别。

use axon_domain::{AxonError, AxonResult};

use crate::remote_feishu::RemoteFeishuHttpResponse;

/// Feishu bot info 相对路径。
pub const REMOTE_FEISHU_BOT_INFO_PATH: &str = "/open-apis/bot/v3/info";

/// Feishu 机器人身份。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuBotIdentity {
    /// app-scoped open_id。
    pub bot_open_id: String,
    /// 机器人显示名。
    pub bot_name: String,
}

/// Feishu bot info 请求计划。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuBotInfoRequestPlan {
    /// 完整 URL。
    pub url: String,
    /// HTTP 方法。
    pub method: String,
    /// 请求头。
    pub headers: Vec<(String, String)>,
}

/// Feishu bot identity runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct RemoteFeishuIdentityRuntime;

impl RemoteFeishuIdentityRuntime {
    /// 生成 bot info 请求计划。
    pub fn plan_request(
        api_base_url: &str,
        tenant_access_token: &str,
    ) -> AxonResult<RemoteFeishuBotInfoRequestPlan> {
        let base = api_base_url.trim().trim_end_matches('/');
        if (!base.starts_with("https://")) && (!base.starts_with("http://")) {
            return Err(AxonError::runtime_failure(
                "Feishu bot info base URL 必须为 http/https",
            ));
        }
        if tenant_access_token.trim().is_empty() {
            return Err(AxonError::runtime_failure(
                "Feishu bot info tenant_access_token 不能为空",
            ));
        }
        Ok(RemoteFeishuBotInfoRequestPlan {
            url: format!("{base}{REMOTE_FEISHU_BOT_INFO_PATH}"),
            method: "GET".to_owned(),
            headers: vec![
                ("Content-Type".to_owned(), "application/json".to_owned()),
                ("Connection".to_owned(), "close".to_owned()),
                (
                    "Authorization".to_owned(),
                    format!("Bearer {tenant_access_token}"),
                ),
            ],
        })
    }

    /// 从 Feishu bot info 响应解析机器人身份。
    pub fn parse_identity(
        response: &RemoteFeishuHttpResponse,
    ) -> AxonResult<RemoteFeishuBotIdentity> {
        if response.status != 200 {
            return Err(AxonError::runtime_failure("Feishu bot info API 状态码无效"));
        }
        let value = serde_json::from_str::<serde_json::Value>(&response.body)
            .map_err(|_| AxonError::runtime_failure("Feishu bot info 响应 JSON 无效"))?;
        if value["code"].as_i64().unwrap_or(0) != 0 {
            return Err(AxonError::runtime_failure("Feishu bot info API 返回错误"));
        }
        let bot = value
            .get("bot")
            .or_else(|| value.get("data").and_then(|data| data.get("bot")))
            .ok_or(AxonError::runtime_failure("Feishu bot info 缺少 bot 节点"))?;
        let bot_open_id = bot["open_id"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_owned();
        let bot_name = bot["app_name"]
            .as_str()
            .or_else(|| bot["bot_name"].as_str())
            .unwrap_or_default()
            .trim()
            .to_owned();
        if bot_open_id.is_empty() {
            return Err(AxonError::runtime_failure("Feishu bot info 缺少 open_id"));
        }
        Ok(RemoteFeishuBotIdentity {
            bot_open_id,
            bot_name,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feishu_identity_builds_bot_info_request() {
        let request =
            RemoteFeishuIdentityRuntime::plan_request("https://open.feishu.cn", "tenant-token")
                .expect("plan request");

        assert_eq!(request.url, "https://open.feishu.cn/open-apis/bot/v3/info");
        assert_eq!(request.method, "GET");
        assert!(
            request
                .headers
                .iter()
                .any(|(name, value)| name == "Authorization" && value == "Bearer tenant-token")
        );
    }

    #[test]
    fn feishu_identity_parses_bot_info_response() {
        let identity = RemoteFeishuIdentityRuntime::parse_identity(&RemoteFeishuHttpResponse {
            status: 200,
            body: r#"{"code":0,"bot":{"open_id":"ou_bot","app_name":"Axon Bot"}}"#.to_owned(),
        })
        .expect("parse identity");

        assert_eq!(identity.bot_open_id, "ou_bot");
        assert_eq!(identity.bot_name, "Axon Bot");
    }
}
