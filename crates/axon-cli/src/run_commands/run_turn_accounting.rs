//! 一轮对话的终态记账：指标、逐字稿投影与会话标题的唯一入口。
//!
//! 所有来源共用这一处，差异只允许由 `TurnProfile` 表达。任何入口都不得自行再写一份，
//! 否则同一轮会散出两条互相矛盾的事实：指标落一条会话、正文落另一条会话。
//!
//! 轮后治理不在这里：它需要拥有整轮请求与记录去起后台作业，由调用方按
//! `profile.accounting.governance` 决定是否启动。

use std::path::Path;

use axon_session_runtime::TurnProfile;

use super::RunRequest;
use super::run_session_store::RunSessionRecord;
use super::run_thread_store::mirror_session_to_thread;

/// 收口一轮已完成的对话。
///
/// 返回本轮的会话标题：标题派生是纯字符串处理，即使不投影也照样返回给调用方，
/// 避免调用方为了拿标题再走一遍派生逻辑。
pub(super) fn account_completed_turn(
    config_path: &Path,
    request: &RunRequest,
    record: &RunSessionRecord,
    profile: TurnProfile,
) -> String {
    // 指标与来源无关：缺行是投影缺口，不是合法差异。
    if profile.accounting.metrics {
        super::publish_turn_metrics(config_path, record);
    }
    if let Err(error) = mirror_session_to_thread(config_path, record) {
        axon_logger::warning(&format!(
            "thread projection skipped: session_id={}, error={error}",
            record.session_id
        ));
    }
    let session_title = super::run_thread_title::derive_thread_title(&record.prompt);
    if let Err(error) =
        super::run_thread_title::mirror_thread_title(config_path, record, &session_title)
    {
        axon_logger::warning(&format!(
            "thread title projection skipped: session_id={}, error={error}",
            record.session_id
        ));
    }
    // 标题生成是主 turn 的决策权：派生 turn 的标题由来源会话决定，不再另起一次生成。
    if profile.accounting.title {
        super::run_session_title::maybe_spawn_session_title_generation(
            config_path,
            &record.session_id,
            &request.cwd,
            request.current_agent_path.is_root(),
        );
    }
    session_title
}
