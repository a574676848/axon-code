use axon_domain::{InteractionId, InteractionRecord};
use std::fmt;

/// Interaction 持久化端口错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InteractionStoreError {
    /// 记录已存在但内容或幂等键冲突。
    Conflict(String),
    /// 目标记录不存在。
    NotFound(InteractionId),
    /// lease 或状态条件不满足。
    InvalidState(String),
    /// 基础设施持久化失败。
    Storage(String),
}

impl fmt::Display for InteractionStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflict(detail) => write!(formatter, "Interaction 冲突：{detail}"),
            Self::NotFound(interaction_id) => {
                write!(formatter, "Interaction 不存在：{}", interaction_id.as_str())
            }
            Self::InvalidState(detail) => write!(formatter, "Interaction 状态无效：{detail}"),
            Self::Storage(detail) => write!(formatter, "Interaction 存储失败：{detail}"),
        }
    }
}

impl std::error::Error for InteractionStoreError {}

/// Interaction append 的稳定回执。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteractionAppendReceipt {
    /// 持久化的 Interaction 标识。
    pub interaction_id: InteractionId,
    /// 是否新写入记录。
    pub inserted: bool,
}

/// 被 Daemon 主 turn 领取的 processing lease。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteractionProcessingLease {
    /// 被领取的 Interaction。
    pub interaction: InteractionRecord,
    /// 本次领取的 fencing token。
    pub lease_token: String,
    /// lease 到期时间（毫秒）。
    pub lease_expires_at_ms: u64,
}

/// Interaction 持久化端口。
pub trait InteractionStore: Send + Sync {
    /// 幂等追加 Interaction；重复来源应返回 inserted=false。
    fn append(
        &self,
        interaction: &InteractionRecord,
    ) -> Result<InteractionAppendReceipt, InteractionStoreError>;

    /// 领取待处理 Interaction，并建立有界 lease。
    ///
    /// 领取按复合 lane 排他：某条 lane 已有未过期 `processing` 时，该 lane 的后续记录
    /// 不会被领取。同 lane 串行由持久化事实源保证，调用方无需额外维护在途 lane 状态。
    fn claim_processing(
        &self,
        limit: u32,
        now_ms: u64,
        lease_duration_ms: u64,
    ) -> Result<Vec<InteractionProcessingLease>, InteractionStoreError>;

    /// 按 ID 领取单条 Interaction，已被其他消费者持有时返回 None。
    fn claim_processing_by_id(
        &self,
        interaction_id: &InteractionId,
        now_ms: u64,
        lease_duration_ms: u64,
    ) -> Result<Option<InteractionProcessingLease>, InteractionStoreError>;

    /// 使用当前 fencing token 续租 processing Interaction。
    fn renew_processing(
        &self,
        interaction_id: &InteractionId,
        lease_token: &str,
        now_ms: u64,
        lease_duration_ms: u64,
    ) -> Result<u64, InteractionStoreError>;

    /// 使用当前 fencing token 释放 processing lease，并恢复为 received。
    fn release_processing(
        &self,
        interaction_id: &InteractionId,
        lease_token: &str,
        now_ms: u64,
    ) -> Result<(), InteractionStoreError>;

    /// 提交 understood 状态并释放 lease。
    fn mark_understood(
        &self,
        interaction_id: &InteractionId,
        lease_token: &str,
        now_ms: u64,
    ) -> Result<(), InteractionStoreError>;

    /// 提交 rejected 状态并释放 lease。
    fn mark_rejected(
        &self,
        interaction_id: &InteractionId,
        lease_token: &str,
        now_ms: u64,
    ) -> Result<(), InteractionStoreError>;

    /// 提交 interrupted 状态并释放 lease。
    ///
    /// 用户主动中断与执行失败是两种终态：前者是「按用户意愿停止」，后者是「没做成」。
    /// 混用 rejected 会让重试、统计与界面提示都失去依据。
    fn mark_interrupted(
        &self,
        interaction_id: &InteractionId,
        lease_token: &str,
        now_ms: u64,
    ) -> Result<(), InteractionStoreError>;
}

