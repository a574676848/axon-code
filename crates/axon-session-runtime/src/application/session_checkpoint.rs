//! Session checkpoint 的应用模型。

/// 一次可恢复 checkpoint 的完整元数据。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionCheckpoint {
    /// checkpoint 序号。
    pub sequence: u64,
    /// 已提交 Action ID。
    pub action_id: String,
    /// Agent context 版本。
    pub context_version: u64,
    /// 已确认的 tool receipt 序号。
    pub tool_receipt_sequence: u64,
    /// Goal revision。
    pub goal_revision: Option<u64>,
    /// 已消耗 token 数。
    pub token_usage: Option<u64>,
    /// 本次 turn 耗时。
    pub time_usage_ms: Option<u64>,
    /// Agent tree 摘要。
    pub agent_tree_summary: Option<String>,
    /// transcript/rollout 外部引用。
    pub transcript_ref: Option<String>,
}
