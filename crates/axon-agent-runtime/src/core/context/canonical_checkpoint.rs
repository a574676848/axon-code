//! 交接摘要的提交前门禁。
//!
//! 摘要内容由模型自由撰写：运行时只要求非空，格式与段落结构都不限制。
//! 曾经把六段式标题当硬门禁，模型把概述写在 `P0:` 同一行就判「缺少固定字段 P0:」，
//! 而换窗是会话自救的唯一动作，在格式上 fail-closed 等于把会话永久钉死在满上下文。
//! 篇幅由换窗层的预算校验统一把关（`ReplacementValidationError::CheckpointOverBudget`），
//! 超预算只会要求重写或回落投影，不会在解析阶段就断死。

use crate::AgentModelMessage;

/// 已通过门禁的交接摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalCheckpoint {
    summary: String,
}

impl CanonicalCheckpoint {
    /// 校验交接摘要非空。内容格式与篇幅不在这里判定。
    ///
    /// `history` 只为兼容既有调用签名保留：活动 tool call 的一致性不再由摘要文本承担，
    /// 带悬空调用的窗口在重建历史时按自身规则处理，不该由一段自由文本兜底。
    pub fn parse(summary: &str, _history: &[AgentModelMessage]) -> Result<Self, String> {
        let summary = summary.trim();
        if summary.is_empty() {
            return Err("checkpoint 内容为空".to_owned());
        }
        Ok(Self {
            summary: summary.to_owned(),
        })
    }

    /// 返回可写入历史 checkpoint 的原始摘要。
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.summary
    }

    /// 取得已校验的摘要所有权。
    #[must_use]
    pub fn into_inner(self) -> String {
        self.summary
    }
}