/// Interaction 状态查询端口。
pub trait InteractionQueryStore: Send + Sync {
    /// 按稳定 ID 读取 Interaction；不存在时返回 None。
    fn get_interaction(
        &self,
        interaction_id: &InteractionId,
    ) -> Result<Option<InteractionRecord>, InteractionStoreError>;

    /// 读取该 Workspace 最近一条来自外部渠道的 Interaction。
    ///
    /// 完成通知在没有在线客户端时按此结果回落到「最近使用的渠道」；系统自身
    /// 派发的 Interaction 渠道不参与回落，避免投递目标自我循环。
    fn latest_external_interaction(
        &self,
        workspace_id: &str,
    ) -> Result<Option<InteractionRecord>, InteractionStoreError>;

    /// 读取该 Workspace 最近一条仍在 Processing 的外部 Interaction。
    ///
    /// 该查询用于渠道重连恢复，必须按状态过滤，不能用最近记录近似判断。
    fn latest_processing_external_interaction(
        &self,
        workspace_id: &str,
    ) -> Result<Option<InteractionRecord>, InteractionStoreError>;

    /// 读取该 Workspace 内指定渠道最近一条来自外部渠道的 Interaction。
    fn latest_external_interaction_for_channel(
        &self,
        workspace_id: &str,
        channel: &str,
    ) -> Result<Option<InteractionRecord>, InteractionStoreError>;

    /// 读取该 Workspace 内指定渠道最近一条仍在 Processing 的外部 Interaction。
    fn latest_processing_interaction_for_channel(
        &self,
        workspace_id: &str,
        channel: &str,
    ) -> Result<Option<InteractionRecord>, InteractionStoreError>;

    /// 读取同一 Workspace 内指定会话表面最近一条仍在 Processing 的外部 Interaction。
    fn latest_processing_interaction_for_route(
        &self,
        workspace_id: &str,
        channel: &str,
        destination: &str,
    ) -> Result<Option<InteractionRecord>, InteractionStoreError>;

    /// 读取 Home 范围内最近一条来自外部渠道的 Interaction。
    fn latest_external_interaction_any(
        &self,
    ) -> Result<Option<InteractionRecord>, InteractionStoreError>;

    /// 读取指定车道下最近的 Interaction，按接收时间升序返回。
    ///
    /// 车道是 Interaction 的复合归属，回放一条会话需要按真实先后顺序逐条恢复，
    /// 因此返回值固定升序；`limit` 截取的是「最近 N 条」而不是「最早 N 条」，
    /// 从而把读放大限制在有界窗口内。
    ///
    /// `before` 为空即取最近一页；给出游标则取它**之前**的那一页，用于按需向前翻。
    fn interactions_for_lane(
        &self,
        lane_id: &str,
        limit: u32,
        before: Option<&InteractionCursor>,
    ) -> Result<Vec<InteractionRecord>, InteractionStoreError>;
}

/// 车道内 Interaction 的反向分页游标。
///
/// 游标取「已加载窗口里最早那一条」的排序键，向前翻页时排除它及其之后的记录。
/// 排序键与查询的 `ORDER BY` 完全一致（`received_at_ms` 加 `interaction_id`），
/// 因此同一毫秒内落库的多条记录既不会被跳过，也不会被重复读出。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteractionCursor {
    /// 排序主键：接收时间。
    pub received_at_ms: u64,
    /// 同一毫秒内的次序键。
    pub interaction_id: String,
}

impl InteractionCursor {
    /// 取一条记录的排序键作为游标。
    #[must_use]
    pub fn of(record: &InteractionRecord) -> Self {
        Self {
            received_at_ms: record.received_at_ms,
            interaction_id: record.interaction_id.as_str().to_owned(),
        }
    }
}
