//! 无 UI 入站渠道到会话的绑定。
//!
//! 飞书群、外部 webhook 这类入口没有客户端状态，消息进来时必须先回答
//! 「这条消息属于哪个会话」。绑定表就是这份答案的唯一事实源：`(channel, destination)`
//! 命中即投进指定会话，未命中才按路由派生一个会话身份。
//!
//! 绑定只服务入站路由，不参与会话身份本身：桌面与 TUI 直接传 `session_id`，不写这张表；
//! 同一个会话可以被多个渠道绑定，因此表里多行可以指向同一个 `session_id`。

use axon_domain::DaemonSessionId;

/// 渠道绑定读写端口。
pub trait ChannelBindingStore {
    /// 建立或改写一条绑定：同一 `(channel, destination)` 只保留最新会话。
    fn bind_channel(
        &self,
        channel: &str,
        destination: &str,
        session_id: &DaemonSessionId,
        now_ms: u64,
    ) -> Result<(), ChannelBindingStoreError>;

    /// 解析绑定；未绑定时返回 `None`，由调用方回落到路由派生。
    fn resolve_channel_binding(
        &self,
        channel: &str,
        destination: &str,
    ) -> Result<Option<DaemonSessionId>, ChannelBindingStoreError>;

    /// 删除一条绑定；本来就不存在时返回 `false`。
    fn unbind_channel(
        &self,
        channel: &str,
        destination: &str,
    ) -> Result<bool, ChannelBindingStoreError>;

    /// 列出某个会话的全部绑定，用于排查「这条会话为什么收到渠道消息」。
    fn list_channel_bindings(
        &self,
        session_id: &DaemonSessionId,
    ) -> Result<Vec<ChannelBinding>, ChannelBindingStoreError>;
}

/// 一条绑定事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelBinding {
    /// 来源渠道（如 `feishu`）。
    pub channel: String,
    /// 该渠道内的目的地（群 ID、webhook 路径等）。
    pub destination: String,
    /// 绑定到的会话。
    pub session_id: DaemonSessionId,
    /// 最近一次改写时刻。
    pub updated_at_ms: u64,
}

/// 绑定读写错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelBindingStoreError {
    /// 存储层错误。
    Storage(String),
    /// 入参不合法（空渠道、空目的地等）。
    InvalidInput(String),
}

impl std::fmt::Display for ChannelBindingStoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Storage(reason) => write!(formatter, "渠道绑定存储错误：{reason}"),
            Self::InvalidInput(reason) => write!(formatter, "渠道绑定入参非法：{reason}"),
        }
    }
}

impl std::error::Error for ChannelBindingStoreError {}
