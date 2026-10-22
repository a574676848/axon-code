//! 客户端 presence 与完成通知持久收件箱端口。
//!
//! 产品定义：一个后台 Execution 完成时只通知主 turn 一次，主 turn 的汇总回复
//! 必须投递到当时所有可用渠道（在线客户端 presence + 已配置的外部通知目标）。因此
//! 持久化被拆成两层：
//!
//! - `CompletionNotificationRecord`：一次 Execution 完成 = 一次主 turn 通知；
//! - `CompletionDeliveryRecord`：该通知在每个渠道上的独立投递状态。
//!
//! 任何渠道都不得静默确认：没有可用渠道时通知停留在收件箱，Result 保持未读；
//! 隔天仍未读的通知整体失效并静默停止投递。

use std::fmt;

/// 客户端 presence 记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientPresenceRecord {
    /// 稳定客户端 ID。
    pub client_id: String,
    /// 目标 Workspace ID。
    pub workspace_id: String,
    /// 投递渠道。
    pub channel: String,
    /// 渠道内投递目标。
    pub destination: String,
    /// 最近一次注册或心跳时间。
    pub registered_at_ms: u64,
    /// presence 过期时间。
    pub expires_at_ms: u64,
}

/// 完成通知的投递状态。
///
/// 该状态同时用于通知级与渠道级：通知级只表达「是否已通知主 turn / 是否已有人
/// 确认送达 / 是否整体失效」，渠道级表达单个渠道的投递进度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationDeliveryState {
    /// 尚未派发到任何渠道。
    Pending,
    /// 已派发到渠道，等待渠道确认送达。
    Dispatched,
    /// 渠道已确认送达。
    Delivered,
    /// 隔天仍未读，通知自动失效并停止投递。
    Expired,
}

impl NotificationDeliveryState {
    /// 持久化取值。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Dispatched => "dispatched",
            Self::Delivered => "delivered",
            Self::Expired => "expired",
        }
    }

    /// 解析持久化取值；未知取值返回 `None`。
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "dispatched" => Some(Self::Dispatched),
            "delivered" => Some(Self::Delivered),
            "expired" => Some(Self::Expired),
            _ => None,
        }
    }

    /// 是否仍需继续投递。
    #[must_use]
    pub const fn is_deliverable(self) -> bool {
        matches!(self, Self::Pending | Self::Dispatched)
    }

    /// 是否已经抵达终态。
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Delivered | Self::Expired)
    }
}

/// 完成通知（持久收件箱条目）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionNotificationRecord {
    /// 稳定通知 ID。
    pub notification_id: String,
    /// 所属 Workspace。
    pub workspace_id: String,
    /// 关联 Goal。
    pub goal_id: String,
    /// 关联 Execution。
    pub execution_id: String,
    /// 通知主 turn 的提示正文；不得直接作为渠道投递正文。
    pub content: String,
    /// 主 turn 的汇总回复正文；Goal/Execution 通知在产出前为空串。
    ///
    /// 该正文是各渠道实际交付内容：产品定义要求完成消息统一由主 turn 汇总
    /// 回复负责，后台 Execution 的原始回复不得直发任何渠道。
    pub summary: String,
    /// 该通知负责确认的 Result ID。
    pub result_ids: Vec<String>,
    /// 通知级投递状态。
    pub state: NotificationDeliveryState,
    /// 创建时间。
    pub created_at_ms: u64,
    /// 最近状态变更时间。
    pub updated_at_ms: u64,
    /// 失效截止时间：跨过该时刻仍未读的通知自动失效，静默停止投递。
    pub expires_at_ms: u64,
}

/// 一个渠道投递目标。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CompletionDeliveryTarget {
    /// 渠道标识。
    pub channel: String,
    /// 渠道内目标。
    pub destination: String,
}

/// 完成通知在单个渠道上的投递记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionDeliveryRecord {
    /// 所属通知。
    pub notification_id: String,
    /// 渠道标识。
    pub channel: String,
    /// 渠道内目标。
    pub destination: String,
    /// 渠道级投递状态。
    pub state: NotificationDeliveryState,
    /// 登记时间。
    pub created_at_ms: u64,
    /// 最近状态变更时间。
    pub updated_at_ms: u64,
}

