//! 工作台 HTTP 路由的契约测试：协议版本、路径解析与各载荷字段。

use super::*;

use axon_agent_runtime::ShellCommandToolRuntime;
use axon_agent_runtime::job_runtime::{JOB_REASON_USER_STOP, JobId, JobRuntime, JobSpec};
use std::sync::Arc;

/// 需要人工收口的命令：给停止入口留出操作窗口。
const SLEEP_COMMAND: &str = "sleep 30";

/// 启动一个归属明确的后台作业，返回作业标识。
fn start_job(owner: &str) -> JobId {
    JobRuntime::global()
        .start(JobSpec {
            owner_id: owner.to_owned(),
            label: SLEEP_COMMAND.to_owned(),
            command: ShellCommandToolRuntime::command_from_script(SLEEP_COMMAND, false)
                .expect("测试命令应规范化成功"),
            cwd: std::env::current_dir().expect("测试工作目录应可解析"),
            timeout_ms: None,
            cancel_token: None,
            tee_root: Some(axon_test_support::test_temp_root()),
        })
        .expect("启动测试作业必须成功")
}

#[test]
fn subagent_message_target_parses_lane_and_agent() {
    assert_eq!(
        lane_subagent_message_target("lanes/session-lane-a/subagents/subagent%3A1/messages"),
        Some(("session-lane-a".to_string(), "subagent:1".to_string()))
    );
    assert_eq!(
        lane_subagent_message_target(
            "lanes/session-lane-a/subagents/subagent%3A%2Froot%2Fw/messages"
        ),
        Some(("session-lane-a".to_string(), "subagent:/root/w".to_string()))
    );
    assert_eq!(
        lane_subagent_message_target("lanes/session-lane-a/subagents/worker/messages"),
        Some(("session-lane-a".to_string(), "worker".to_string()))
    );
    assert_eq!(
        lane_subagent_message_target("lanes/session-lane-a/subagents/worker"),
        None
    );
    assert_eq!(
        lane_subagent_message_target("lanes/session-lane-a/subagents//messages"),
        None
    );
    // 车道段里多出斜杠说明这不是本路由。
    assert_eq!(
        lane_subagent_message_target("lanes/a/b/subagents/worker/messages"),
        None
    );
    // 坏转义按不匹配处理，而不是解出一段乱码当子代理名。
    assert_eq!(
        lane_subagent_message_target("lanes/session-lane-a/subagents/%ZZ/messages"),
        None
    );
}

#[test]
fn test_question_targets_parsing() {
    assert_eq!(
        lane_question_answer_target("lanes/lane-test/questions/call-123/answer"),
        Some(("lane-test".to_string(), "call-123".to_string()))
    );
    assert_eq!(
        lane_question_answer_target("lanes/lane-test/questions/call-123"),
        None
    );
    assert_eq!(
        lane_questions_target("lanes/lane-test/questions"),
        Some("lane-test".to_string())
    );
    assert_eq!(lane_questions_target("lanes/lane-test/other"), None);
}

#[test]
fn test_workbench_adapter_with_answer_service() {
    let temp_root = axon_test_support::test_temp_root();
    let store = Arc::new(axon_session::SqliteRuntimeStore::from_root(
        temp_root.join("data"),
    ));
    let service = crate::user_question::UserQuestionAnswerService::new(
        store,
        None,
        Arc::new(crate::user_question::NoopUserQuestionEventSink),
        Arc::new(crate::user_question::NoopQuestionMessageSteerer),
    );
    let adapter = WorkbenchHttpAdapter::from_root(&temp_root).with_answer_service(service);
    assert!(adapter.answer_service.is_some());
}

#[test]
fn subagent_message_is_rejected_without_an_active_turn() {
    let adapter = WorkbenchHttpAdapter::from_root(axon_test_support::test_temp_root());
    let error = adapter
        .send_subagent_message(
            "session-lane-idle",
            "subagent:/root/worker",
            r#"{"content":"继续"}"#.as_bytes(),
        )
        .expect_err("没有活跃主 turn 时必须拒绝");
    assert!(
        error.contains("没有正在运行"),
        "拒绝原因必须说明没有活跃会话：{error}"
    );
}

