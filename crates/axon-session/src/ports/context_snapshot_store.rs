//! 会话级上下文快照的只读端口。
//!
//! 与 [`super::RuntimeStore`] 的分工：写入只发生在事件追加的唯一投影入口
//! （`apply_projections`），本端口只回答「这条会话现在的快照是什么」。读侧因此没有
//! 第二条写路径，也就不存在两处各自维护读数的问题。

use super::RuntimeStoreError;
use crate::domain::ContextSnapshot;
use axon_domain::DaemonSessionId;

/// 会话级上下文快照的读取端口。
pub trait ContextSnapshotStore: Send + Sync {
    /// 读该会话的上下文快照。
    ///
    /// 没有记录时返回 `None`，与「有记录但字段未知」区分开：前者说明这条会话还没出现过
    /// 任何可用的上下文读数（例如从未跑过一轮），后者说明读到过帧但帧里没有该字段。
    /// 两种情况的文案不同，读侧不能把它们合并成同一个空结构体。
    fn context_snapshot(
        &self,
        session_id: &DaemonSessionId,
    ) -> Result<Option<ContextSnapshot>, RuntimeStoreError>;
}