/// 渠道收件箱条目：已就绪、等待该渠道确认送达的完成通知。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientInboxEntry {
    /// 通知 ID。
    pub notification_id: String,
    /// 关联 Goal。
    pub goal_id: String,
    /// 关联 Execution。
    pub execution_id: String,
    /// 所属渠道。
    pub channel: String,
    /// 该渠道内的投递目标。
    ///
    /// 渠道消费者按目标结算自己的投递记录；进程内渠道投递器据此把正文写入对应
    /// 对话的 durable 出站队列。
    pub destination: String,
    /// 待展示正文：只能是主 turn 汇总回复或已就绪投递项的最终正文。
    pub content: String,
    /// 通知创建时间。
    pub created_at_ms: u64,
    /// 通知失效时间；过期的条目不再返回。
    pub expires_at_ms: u64,
}

/// 客户端投递存储错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientDeliveryStoreError {
    /// 持久化失败。
    Storage(String),
}

impl fmt::Display for ClientDeliveryStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage(detail) => write!(formatter, "客户端投递存储失败：{detail}"),
        }
    }
}

impl std::error::Error for ClientDeliveryStoreError {}

/// 客户端 presence 与完成通知收件箱持久化端口。
pub trait ClientDeliveryStore: Send + Sync {
    /// 幂等注册或续期客户端 presence。
    fn register_client_presence(
        &self,
        presence: &ClientPresenceRecord,
    ) -> Result<(), ClientDeliveryStoreError>;

    /// 注销客户端 presence；不存在时也是幂等的。
    fn unregister_client_presence(&self, client_id: &str) -> Result<(), ClientDeliveryStoreError>;

    /// 读取 Workspace 内尚未过期的全部客户端 presence。
    ///
    /// 产品定义要求完成消息投递到所有可用渠道，因此这里必须返回全集而不是
    /// 最近一个：漏掉任何一个在线客户端都等于让该渠道看不到完成消息。
    fn active_client_presences(
        &self,
        workspace_id: &str,
        now_ms: u64,
    ) -> Result<Vec<ClientPresenceRecord>, ClientDeliveryStoreError>;

    /// 读取 Home 范围内尚未过期的全部客户端 presence。
    fn active_client_presences_all(
        &self,
        now_ms: u64,
    ) -> Result<Vec<ClientPresenceRecord>, ClientDeliveryStoreError>;

    /// 按客户端 ID 读取 presence；不存在时返回 `None`。
    fn client_presence(
        &self,
        client_id: &str,
    ) -> Result<Option<ClientPresenceRecord>, ClientDeliveryStoreError>;

    /// 按通知 ID 读取完成通知；不存在时返回 `None`。
    fn completion_notification(
        &self,
        notification_id: &str,
    ) -> Result<Option<CompletionNotificationRecord>, ClientDeliveryStoreError>;

    /// 幂等写入完成通知；返回是否为首次写入。
    fn upsert_completion_notification(
        &self,
        notification: &CompletionNotificationRecord,
    ) -> Result<bool, ClientDeliveryStoreError>;

    /// 读取最早等待派发的完成通知。
    fn pending_completion_notifications(
        &self,
        limit: u32,
    ) -> Result<Vec<CompletionNotificationRecord>, ClientDeliveryStoreError>;

    /// 读取已过失效截止时间、仍未确认送达的完成通知。
    ///
    /// 这些通知按产品定义「隔天未读自动失效」：调用方必须把它们结算为
    /// `expired` 并静默停止投递，而不是继续补发。
    fn stale_completion_notifications(
        &self,
        now_ms: u64,
        limit: u32,
    ) -> Result<Vec<CompletionNotificationRecord>, ClientDeliveryStoreError>;

    /// 读取仍可投递且未过期的通知，用于刷新当前全部可用渠道。
    ///
    /// 通知级 `delivered` 只表示至少一个渠道已确认，不阻止其他渠道继续消费：
    /// 新上线渠道仍可在失效前补登记，由补偿扫描推进到可投递状态。
    fn deliverable_completion_notifications(
        &self,
        now_ms: u64,
        limit: u32,
    ) -> Result<Vec<CompletionNotificationRecord>, ClientDeliveryStoreError>;

    /// 读取已形成结论但还没有完成通知的 Execution ID。
    ///
    /// 用于 commit 之后、通知落库之前崩溃的补偿扫描：没有这条补偿，Result 会
    /// 永久滞留未读，用户再也收不到完成消息。
    fn unnotified_conclusive_executions(
        &self,
        limit: u32,
    ) -> Result<Vec<String>, ClientDeliveryStoreError>;

    /// 登记通知的全部投递目标；已存在的目标保留既有状态。
    ///
    /// 返回本次新增的目标数量。派发前登记，保证「所有可用渠道都投递」在持久层
    /// 有明确事实，即使进程在派发中途崩溃也不会丢失某个渠道。
    fn upsert_completion_deliveries(
        &self,
        notification_id: &str,
        targets: &[CompletionDeliveryTarget],
        now_ms: u64,
    ) -> Result<u32, ClientDeliveryStoreError>;

