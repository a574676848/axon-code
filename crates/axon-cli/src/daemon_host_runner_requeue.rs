//! 收口回投落点：把「已结算却没被取走」的追加交回 durable 入站表重新排队。
//!
//! 单独成文件有两个理由：`daemon_host_runner` 已顶到单文件行数上限；这条路径决定消息
//! 会不会丢，装配点与用例必须共用同一份实现，不允许测试自己复制一份闭包。

use std::sync::Arc;

use axon_domain::InteractionId;
use axon_session::{InteractionStore as _, SqliteRuntimeStore};

use crate::run_commands::run_steer::SteerRequeueHook;

/// 装配回投落点：每次调用落一条独立写事务，失败只返回可读原因，由调用方决定告警。
pub(crate) fn store_requeue_hook(store: SqliteRuntimeStore) -> SteerRequeueHook {
    Arc::new(move |interaction_id: &InteractionId, now_ms: u64| {
        store
            .requeue_steered(interaction_id, now_ms)
            .map_err(|error| error.to_string())
    })
}
