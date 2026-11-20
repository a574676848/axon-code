//! 轮次事实侧车：把打开会话要读的三个标量按轮落一行。
//!
//! 性能取向：每轮只在收口里多写一条小行（一次单行 upsert，不读 thread、不序列化整条
//! 记录），把「读模型别名/强度/反思提示」从读大记录降到一次主键查询。表是投影，写入
//! 失败只告警，读侧缺行时仍回落尾窗读。

use std::path::Path;

use axon_session::ThreadTurnFacts;

use super::run_session_store::RunSessionRecord;
use super::run_thread_store::thread_turn_metadata_from_record;

/// 控制面库是否已经存在。
///
/// 只做既存库的投影：库里没有这张表就跳过，绝不为了写侧车而新建一个库。
/// 这样只跑 thread 的路径（测试、只读工具）不会因为一次收口凭空多出一个库文件，
/// 也不会把库句柄留在临时目录里挡住清理。
fn control_store_ready(config_path: &Path) -> bool {
    crate::runtime_paths::runtime_axon_data_path_static(config_path)
        .map(|data_root| data_root.join("control.sqlite3").exists())
        .unwrap_or(false)
}

/// 收口时写入本轮标量事实。
pub(crate) fn record_turn_facts(config_path: &Path, record: &RunSessionRecord) {
    if !control_store_ready(config_path) {
        return;
    }
    let Ok(store) = super::run_turn_metrics::shared_store(config_path) else {
        return;
    };
    let metadata = thread_turn_metadata_from_record(record);
    // 没有 assistant part 标识就没有可匹配的轮次：宁可不写，也不留一行无法归属的事实。
    if metadata.assistant_part_id.is_empty() {
        return;
    }
    let facts = ThreadTurnFacts {
        thread_id: record.thread_id.clone(),
        assistant_part_id: metadata.assistant_part_id,
        recorded_at_ms: record.created_at_ms,
        model_name: metadata.model_name,
        reasoning_effort: metadata.reasoning_effort,
        reflection_notice: metadata.reflection_notice,
    };
    if let Err(error) = store.upsert_thread_turn_facts(&facts, axon_time::unix_now_ms()) {
        axon_logger::warning(&format!("轮次事实侧车写入失败: {error}"));
    }
}

/// 治理回写时补写反思提示：只改一个字段，行不存在则什么都不做。
pub(crate) fn record_turn_notice(
    config_path: &Path,
    thread_id: &str,
    assistant_part_id: &str,
    notice: &str,
) {
    if !control_store_ready(config_path) {
        return;
    }
    let Ok(store) = super::run_turn_metrics::shared_store(config_path) else {
        return;
    };
    if let Err(error) = store.update_thread_turn_notice(
        thread_id,
        assistant_part_id,
        notice,
        axon_time::unix_now_ms(),
    ) {
        axon_logger::warning(&format!("轮次事实侧车补写提示失败: {error}"));
    }
}