    /// 读取一条通知的全部渠道投递记录。
    fn completion_deliveries(
        &self,
        notification_id: &str,
    ) -> Result<Vec<CompletionDeliveryRecord>, ClientDeliveryStoreError>;

    /// 把通知中等待投递的渠道推进到 `dispatched`，表示交付正文已就绪。
    ///
    /// 主 turn 汇总回复产出后调用；返回推进的渠道数量。
    fn dispatch_completion_deliveries(
        &self,
        notification_id: &str,
        now_ms: u64,
    ) -> Result<u32, ClientDeliveryStoreError>;

    /// 读取某渠道已就绪、等待确认送达的收件箱条目。
    ///
    /// 只返回 `dispatched` 且未过失效截止时间的条目；已确认或已失效的条目不再
    /// 重复展示，避免重复通知用户。
    fn dispatched_completion_inbox(
        &self,
        workspace_id: &str,
        channel: &str,
        destination: &str,
        now_ms: u64,
        limit: u32,
    ) -> Result<Vec<ClientInboxEntry>, ClientDeliveryStoreError>;

    /// 按渠道扫描全部目标上已就绪、等待确认送达的收件箱条目。
    ///
    /// 供进程内渠道投递器使用：按渠道读取已就绪条目。
    /// 与 `dispatched_completion_inbox` 使用同一份「已就绪 + 未失效」口径。
    fn dispatched_channel_inbox(
        &self,
        channel: &str,
        now_ms: u64,
        limit: u32,
    ) -> Result<Vec<ClientInboxEntry>, ClientDeliveryStoreError>;

    /// 按渠道和明确目标读取已就绪的收件箱条目。
    ///
    /// 渠道消费者应优先使用此查询，避免其他目标占满批次后再在内存中过滤，
    /// 从而保证目标切换后的消息可以在固定批次内被处理。
    fn dispatched_channel_inbox_for_destination(
        &self,
        channel: &str,
        destination: &str,
        now_ms: u64,
        limit: u32,
    ) -> Result<Vec<ClientInboxEntry>, ClientDeliveryStoreError>;

    /// 结算单个渠道的投递状态；返回是否确实从 `dispatched` 转为目标状态。
    fn settle_completion_delivery(
        &self,
        notification_id: &str,
        channel: &str,
        destination: &str,
        state: NotificationDeliveryState,
        now_ms: u64,
    ) -> Result<bool, ClientDeliveryStoreError>;

    /// 通知整体失效时，把所有尚未进入终态的渠道投递一并置为 `expired`。
    fn expire_completion_deliveries(
        &self,
        notification_id: &str,
        now_ms: u64,
    ) -> Result<u32, ClientDeliveryStoreError>;

    /// 回写主 turn 的汇总回复正文。
    ///
    /// 只有主 turn 汇总回复允许成为渠道交付内容；该写入必须先于渠道派发完成，
    /// 否则渠道拿到的会是后台 Execution 的原始结论。
    fn record_completion_summary(
        &self,
        notification_id: &str,
        summary: &str,
        now_ms: u64,
    ) -> Result<(), ClientDeliveryStoreError>;

    /// 结算通知级状态。
    /// 通知级状态为 Delivered 只表示至少一个渠道已确认；其他渠道的 delivery
    /// 仍可继续消费，直至各自确认或通知失效。
    fn settle_completion_notification(
        &self,
        notification_id: &str,
        state: NotificationDeliveryState,
        now_ms: u64,
    ) -> Result<(), ClientDeliveryStoreError>;

    /// 读取「正文已就绪、但还没有任何渠道目标」的通知。
    ///
    /// 登记投递项时可能没有任何可用渠道（客户端离线且外部渠道不可用）。这类条目
    /// 必须保留在收件箱，等渠道出现后补登记并投递；丢弃它们等同于静默丢失。
    fn notifications_awaiting_channel_delivery(
        &self,
        now_ms: u64,
        limit: u32,
    ) -> Result<Vec<CompletionNotificationRecord>, ClientDeliveryStoreError>;

    /// 读取「汇总正文已就绪、但仍有渠道停留在 `pending`」的通知 ID。
    ///
    /// 覆盖两类必须补偿的窗口：主 turn 汇总已落库但渠道推进中断，以及通知结算后
    /// 才补登记的渠道。没有这条扫描，这些渠道会永久停在 `pending`，用户再也收不到
    /// 完成消息，通知本身也无法进入可清理的终态。
    fn notification_ids_with_pending_deliveries(
        &self,
        now_ms: u64,
        limit: u32,
    ) -> Result<Vec<String>, ClientDeliveryStoreError>;
}
