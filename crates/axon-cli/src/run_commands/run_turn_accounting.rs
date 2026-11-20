//! 一轮对话的终态记账：指标、逐字稿投影与会话标题的唯一入口。
//!
//! 所有来源共用这一处，差异只允许由 `TurnProfile` 表达。任何入口都不得自行再写一份，
//! 否则同一轮会散出两条互相矛盾的事实：指标落一条会话、正文落另一条会话。
//!
//! 轮后治理不在这里：它需要拥有整轮请求与记录去起后台作业，由调用方按
//! `profile.accounting.governance` 决定是否启动。

use std::path::Path;
use std::thread::sleep;
use std::time::Duration;

use axon_session_runtime::TurnProfile;

use super::RunRequest;
use super::run_session_store::RunSessionRecord;
use super::run_thread_store::mirror_session_to_thread;

/// 投影失败后的退避：先 50ms 再 200ms，共三次尝试。
///
/// 收口读失败的常见原因是瞬时占用（杀软扫描、另一个句柄、并发重写），退避一次就能过；
/// 三次都失败才说明真的读不出来，那时必须把错误交回调用方，不能只留一条告警。
const PROJECTION_RETRY_BACKOFF_MS: [u64; 2] = [50, 200];

/// 收口一轮已完成的对话。
///
/// 返回本轮的会话标题：标题派生是纯字符串处理，即使不投影也照样返回给调用方，
/// 避免调用方为了拿标题再走一遍派生逻辑。投影失败时返回错误：一轮的正文只存在于
/// thread 里，投影失败等于这一轮在界面上消失，必须让调用方看见。
pub(super) fn account_completed_turn(
    config_path: &Path,
    request: &RunRequest,
    record: &RunSessionRecord,
    profile: TurnProfile,
) -> Result<String, String> {
    // 指标与来源无关：缺行是投影缺口，不是合法差异。
    if profile.accounting.metrics {
        super::publish_turn_metrics(config_path, record);
    }
    let projection = project_turn_with_retry(config_path, record);
    let session_title = super::run_thread_title::derive_thread_title(&record.prompt);
    // 标题投影与正文投影是两笔独立事实：正文写不进去时标题仍按原口径尽力落盘。
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
    projection.map(|()| session_title)
}

/// 把本轮投影进 thread，瞬时失败带退避重试；用尽后把最后一次错误原样交回。
fn project_turn_with_retry(config_path: &Path, record: &RunSessionRecord) -> Result<(), String> {
    let mut retried = 0_usize;
    loop {
        let error = match mirror_session_to_thread(config_path, record) {
            Ok(_) => return Ok(()),
            Err(error) => error,
        };
        let Some(backoff_ms) = PROJECTION_RETRY_BACKOFF_MS.get(retried) else {
            axon_logger::error(&format!(
                "thread projection failed: session_id={}, thread_id={}, part={}, error={error}",
                record.session_id, record.thread_id, record.assistant_part_id
            ));
            return Err(format!(
                "本轮逐字稿投影失败（session={}, thread={}）：{error}",
                record.session_id, record.thread_id
            ));
        };
        retried += 1;
        sleep(Duration::from_millis(*backoff_ms));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_root(label: &str) -> PathBuf {
        axon_test_support::test_temp_root().join(format!(
            "axon-turn-accounting-{label}-{}-{}",
            std::process::id(),
            axon_time::unix_now_ms()
        ))
    }

    fn session_record(thread_id: &str) -> RunSessionRecord {
        RunSessionRecord {
            session_id: "session-1".to_owned(),
            thread_id: thread_id.to_owned(),
            parent_session_id: None,
            parent_thread_id: None,
            interaction_id: String::new(),
            created_at_ms: 1,
            startup_timeline: crate::run_commands::run_startup_snapshot::test_startup_timeline(),
            cwd: Some("fixture".to_owned()),
            provider: "provider-a".to_owned(),
            model: "model-a".to_owned(),
            model_name: Some("model-a".to_owned()),
            selected_skill_name: None,
            repo_context: None,
            session_context: None,
            durable_memory: None,
            prompt: "收口一轮".to_owned(),
            local_images: Vec::new(),
            local_files: Vec::new(),
            response: "已经答完".to_owned(),
            assistant_part_id: "assistant:session-1:part".to_owned(),
            usage: None,
            tool_receipts: Vec::new(),
            tool_calls: Vec::new(),
            reasoning_messages: Vec::new(),
            turn_events: Vec::new(),
            failure: None,
            upstream_model: None,
            reasoning_effort: None,
            request_path: None,
            reflection_notice: None,
            turn_trace: None,
            origin_channel: None,
            speed_mode: None,
            ttft_ms: None,
            generation_speed_tps: None,
            injected_context_messages: Vec::new(),
        }
    }

    /// thread 事件文件的逻辑路径：文件名是 thread 标识的十六进制编码。
    fn thread_logical_path(root: &Path, thread_id: &str) -> PathBuf {
        let mut prefix = String::new();
        for byte in thread_id.as_bytes() {
            prefix.push_str(&format!("{byte:02x}"));
        }
        axon_session::JsonlThreadEventStore::path(root).join(format!("{prefix}.jsonl"))
    }

    #[test]
    fn readable_turn_is_projected() {
        let root = temp_root("ok");
        let config_path = root.join("config.toml");
        let record = session_record("lane-ok");

        project_turn_with_retry(&config_path, &record).expect("可写的 thread 必须投影成功");

        let detail = axon_session::ThreadService::new(axon_session::JsonlThreadEventStore)
            .read_thread(&root, "lane-ok")
            .expect("投影后必须读得回这一轮");
        assert_eq!(detail.turns.len(), 1);
        let _ = std::fs::remove_dir_all(root);
    }

    /// 条目 10③：读不出 thread 时收口必须以失败对外可见，绝不静默丢轮。
    #[test]
    fn unreadable_thread_fails_the_turn_projection_visibly() {
        let root = temp_root("torn");
        let config_path = root.join("config.toml");
        let record = session_record("lane-torn");
        // 造一个清不掉的崩溃现场：零分片 + 用目录占住重写临时文件名。
        let temporary = thread_logical_path(&root, "lane-torn").with_extension("rewrite0.tmp");
        std::fs::create_dir_all(&temporary).expect("造崩溃现场");

        let error = project_turn_with_retry(&config_path, &record)
            .expect_err("读不出来的 thread 必须让收口失败");

        assert!(error.contains("逐字稿投影失败"), "{error}");
        assert!(
            error.contains("lane-torn"),
            "错误必须带 thread 标识：{error}"
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
