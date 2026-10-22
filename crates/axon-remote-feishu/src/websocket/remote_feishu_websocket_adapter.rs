//! Feishu WebSocket 远控 adapter 发送计划。
//!
//! 本模块先只规划主链需要的 adapter dispatch plan，不直接发起网络连接。
//! 真实 WebSocket 发送器基于本地动作计划执行。

use axon_domain::{AxonError, AxonResult};

use crate::{
    RemoteAdapterKind, RemoteAdapterRegistryRuntime, RemoteNotificationAdapterKind,
    RemoteNotificationAdapterRegistryRuntime, RemoteNotificationCard,
    RemoteNotificationRenderArtifact, RemoteTransportKind,
};

/// Feishu WebSocket 默认地址。
pub const REMOTE_FEISHU_WEBSOCKET_DEFAULT_URL: &str = "wss://open.feishu.cn/open-apis/ws/v2";
/// Feishu WebSocket dispatch 已规划原因。
pub const REMOTE_FEISHU_WEBSOCKET_REASON_DISPATCH_PLANNED: &str =
    "remote_feishu_websocket:dispatch_planned";
/// Feishu WebSocket 使用默认地址原因。
pub const REMOTE_FEISHU_WEBSOCKET_REASON_DEFAULT_URL: &str = "remote_feishu_websocket:default_url";
/// Feishu WebSocket 网络发送未开放原因。
pub const REMOTE_FEISHU_WEBSOCKET_REASON_NETWORK_DISABLED: &str =
    "remote_feishu_websocket:network_disabled";

/// Feishu WebSocket adapter 配置快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuWebSocketAdapterConfig {
    /// 是否启用。
    pub enabled: bool,
    /// Feishu app_id。
    pub app_id: String,
    /// Feishu app_secret 环境变量名。
    pub app_secret_env: String,
    /// WebSocket URL。
    pub websocket_url: Option<String>,
    /// 是否允许发送通知。
    pub enable_notification_sending: bool,
}

/// Feishu WebSocket dispatch 计划。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuWebSocketDispatchPlan {
    /// adapter 类型。
    pub adapter: RemoteNotificationAdapterKind,
    /// Feishu app_id。
    pub app_id: String,
    /// Feishu app_secret 环境变量名。
    pub app_secret_env: String,
    /// 最终连接地址。
    pub websocket_url: String,
    /// 渲染产物。
    pub artifact: RemoteNotificationRenderArtifact,
    /// 是否允许真实发送。
    pub notification_sending_enabled: bool,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// Feishu WebSocket adapter runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct RemoteFeishuWebSocketAdapterRuntime;

impl RemoteFeishuWebSocketAdapterRuntime {
    /// 基于统一 card 与 registry 规划 Feishu WebSocket dispatch。
    ///
    /// # Errors
    ///
    /// 当配置缺失、未启用、渲染失败或 renderer 不属于 Feishu 时返回错误。
    pub fn plan_dispatch(
        config: &RemoteFeishuWebSocketAdapterConfig,
        card: &RemoteNotificationCard,
    ) -> AxonResult<RemoteFeishuWebSocketDispatchPlan> {
        if !config.enabled {
            return Err(AxonError::runtime_failure(
                "Feishu WebSocket 远控 adapter 未启用",
            ));
        }
        if config.app_id.trim().is_empty() || config.app_secret_env.trim().is_empty() {
            return Err(AxonError::runtime_failure(
                "Feishu WebSocket 远控凭据配置无效",
            ));
        }
        let descriptor = RemoteAdapterRegistryRuntime::descriptor(RemoteTransportKind::Feishu);
        if descriptor.adapter != Some(RemoteAdapterKind::FeishuWebSocket)
            || !descriptor.inbound_gateway_registered
        {
            return Err(AxonError::runtime_failure(
                "Feishu WebSocket 远控 adapter 未注册",
            ));
        }
        let artifact = RemoteNotificationAdapterRegistryRuntime::render(card)?;
        if artifact.adapter != RemoteNotificationAdapterKind::FeishuWebSocket {
            return Err(AxonError::runtime_failure(
                "远控通知 card 未命中 Feishu WebSocket adapter",
            ));
        }
        let websocket_url = config
            .websocket_url
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(REMOTE_FEISHU_WEBSOCKET_DEFAULT_URL)
            .to_owned();
        let mut reasons = vec![REMOTE_FEISHU_WEBSOCKET_REASON_DISPATCH_PLANNED.to_owned()];
        if config
            .websocket_url
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
        {
            reasons.push(REMOTE_FEISHU_WEBSOCKET_REASON_DEFAULT_URL.to_owned());
        }
        if !config.enable_notification_sending {
            reasons.push(REMOTE_FEISHU_WEBSOCKET_REASON_NETWORK_DISABLED.to_owned());
        }
        Ok(RemoteFeishuWebSocketDispatchPlan {
            adapter: artifact.adapter,
            app_id: config.app_id.clone(),
            app_secret_env: config.app_secret_env.clone(),
            websocket_url,
            artifact,
            notification_sending_enabled: config.enable_notification_sending,
            reasons,
        })
    }
}

#[cfg(test)]
mod tests {
    use axon_domain::SessionId;

    use crate::{
        RemoteNotificationAdapterEnvelopeSurface, RemoteNotificationCard,
        RemoteNotificationCardTone, RemoteTransportKind,
    };

    use super::*;

    #[test]
    fn feishu_websocket_dispatch_plan_uses_registry_and_default_url() {
        let plan = RemoteFeishuWebSocketAdapterRuntime::plan_dispatch(&config(), &card())
            .expect("plan dispatch");

        assert_eq!(plan.adapter, RemoteNotificationAdapterKind::FeishuWebSocket);
        assert_eq!(plan.websocket_url, REMOTE_FEISHU_WEBSOCKET_DEFAULT_URL);
        assert!(!plan.notification_sending_enabled);
        assert!(
            plan.reasons
                .iter()
                .any(|reason| reason == REMOTE_FEISHU_WEBSOCKET_REASON_DEFAULT_URL)
        );
    }

    #[test]
    fn feishu_websocket_dispatch_plan_rejects_disabled_adapter() {
        let mut config = config();
        config.enabled = false;

        assert!(RemoteFeishuWebSocketAdapterRuntime::plan_dispatch(&config, &card()).is_err());
    }

    fn config() -> RemoteFeishuWebSocketAdapterConfig {
        RemoteFeishuWebSocketAdapterConfig {
            enabled: true,
            app_id: "cli_xxx".to_owned(),
            app_secret_env: "FEISHU_APP_SECRET".to_owned(),
            websocket_url: None,
            enable_notification_sending: false,
        }
    }

    fn card() -> RemoteNotificationCard {
        RemoteNotificationCard {
            session_id: SessionId(11),
            transport: RemoteTransportKind::Feishu,
            surface: RemoteNotificationAdapterEnvelopeSurface::FeishuCard,
            title: "Axon 飞书远控通知".to_owned(),
            body: "会话 11 等待审批".to_owned(),
            kind_summary: "等待审批".to_owned(),
            delivery_count: 1,
            tone: RemoteNotificationCardTone::Warning,
            local_only: true,
            network_send_allowed: false,
            raw_payload_forward_allowed: false,
        }
    }
}
