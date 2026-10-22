//! 远控通知 adapter renderer registry。
//!
//! 远控 adapter 的可插拔描述由 `remote_adapter_registry` 维护；本模块只负责
//! 把平台无关通知 card 渲染为已注册 adapter 的发送载荷。

use axon_domain::{AxonError, AxonResult};

use axon_remote_runtime::{RemoteNotificationCard, RemoteTransportKind};

use crate::RemoteNotificationFeishuRendererRuntime;

pub use axon_remote_runtime::{
    RemoteAdapterDescriptor as RemoteNotificationAdapterDescriptor,
    RemoteAdapterKind as RemoteNotificationAdapterKind,
};

/// 远控通知 adapter 渲染产物。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteNotificationRenderArtifact {
    /// adapter 类型。
    pub adapter: RemoteNotificationAdapterKind,
    /// 传输类型。
    pub transport: RemoteTransportKind,
    /// 载荷 mime。
    pub content_type: String,
    /// 渲染产物。
    pub payload: String,
    /// 是否允许直接网络发送。
    pub network_send_allowed: bool,
}

/// 远控通知 adapter 渲染 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct RemoteNotificationAdapterRegistryRuntime;

impl RemoteNotificationAdapterRegistryRuntime {
    /// 查询 transport 的 adapter 描述。
    #[must_use]
    pub const fn descriptor(transport: RemoteTransportKind) -> RemoteNotificationAdapterDescriptor {
        axon_remote_runtime::RemoteAdapterRegistryRuntime::descriptor(transport)
    }

    /// 根据 card 选择并渲染对应 adapter 载荷。
    ///
    /// # Errors
    ///
    /// 当 transport 当前没有已注册 renderer，或 renderer 拒绝该 card 时返回错误。
    pub fn render(card: &RemoteNotificationCard) -> AxonResult<RemoteNotificationRenderArtifact> {
        match card.transport {
            RemoteTransportKind::Feishu => Ok(RemoteNotificationRenderArtifact {
                adapter: RemoteNotificationAdapterKind::FeishuWebSocket,
                transport: card.transport,
                content_type: "application/json".to_owned(),
                payload: RemoteNotificationFeishuRendererRuntime::render(card)?,
                network_send_allowed: false,
            }),
            RemoteTransportKind::Mobile => {
                Err(AxonError::runtime_failure("mobile 远控 adapter 尚未注册"))
            }
            RemoteTransportKind::Web => {
                Err(AxonError::runtime_failure("web 远控 adapter 尚未注册"))
            }
            RemoteTransportKind::Http | RemoteTransportKind::WebSocket => Err(
                AxonError::runtime_failure("当前 transport 未注册远控通知 adapter"),
            ),
        }
    }
}
