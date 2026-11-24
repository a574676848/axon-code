//! 换窗时注入新窗口的上下文装配测试辅助。
//!
//! 摘要本身由调用方（持有本轮工具回执的一侧）生成并通过
//! `WindowRolloverPolicyRuntime::with_digest_source` 注入；本模块只保留装配测试用的
//! 单次重建入口。

#[cfg(test)]
use crate::{AgentModelMessage, CanonicalCheckpoint, CompactedHistory};

/// 运行时的 Debug 视图：只暴露策略与是否挂上摘要来源，闭包本身不可打印。
impl std::fmt::Debug for super::WindowRolloverPolicyRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WindowRolloverPolicyRuntime")
            .field("policy", &self.policy)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
pub(crate) fn rebuild_with_digest_for_test(
    messages: &[AgentModelMessage],
    checkpoint: &CanonicalCheckpoint,
    digest: Option<&str>,
) -> CompactedHistory {
    super::history::build_fresh_window_history_from_normalized(messages, checkpoint, None, digest)
}
