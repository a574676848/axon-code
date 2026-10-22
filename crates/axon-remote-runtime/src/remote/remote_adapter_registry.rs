//! 远控 adapter registry。
//!
//! 该模块只声明平台 adapter 的可插拔边界；通用远控主链不直接依赖 Feishu、
//! Mobile 或 Web 的 OpenAPI、WebSocket、卡片 payload 和 worker 生命周期。

use crate::RemoteTransportKind;

/// 远控 adapter 类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteAdapterKind {
    /// Feishu WebSocket adapter。
    FeishuWebSocket,
    /// Mobile app adapter。
    MobileApp,
    /// Web console adapter。
    WebConsole,
}

/// 远控 adapter 描述。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteAdapterDescriptor {
    /// 传输类型。
    pub transport: RemoteTransportKind,
    /// adapter 类型。
    pub adapter: Option<RemoteAdapterKind>,
    /// 是否是平台插件 adapter。
    pub platform_adapter: bool,
    /// 是否已注册通知 renderer。
    pub notification_renderer_registered: bool,
    /// 是否已注册入站 gateway。
    pub inbound_gateway_registered: bool,
    /// 默认是否允许主动网络发送。
    pub network_send_allowed_by_default: bool,
}

/// 远控 adapter registry runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct RemoteAdapterRegistryRuntime;

impl RemoteAdapterRegistryRuntime {
    /// 查询 transport 的 adapter 描述。
    #[must_use]
    pub const fn descriptor(transport: RemoteTransportKind) -> RemoteAdapterDescriptor {
        match transport {
            RemoteTransportKind::Feishu => RemoteAdapterDescriptor {
                transport,
                adapter: Some(RemoteAdapterKind::FeishuWebSocket),
                platform_adapter: true,
                notification_renderer_registered: true,
                inbound_gateway_registered: true,
                network_send_allowed_by_default: false,
            },
            RemoteTransportKind::Mobile => RemoteAdapterDescriptor {
                transport,
                adapter: Some(RemoteAdapterKind::MobileApp),
                platform_adapter: true,
                notification_renderer_registered: false,
                inbound_gateway_registered: false,
                network_send_allowed_by_default: false,
            },
            RemoteTransportKind::Web => RemoteAdapterDescriptor {
                transport,
                adapter: Some(RemoteAdapterKind::WebConsole),
                platform_adapter: true,
                notification_renderer_registered: false,
                inbound_gateway_registered: false,
                network_send_allowed_by_default: false,
            },
            RemoteTransportKind::Http | RemoteTransportKind::WebSocket => RemoteAdapterDescriptor {
                transport,
                adapter: None,
                platform_adapter: false,
                notification_renderer_registered: false,
                inbound_gateway_registered: false,
                network_send_allowed_by_default: false,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_distinguishes_platform_adapters_from_raw_transports() {
        let descriptor = RemoteAdapterRegistryRuntime::descriptor(RemoteTransportKind::Feishu);
        assert_eq!(descriptor.adapter, Some(RemoteAdapterKind::FeishuWebSocket));
        assert!(descriptor.platform_adapter);
        assert!(descriptor.notification_renderer_registered);
        assert!(descriptor.inbound_gateway_registered);
        assert!(!descriptor.network_send_allowed_by_default);

        let http = RemoteAdapterRegistryRuntime::descriptor(RemoteTransportKind::Http);
        let websocket = RemoteAdapterRegistryRuntime::descriptor(RemoteTransportKind::WebSocket);
        assert_eq!(http.adapter, None);
        assert_eq!(websocket.adapter, None);
        assert!(!http.platform_adapter);
        assert!(!websocket.platform_adapter);
    }
}
