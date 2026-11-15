//! 孤儿轮次对账：把「执行者已经失去、轮次却仍挂 Active」的会话改判为终态。
//!
//! 判据完全复用存储层的失效定义（宿主记录缺失、代际落后、宿主租约过期），与调度器
//! 的接管判据同源，因此不会出现「调度器认为该接管、对账认为还活着」的两套结论。
//!
//! 这一层只做一件事：找出没有人推进的 Active 轮次并写终态。它不接管会话、不重新
//! 调度、不改执行权归属，那些是调度器的职责。

use std::path::Path;

use axon_session::{
    InterruptTurnCommand, JsonlThreadEventStore, RuntimeStore, SqliteRuntimeStore, ThreadService,
    TurnStatus,
};

use super::run_execution::now_ms;
use super::run_thread_store::load_thread_detail;

/// 单轮最多对账的会话数，避免一次启动把整库扫一遍。
pub(crate) const TURN_RECONCILE_BATCH_LIMIT: u32 = 64;

/// 一次对账的结果。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TurnReconcileReport {
    /// 检出为孤儿的会话数。
    pub examined: usize,
    /// 实际写入了中断终态的轮次数。
    pub interrupted: usize,
}

/// 启动对账：守护进程起来后立刻对账一次，结果打进日志。
///
/// 根目录解析失败只告警不阻断启动：对账是自愈手段，不该成为启动的前置条件。
pub(crate) fn reconcile_at_startup(store: &SqliteRuntimeStore, config_path: &Path) {
    let Ok(root) = axon_config::runtime_axon_root_path(config_path) else {
        axon_logger::warning("启动对账跳过：运行时根目录解析失败");
        return;
    };
    match reconcile_orphan_turns(store, &root, TURN_RECONCILE_BATCH_LIMIT) {
        Ok(report) if report.interrupted > 0 => axon_logger::warning(&format!(
            "启动对账：检出 {} 个无推进者的会话，其中 {} 个轮次已改判为中断",
            report.examined, report.interrupted
        )),
        Ok(_) => {}
        Err(error) => axon_logger::warning(&format!("启动对账失败: {error}")),
    }
}

/// 对账一次：把 owner 已失效且最后一轮仍是 Active 的会话改判为中断。
///
/// 幂等：改判成功后该轮次不再是 Active，下一次对账自然不再命中。
pub(crate) fn reconcile_orphan_turns(
    store: &SqliteRuntimeStore,
    axon_root: &Path,
    limit: u32,
) -> Result<TurnReconcileReport, String> {
    let now = now_ms();
    let orphans = store
        .list_orphaned_sessions(now, limit)
        .map_err(|error| format!("列出孤儿会话失败：{error:?}"))?;
    let mut report = TurnReconcileReport {
        examined: orphans.len(),
        interrupted: 0,
    };
    if orphans.is_empty() {
        return Ok(report);
    }
    let service = ThreadService::new(JsonlThreadEventStore);
    for runtime in orphans {
        let session_id = runtime.session_id.as_str().to_owned();
        let Ok(detail) = load_thread_detail(axon_root, &session_id) else {
            continue;
        };
        let Some(turn) = detail.turns.last() else {
            continue;
        };
        if turn.status != TurnStatus::Active {
            continue;
        }
        let command = InterruptTurnCommand {
            thread_id: detail.thread.thread_id.clone(),
            turn_id: turn.turn_id.as_str().to_owned(),
            recorded_at_ms: now,
        };
        match service.interrupt_turn(axon_root, command) {
            Ok(()) => {
                report.interrupted = report.interrupted.saturating_add(1);
                axon_logger::warning(&format!(
                    "孤儿轮次已改判为中断：session={session_id}，宿主记录缺失或租约已过期，本轮已无推进者"
                ));
            }
            Err(error) => axon_logger::warning(&format!(
                "孤儿轮次改判失败：session={session_id}，error={error}"
            )),
        }
    }
    Ok(report)
}