#[test]
fn subagent_message_rejects_empty_and_oversized_content() {
    let adapter = WorkbenchHttpAdapter::from_root(axon_test_support::test_temp_root());
    let empty = adapter
        .send_subagent_message("session-lane-a", "/root/worker", br#"{"content":"   "}"#)
        .expect_err("空正文必须被拒绝");
    assert!(
        empty.contains("不能为空"),
        "拒绝原因必须说明空正文：{empty}"
    );
    let oversized = format!(r#"{{"content":"{}"}}"#, "字".repeat(8_001));
    let error = adapter
        .send_subagent_message("session-lane-a", "/root/worker", oversized.as_bytes())
        .expect_err("超长正文必须被拒绝");
    assert!(error.contains("超过"), "拒绝原因必须说明超长：{error}");
}

#[test]
fn lane_jobs_target_parses_lane_only() {
    assert_eq!(
        lane_jobs_target("lanes/session-lane-a/jobs"),
        Some("session-lane-a".to_string())
    );
    assert_eq!(lane_jobs_target("lanes/session-lane-a/jobs/1/stop"), None);
    assert_eq!(lane_jobs_target("lanes//jobs"), None);
}

#[test]
fn empty_roster_is_returned_for_a_lane_without_runtime() {
    let root = axon_test_support::test_temp_root();
    // 先让存储建出运行态表：真实环境里 daemon 启动时就已经建好，
    // 缺表在这里只是测试脚手架的假象。
    let other = DaemonSessionId::new("session-lane-schema").expect("车道标识合法");
    let _ = runtime_store(&root)
        .create_session(&other, ".", 1)
        .expect("登记会话必须成功");
    let payload = lane_jobs_payload(&root, "session-lane-no-runtime")
        .expect("没有运行会话时名册应为空而不是报错");
    assert_eq!(payload["jobIds"], serde_json::json!([]));
    assert!(payload["epoch"].as_str().is_some_and(|v| !v.is_empty()));
}

#[test]
fn job_stop_target_parses_lane_and_job() {
    assert_eq!(
        lane_job_stop_target("lanes/session-lane-abc/jobs/7/stop"),
        Some(("session-lane-abc".to_string(), "7".to_string()))
    );
    assert_eq!(lane_job_stop_target("lanes/session-lane-abc/jobs/7"), None);
    assert_eq!(lane_job_stop_target("lanes//jobs/7/stop"), None);
    assert_eq!(
        lane_job_stop_target("lanes/session-lane-abc/interrupt"),
        None
    );
}

#[test]
fn ownership_accepts_lane_identity_and_its_run_session() {
    assert!(owner_matches_lane("session-lane-a", "session-lane-a", None));
    assert!(owner_matches_lane(
        "session-lane-a",
        "run-uuid",
        Some("run-uuid")
    ));
    assert!(!owner_matches_lane(
        "session-lane-a",
        "run-other",
        Some("run-uuid")
    ));
    assert!(!owner_matches_lane("session-lane-a", "run-uuid", None));
}

#[test]
fn stop_is_rejected_for_unknown_job() {
    let root = axon_test_support::test_temp_root();
    let error =
        stop_lane_job(&root, "session-lane-missing", "999999").expect_err("未知作业必须被拒绝");
    assert!(
        error.contains("不存在"),
        "拒绝原因必须是作业不可达：{error}"
    );
}

#[test]
fn stop_is_rejected_for_job_owned_by_another_lane() {
    let root = axon_test_support::test_temp_root();
    let lane_id = "session-lane-owner-b";
    let session_id = DaemonSessionId::new(lane_id).expect("车道标识合法");
    // 先登记本车道的运行会话：归属核对发生在真实存储上，缺表会被当成读取失败。
    let _ = runtime_store(&root)
        .create_session(&session_id, ".", 1)
        .expect("登记车道运行会话必须成功");
    let owner = "run-session-of-other-lane";
    let job_id = start_job(owner);
    let error =
        stop_lane_job(&root, lane_id, &job_id.0.to_string()).expect_err("他人车道的作业必须被拒绝");
    assert!(error.contains("不属于"), "拒绝原因必须是归属不符：{error}");
    let _ = JobRuntime::global().kill(job_id, owner);
}

#[test]
fn stop_records_user_reason_for_the_lane_run_session() {
    let root = axon_test_support::test_temp_root();
    let lane_id = "session-lane-owner-c";
    let session_id = DaemonSessionId::new(lane_id).expect("车道标识合法");
    let record = runtime_store(&root)
        .create_session(&session_id, ".", 1)
        .expect("登记车道运行会话必须成功");
    let job_id = start_job(&record.run_session_id);

    let payload = stop_lane_job(&root, lane_id, &job_id.0.to_string())
        .expect("本车道运行会话的作业必须可停止");
    assert_eq!(payload["stopped"], serde_json::json!(true));
    assert_eq!(payload["reason"], serde_json::json!(JOB_REASON_USER_STOP));
    assert_eq!(
        JobRuntime::global().terminal_reason(job_id).as_deref(),
        Some(JOB_REASON_USER_STOP),
        "终态原因必须落到注册表，随完成通知回到 Agent"
    );
}
