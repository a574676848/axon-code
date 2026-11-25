//! 工作台 HTTP 路由的契约测试：协议版本、路径解析与各载荷字段。

use super::*;

use axon_agent_runtime::ShellCommandToolRuntime;
use axon_agent_runtime::job_runtime::{JOB_REASON_USER_STOP, JobId, JobRuntime, JobSpec};
use axon_session::EventLogStore as _;
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
fn subagent_message_target_parses_session_and_agent() {
    assert_eq!(
        session_subagent_message_target(
            "sessions/session-session-a/subagents/subagent%3A1/messages"
        ),
        Some(("session-session-a".to_string(), "subagent:1".to_string()))
    );
    assert_eq!(
        session_subagent_message_target(
            "sessions/session-session-a/subagents/subagent%3A%2Froot%2Fw/messages"
        ),
        Some((
            "session-session-a".to_string(),
            "subagent:/root/w".to_string()
        ))
    );
    assert_eq!(
        session_subagent_message_target("sessions/session-session-a/subagents/worker/messages"),
        Some(("session-session-a".to_string(), "worker".to_string()))
    );
    assert_eq!(
        session_subagent_message_target("sessions/session-session-a/subagents/worker"),
        None
    );
    assert_eq!(
        session_subagent_message_target("sessions/session-session-a/subagents//messages"),
        None
    );
    // 车道段里多出斜杠说明这不是本路由。
    assert_eq!(
        session_subagent_message_target("sessions/a/b/subagents/worker/messages"),
        None
    );
    // 坏转义按不匹配处理，而不是解出一段乱码当子代理名。
    assert_eq!(
        session_subagent_message_target("sessions/session-session-a/subagents/%ZZ/messages"),
        None
    );
}

#[test]
fn test_question_targets_parsing() {
    assert_eq!(
        session_question_answer_target("sessions/session-test/questions/call-123/answer"),
        Some(("session-test".to_string(), "call-123".to_string()))
    );
    assert_eq!(
        session_question_answer_target("sessions/session-test/questions/call-123"),
        None
    );
    assert_eq!(
        session_questions_target("sessions/session-test/questions"),
        Some("session-test".to_string())
    );
    assert_eq!(
        session_questions_target("sessions/session-test/other"),
        None
    );
}

