//! 用户问答交互持久化端口定义。

use axon_domain::{UserQuestionRecord, UserQuestionState};
use thiserror::Error;

/// 用户问答存储错误。
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum UserQuestionStoreError {
    /// 目标问题未找到。
    #[error("问题未找到: {0}")]
    NotFound(String),
    /// 问题已经结算（已作答或已取消/过期）。
    #[error("问题已结算: {0}")]
    AlreadySettled(String),
    /// 唯一键冲突或状态冲突。
    #[error("问题数据冲突: {0}")]
    Conflict(String),
    /// 底层数据库错误。
    #[error("数据库存储错误: {0}")]
    Storage(String),
    /// 序列化或反序列化失败。
    #[error("序列化错误: {0}")]
    Serialization(String),
}

/// 用户问答持久化端口。
pub trait UserQuestionStore: Send + Sync {
    /// 插入提问记录；若 idempotency_key 已存在则幂等返回已有记录。
    fn insert_question(
        &self,
        record: &UserQuestionRecord,
    ) -> Result<UserQuestionRecord, UserQuestionStoreError>;

    /// 按全局 question_id 查询记录。
    fn get_question_by_id(
        &self,
        question_id: &str,
    ) -> Result<Option<UserQuestionRecord>, UserQuestionStoreError>;

    /// 按 tool_call_id 查询记录。
    fn get_question_by_call_id(
        &self,
        call_id: &str,
    ) -> Result<Option<UserQuestionRecord>, UserQuestionStoreError>;

    /// 按车道查询提问列表。
    fn list_questions_by_lane(
        &self,
        lane_id: &str,
        state: Option<UserQuestionState>,
        limit: usize,
    ) -> Result<Vec<UserQuestionRecord>, UserQuestionStoreError>;

    /// CAS 结算作答：仅当问题处于 pending 或 suspended 状态时生效。
    /// 结算成功后将状态置为 answered，写入 answer_json、answered_at_ms、channel、delivery_mode，delivered 保持 0。
    fn cas_settle_answer(
        &self,
        call_id: &str,
        answer_json: &str,
        answered_at_ms: i64,
        channel: Option<&str>,
        delivery_mode: Option<&str>,
    ) -> Result<UserQuestionRecord, UserQuestionStoreError>;

    /// CAS 状态推进（如 Pending -> Suspended，或刷新 deadline）。
    fn cas_update_state(
        &self,
        question_id: &str,
        from_state: UserQuestionState,
        to_state: UserQuestionState,
        deadline_at_ms: Option<i64>,
        handoff_text: Option<&str>,
    ) -> Result<bool, UserQuestionStoreError>;

    /// 查询已作答但尚未投递的记录（用于对账补投）。
    fn list_unreconciled_answered(
        &self,
        limit: usize,
    ) -> Result<Vec<UserQuestionRecord>, UserQuestionStoreError>;

    /// 标记答案已完成投递（delivered = 1）并写入投递方式。
    ///
    /// 送达标记与投递方式描述同一次投递事实，必须在同一条 UPDATE 内写入：
    /// 拆成两条语句会留下「已送达却没有投递方式」的中间态。
    fn mark_delivered(
        &self,
        question_id: &str,
        delivery_mode: &str,
    ) -> Result<bool, UserQuestionStoreError>;

    /// 把越过自身过期时刻、仍未作答的提问标记为 `expired`。
    ///
    /// 只推进未决状态（pending / suspended）并保留记录本身：过期是可审计状态，
    /// 行级删除由运行态保留窗口的清理族另行完成。
    fn mark_expired_questions(
        &self,
        now_ms: i64,
        limit: usize,
    ) -> Result<usize, UserQuestionStoreError>;
}
