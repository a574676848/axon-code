//! 会话级上下文快照：界面上所有「上下文占用」读数的唯一来源。
//!
//! 快照只由用量帧投影而来：用量帧给出当前占用、窗口与剩余量。投影是派生数据，缺行表示
//! 「没有记录」，字段为空表示「未知」；两者都不允许用旧值顶替（INV-6）：界面拿不到读数时
//! 只能显示未知，不能沿用上一份读数冒充现在。

/// 会话级上下文快照。
///
/// 字段全部可空是刻意的：帧里没有的值必须留空，读侧据此显示未知，而不是拿上一次的
/// 数字继续显示。`updated_at_ms` 与 `source` 一起回答「这份读数出自哪一类帧、什么时候」，
/// 界面据此显示「更新于 N 秒前」并在来源变化时刷新判据。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ContextSnapshot {
    /// 当前上下文占用 token，来自用量帧的 `currentContextTokens`（Provider 实测）。
    pub current_context_tokens: Option<u64>,
    /// 模型上下文窗口；帧里带有时才有值。
    pub context_window: Option<u64>,
    /// 上下文构成估算（系统提示词、工具定义、对话消息三类 token）。
    pub composition: Option<serde_json::Value>,
    /// 该快照最后一次被写入的时刻（Unix 毫秒）。
    pub updated_at_ms: Option<u64>,
    /// 写入这份读数的帧类别；目前只有 `usage_frame`。
    pub source: Option<String>,
}
