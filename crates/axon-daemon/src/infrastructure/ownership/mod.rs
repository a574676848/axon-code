//! Host ownership 实现边界。

/// lease fencing 结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FencingDecision {
    /// generation 校验通过。
    Accept,
    /// generation 校验失败。
    Reject,
}
