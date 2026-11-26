//! 收口回投落点：把「已结算却没被取走」的追加交回 durable 入站表重新排队。
//!
//! 单独成文件有两个理由：`daemon_host_runner` 已顶到单文件行数上限；这条路径决定消息
//! 会不会丢，装配点与用例必须共用同一份实现，不允许测试自己复制一份闭包。

use std::sync::Arc;

use axon_domain::InteractionId;
use axon_session::{InteractionStore as _, RuntimeStore as _, SqliteRuntimeStore};

use crate::run_commands::run_steer::SteerRequeueHook;

/// 装配回投落点：每次调用落一条独立写事务，失败只返回可读原因，由调用方决定告警。
pub(crate) fn store_requeue_hook(store: SqliteRuntimeStore) -> SteerRequeueHook {
    Arc::new(move |interaction_id: &InteractionId, now_ms: u64| {
        store
            .requeue_steered(interaction_id, now_ms)
            .map_err(|error| error.to_string())
    })
}

/// 重启续跑：把上一个进程留下的在途执行排回队列。
///
/// 必须抢在后台 Worker 第一次 supervisor tick 之前调用：一旦
/// `quarantine_interrupted_executions` 跑过，这些执行会被收敛成 uncertain 并落一条终态
/// 结果，之后再重排就会撞结果主键。重排后调度器自行领取，run 用 Thread 里已经落库的
/// 工具回执接着跑，用户不必手动点「继续」。
pub(crate) fn requeue_running_after_restart(store: &SqliteRuntimeStore, now_ms: u64) {
    match store.requeue_stale_running(now_ms) {
        Ok(0) => {}
        Ok(requeued) => axon_logger::warning(&format!(
            "Daemon 重启续跑：已把 {requeued} 条在途执行排回队列"
        )),
        Err(error) => axon_logger::warning(&format!("Daemon 重启续跑重排失败：{error}")),
    }
}
