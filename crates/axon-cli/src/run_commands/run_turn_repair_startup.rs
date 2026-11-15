//! 启动时的 Thread 侧收口。
//!
//! 进程级中断收敛只覆盖 Interaction 与 Execution，Thread 的事件流不会被它改判，
//! 崩溃遗留的 `Active` 尾轮会永久挡住该会话的下一轮与空闲压缩。这里按「刚被判定
//! 执行者失效」的会话逐条收口：判据只看尾部是否 `Active`，不看时间。

use std::path::Path;

use axon_session::{
    JsonlThreadEventStore, RepairOpenTurnCommand, SessionRuntimeRecord, ThreadService,
    TurnRepairOutcome,
};

use super::run_execution::now_ms;

/// 对一批执行者已失效的会话做 Thread 侧收口，返回实际收口的轮次数。
///
/// 单个会话修不好只告警：收口是自愈手段，不该因为一条历史异常拖住守护进程启动。
pub(crate) fn repair_orphaned_threads_startup(
    axon_root: &Path,
    sessions: &[SessionRuntimeRecord],
) -> usize {
    if sessions.is_empty() {
        return 0;
    }
    let service = ThreadService::new(JsonlThreadEventStore);
    let mut repaired = 0_usize;
    for runtime in sessions {
        let thread_id = runtime.run_session_id.clone();
        let Ok(activation) = service.read_thread_activation(axon_root, &thread_id) else {
            continue;
        };
        let Some(turn_id) = activation.active_turn_id else {
            continue;
        };
        let command = RepairOpenTurnCommand {
            thread_id: thread_id.clone(),
            turn_id,
            recorded_at_ms: now_ms(),
        };
        match service.repair_open_turn(axon_root, command) {
            Ok(TurnRepairOutcome::Repaired {
                turn_id,
                synthetic_results,
            }) => {
                repaired = repaired.saturating_add(1);
                axon_logger::warning(&format!(
                    "崩溃遗留轮次已收口：thread={thread_id}, turn={turn_id}, 补合成工具结果={synthetic_results}"
                ));
            }
            Ok(TurnRepairOutcome::Balanced) => {}
            Err(error) => axon_logger::warning(&format!(
                "崩溃遗留轮次收口失败：thread={thread_id}, error={error}"
            )),
        }
    }
    repaired
}