#[test]
fn sessions_route_returns_the_desktop_contract_shape() {
    // 桌面左侧菜单按 `{ sessions: [...] }` 解析，并且每条会话要带会话表面的渠道与目标。
    // 这条用例锁住「清单读路由归工作台」：裸数组会让桌面直接报「网关响应缺少 sessions 字段」。
    let temp_root = axon_test_support::test_temp_root();
    // 运行态库要先建出来：没有表时投影会如实报错，而不是伪装成空清单。
    axon_session::SqliteRuntimeStore::from_root(temp_root.join("data"))
        .initialize()
        .expect("初始化运行态库");
    let adapter = WorkbenchHttpAdapter::from_root(&temp_root);
    let response = adapter
        .handle_request("GET", "/api/v1/sessions", None, &[])
        .expect("工作台必须接管会话清单读路由");
    assert_eq!(response.status, 200);
    let payload: serde_json::Value =
        serde_json::from_slice(&response.body).expect("响应是 JSON 对象");
    assert!(
        payload
            .get("sessions")
            .is_some_and(|value| value.is_array()),
        "载荷必须是带 sessions 数组的对象：{payload}"
    );
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
            "session-session-idle",
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
        .send_subagent_message("session-session-a", "/root/worker", br#"{"content":"   "}"#)
        .expect_err("空正文必须被拒绝");
    assert!(
        empty.contains("不能为空"),
        "拒绝原因必须说明空正文：{empty}"
    );
    let oversized = format!(r#"{{"content":"{}"}}"#, "字".repeat(8_001));
    let error = adapter
        .send_subagent_message("session-session-a", "/root/worker", oversized.as_bytes())
        .expect_err("超长正文必须被拒绝");
    assert!(error.contains("超过"), "拒绝原因必须说明超长：{error}");
}

#[test]
fn session_jobs_target_parses_session_only() {
    assert_eq!(
        session_jobs_target("sessions/session-session-a/jobs"),
        Some("session-session-a".to_string())
    );
    assert_eq!(
        session_jobs_target("sessions/session-session-a/jobs/1/stop"),
        None
    );
    assert_eq!(session_jobs_target("sessions//jobs"), None);
}

/// 尾随路由与名册、停止互不吞并：三者前缀相同，动作是唯一判据。
#[test]
fn job_output_target_parses_session_and_job() {
    assert_eq!(
        session_job_output_target("sessions/session-session-a/jobs/256/output"),
        Some(("session-session-a".to_string(), "256".to_string()))
    );
    assert_eq!(
        session_job_output_target("sessions/session-session-a/jobs/256/stop"),
        None
    );
    assert_eq!(
        session_job_output_target("sessions/session-session-a/jobs"),
        None
    );
    assert_eq!(
        session_job_output_target("sessions/session-session-a/jobs//output"),
        None
    );
    assert_eq!(session_job_output_target("sessions//jobs/1/output"), None);
}

#[test]
fn empty_roster_is_returned_for_a_session_without_runtime() {
    let root = axon_test_support::test_temp_root();
    // 先让存储建出运行态表：真实环境里 daemon 启动时就已经建好，
    // 缺表在这里只是测试脚手架的假象。
    let other = DaemonSessionId::new("session-session-schema").expect("车道标识合法");
    let _ = runtime_store(&root)
        .create_session(&other, ".", 1)
        .expect("登记会话必须成功");
    let payload = session_jobs_payload(&root, "session-session-no-runtime", &[])
        .expect("没有运行会话时名册应为空而不是报错");
    assert_eq!(payload["jobIds"], serde_json::json!([]));
    assert!(payload["epoch"].as_str().is_some_and(|v| !v.is_empty()));
}

#[test]
fn roster_lists_jobs_owned_by_the_session_identity_and_its_run_window() {
    let root = axon_test_support::test_temp_root();
    let session_id = DaemonSessionId::new("session-session-roster").expect("车道标识合法");
    let record = runtime_store(&root)
        .create_session(&session_id, ".", 1)
        .expect("登记会话必须成功");
    // 生产路径里 Shell 工具把会话身份写进作业 owner，运行窗口是另一个标识。
    // 名册只按窗口查就会恒空，界面把仍在跑的作业判成已结束。
    assert_ne!(record.current_run_id, session_id.as_str());
    let owner = session_id.as_str();
    let by_session = start_job(owner);
    let by_run_window = start_job(&record.current_run_id);
    let other = start_job("session-session-roster-other");

    // 点名一条仍在名册里的作业：它不得被报成收口，也不该被收口事实取代。
    let payload =
        session_jobs_payload(&root, owner, &[by_session.0.to_string()]).expect("名册必须可读");
    let mut listed = payload["jobIds"]
        .as_array()
        .expect("名册是数组")
        .iter()
        .map(|value| value.as_str().unwrap_or_default().to_owned())
        .collect::<Vec<_>>();
    listed.sort_unstable();
    let mut expected = vec![by_session.0.to_string(), by_run_window.0.to_string()];
    expected.sort_unstable();
    assert_eq!(listed, expected, "名册只列本车道仍在跑的作业");
    assert_eq!(
        payload["settled"],
        serde_json::json!([]),
        "仍在跑的作业即使被点名，也不得被报成收口"
    );

    let _ = JobRuntime::global().kill(by_session, owner);
    let _ = JobRuntime::global().kill(by_run_window, &record.current_run_id);
    let _ = JobRuntime::global().kill(other, "session-session-roster-other");
}

/// 在会话事件日志里追加一条作业事件，形状与运行侧落盘的一致。
fn append_job_event(
    store: &SqliteRuntimeStore,
    session: &DaemonSessionId,
    kind: &str,
    payload: &str,
) {
    let host_id = axon_domain::RuntimeHostId::new(format!("test-jobs-{}", std::process::id()))
        .expect("host 标识合法");
    let host = store
        .claim_host(&host_id, std::process::id(), None, 1, 60_000)
        .expect("取得 host 租约");
    store
        .append_events(session, &host, &[(kind.to_owned(), payload.to_owned())], 2)
        .expect("写入作业事件必须成功");
}

#[test]
fn roster_reports_settled_facts_measured_by_the_backend() {
    let root = axon_test_support::test_temp_root();
    let session_id = DaemonSessionId::new("session-session-settled").expect("车道标识合法");
    let store = runtime_store(&root);
    let _ = store
        .create_session(&session_id, ".", 1)
        .expect("登记会话必须成功");
    // 收口事实来自落盘事件：耗时是作业自己的实测值，不是启动回执的工具耗时。
    append_job_event(
        &store,
        &session_id,
        "job_started",
        r#"{"jobId":"7","kind":"shell","owner":"session-session-settled","startedAtMs":100}"#,
    );
    append_job_event(
        &store,
        &session_id,
        "job_completed",
        r#"{"jobId":"7","status":"completed","elapsedMs":192340,"exitCode":0}"#,
    );

    let payload = session_jobs_payload(
        &root,
        "session-session-settled",
        &["7".to_owned(), "9".to_owned()],
    )
    .expect("名册必须可读");
    assert_eq!(payload["jobIds"], serde_json::json!([]));
    let settled = payload["settled"].as_array().expect("收口事实是数组");
    assert_eq!(settled.len(), 1, "只回点名且确有收口事实的那一条");
    assert_eq!(settled[0]["jobId"], serde_json::json!("7"));
    assert_eq!(settled[0]["status"], serde_json::json!("completed"));
    assert_eq!(settled[0]["elapsedMs"], serde_json::json!(192340));
    assert_eq!(settled[0]["exitCode"], serde_json::json!(0));
}

#[test]
fn settled_facts_take_the_most_recent_close_for_one_job_id() {
    let root = axon_test_support::test_temp_root();
    let session_id = DaemonSessionId::new("session-session-reclose").expect("车道标识合法");
    let store = runtime_store(&root);
    let _ = store
        .create_session(&session_id, ".", 1)
        .expect("登记会话必须成功");
    // 作业标识在进程重启后从 1 重计，同一段日志里可以存在同一个 id 的两代收口记录。
    for elapsed in [1_000_u64, 9_000] {
        append_job_event(
            &store,
            &session_id,
            "job_completed",
            &format!(r#"{{"jobId":"7","status":"completed","elapsedMs":{elapsed}}}"#),
        );
    }
    let payload = session_jobs_payload(&root, "session-session-reclose", &["7".to_owned()])
        .expect("名册必须可读");
    assert_eq!(
        payload["settled"][0]["elapsedMs"],
        serde_json::json!(9_000),
        "倒序扫描必须取最近一次收口，早先的记录不得覆盖它"
    );
    assert_eq!(
        payload["settled"].as_array().map(Vec::len),
        Some(1),
        "同一个作业标识只占一行"
    );
}

#[test]
fn settled_facts_stay_inside_the_asking_lane() {
    let root = axon_test_support::test_temp_root();
    let store = runtime_store(&root);
    let mine = DaemonSessionId::new("session-session-mine").expect("车道标识合法");
    let other = DaemonSessionId::new("session-session-other").expect("车道标识合法");
    let _ = store.create_session(&mine, ".", 1).expect("登记本车道");
    let _ = store.create_session(&other, ".", 1).expect("登记他人车道");
    append_job_event(
        &store,
        &other,
        "job_completed",
        r#"{"jobId":"8","status":"failed","elapsedMs":5000,"exitCode":1}"#,
    );

    let payload = session_jobs_payload(&root, "session-session-mine", &["8".to_owned()])
        .expect("名册必须可读");
    assert_eq!(
        payload["settled"],
        serde_json::json!([]),
        "别人车道的作业不得出现在本车道的收口事实里"
    );
}

#[test]
fn job_stop_target_parses_session_and_job() {
    assert_eq!(
        session_job_stop_target("sessions/session-session-abc/jobs/7/stop"),
        Some(("session-session-abc".to_string(), "7".to_string()))
    );
    assert_eq!(
        session_job_stop_target("sessions/session-session-abc/jobs/7"),
        None
    );
    assert_eq!(session_job_stop_target("sessions//jobs/7/stop"), None);
    assert_eq!(
        session_job_stop_target("sessions/session-session-abc/interrupt"),
        None
    );
}

#[test]
fn ownership_accepts_session_identity_and_its_run_session() {
    assert!(owner_matches_session(
        "session-session-a",
        "session-session-a",
        None
    ));
    assert!(owner_matches_session(
        "session-session-a",
        "run-uuid",
        Some("run-uuid")
    ));
    assert!(!owner_matches_session(
        "session-session-a",
        "run-other",
        Some("run-uuid")
    ));
    assert!(!owner_matches_session(
        "session-session-a",
        "run-uuid",
        None
    ));
}

#[test]
fn stop_is_rejected_for_unknown_job() {
    let root = axon_test_support::test_temp_root();
    let error = stop_session_job(&root, "session-session-missing", "999999")
        .expect_err("未知作业必须被拒绝");
    assert!(
        error.contains("不存在"),
        "拒绝原因必须是作业不可达：{error}"
    );
}

#[test]
fn stop_is_rejected_for_job_owned_by_another_session() {
    let root = axon_test_support::test_temp_root();
    let session_id = "session-session-owner-b";
    let session_id = DaemonSessionId::new(session_id).expect("车道标识合法");
    // 先登记本车道的运行会话：归属核对发生在真实存储上，缺表会被当成读取失败。
    let _ = runtime_store(&root)
        .create_session(&session_id, ".", 1)
        .expect("登记车道运行会话必须成功");
    let owner = "run-session-of-other-session";
    let job_id = start_job(owner);
    let error = stop_session_job(&root, session_id.as_str(), &job_id.0.to_string())
        .expect_err("他人车道的作业必须被拒绝");
    assert!(error.contains("不属于"), "拒绝原因必须是归属不符：{error}");
    let _ = JobRuntime::global().kill(job_id, owner);
}

#[test]
fn stop_records_user_reason_for_the_session_run_session() {
    let root = axon_test_support::test_temp_root();
    let session_id = "session-session-owner-c";
    let session_id = DaemonSessionId::new(session_id).expect("车道标识合法");
    let record = runtime_store(&root)
        .create_session(&session_id, ".", 1)
        .expect("登记车道运行会话必须成功");
    let job_id = start_job(&record.current_run_id);

    let payload = stop_session_job(&root, session_id.as_str(), &job_id.0.to_string())
        .expect("本车道运行会话的作业必须可停止");
    assert_eq!(payload["stopped"], serde_json::json!(true));
    assert_eq!(payload["reason"], serde_json::json!(JOB_REASON_USER_STOP));
    assert_eq!(
        JobRuntime::global().terminal_reason(job_id).as_deref(),
        Some(JOB_REASON_USER_STOP),
        "终态原因必须落到注册表，随完成通知回到 Agent"
    );
}

/// 组合器必须把网关解析出的身份交给各适配器。
///
/// 这条守卫针对的是「身份在组合层被悄悄丢掉」：网关注入的处理器只有这一个组合器，
/// 它如果不覆写 `handle_request_as`，`/api/v1/keys` 这类按身份分级的适配器拿到的永远
/// 是 `None`，连超管也会被拒成 403，而适配器自己的单测依旧全绿。
#[test]
fn composite_forwards_identity_to_every_handler() {
    use axon_daemon::infrastructure::gateway_auth::GatewayIdentity;
    use std::sync::Mutex;

    /// 只记录拿到什么身份的处理器。
    struct IdentityRecorder {
        seen: Mutex<Option<Option<GatewayIdentity>>>,
    }

    impl GatewayHttpRouteHandler for IdentityRecorder {
        fn handle_request(
            &self,
            _method: &str,
            _path: &str,
            _query: Option<&str>,
            _body: &[u8],
        ) -> Option<GatewayHttpResponse> {
            // 无身份入口在这里被选中即说明身份确实为 None。
            *self.seen.lock().expect("记录锁可用") = Some(None);
            Some(GatewayHttpResponse::json(
                200,
                &serde_json::json!({ "ok": true }),
            ))
        }

        fn handle_request_as(
            &self,
            _method: &str,
            _path: &str,
            _query: Option<&str>,
            _body: &[u8],
            identity: Option<&GatewayIdentity>,
            _peer: Option<&str>,
        ) -> Option<GatewayHttpResponse> {
            *self.seen.lock().expect("记录锁可用") = Some(identity.cloned());
            Some(GatewayHttpResponse::json(
                200,
                &serde_json::json!({ "ok": true }),
            ))
        }
    }

    let recorder = Arc::new(IdentityRecorder {
        seen: Mutex::new(None),
    });
    let composite = CompositeHttpAdapter::new(vec![recorder.clone()]);

    let response = composite.handle_request_as(
        "GET",
        "/api/v1/keys",
        None,
        b"",
        Some(&GatewayIdentity::SuperAdmin),
        None,
    );
    assert!(response.is_some(), "命中处理器时必须给出响应");
    assert_eq!(
        *recorder.seen.lock().expect("记录锁可用"),
        Some(Some(GatewayIdentity::SuperAdmin)),
        "身份必须原样透传给适配器"
    );

    // 无身份的调用同样照原样传下去：适配器据此拒绝，而不是被组合层替换成超管。
    let response = composite.handle_request("GET", "/api/v1/keys", None, b"");
    assert!(response.is_some());
    assert_eq!(*recorder.seen.lock().expect("记录锁可用"), Some(None));
}

#[test]
fn interaction_events_target_carries_tail_flag() {
    // 默认是按 after 正向翻页，不带尾窗语义。
    assert_eq!(
        interaction_events_target("interactions/webhook-abc/events", None),
        Some(("webhook-abc".to_string(), 0, 800, false))
    );
    // tail=1 与 tail=true 等价：首屏一次取该交互自身的尾窗。
    assert_eq!(
        interaction_events_target(
            "interactions/webhook-abc/events",
            Some("after=12&limit=34&tail=1")
        ),
        Some(("webhook-abc".to_string(), 12, 34, true))
    );
    assert_eq!(
        interaction_events_target("interactions/webhook-abc/events", Some("tail=true")),
        Some(("webhook-abc".to_string(), 0, 800, true))
    );
    // 尾部多出一段说明这不是本路由，不能误命中。
    assert_eq!(
        interaction_events_target("interactions/webhook-abc/events/extra", None),
        None
    );
}
