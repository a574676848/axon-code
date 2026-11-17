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
fn pending_question_counts_are_read_from_the_outbox_database() {
    // `user_questions` 落在待消费队列库。按控制面库查会以「no such table」失败并被
    // 调用方吞成 0，桌面左侧菜单因此永远看不到「有待答提问」：库选错在界面上就是
    // 一条静默缺失的读数，只能靠这条用例钉住。
    use axon_session::UserQuestionStore as _;
    let temp_root = axon_test_support::test_temp_root();
    let store = axon_session::SqliteRuntimeStore::from_root(temp_root.join("data"));
    store.initialize().expect("初始化运行态库");
    store
        .insert_question(&axon_domain::UserQuestionRecord {
            question_id: "uq-pending-1".to_owned(),
            idempotency_key: "idem-pending-1".to_owned(),
            session_id: "session-pending-question".to_owned(),
            interaction_id: "interaction-pending-question".to_owned(),
            turn_id: "turn-pending-question".to_owned(),
            tool_call_id: "call-pending-question".to_owned(),
            questions_json: r#"[{"question":"继续吗"}]"#.to_owned(),
            handoff_text: None,
            state: axon_domain::UserQuestionState::Pending,
            deadline_at_ms: 60_000,
            answer_json: None,
            answered_at_ms: None,
            answered_channel: None,
            delivered: false,
            delivery_mode: None,
            created_at_ms: 10,
            expires_at_ms: 60_000,
        })
        .expect("写入待答提问");

    let counts = session_pending_question_counts(&temp_root).expect("读取待答计数");
    assert_eq!(
        counts.get("session-pending-question").copied(),
        Some(1),
        "待答计数必须来自待消费队列库：{counts:?}"
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
    let payload = session_jobs_payload(&root, "session-session-no-runtime")
        .expect("没有运行会话时名册应为空而不是报错");
    assert_eq!(payload["jobIds"], serde_json::json!([]));
    assert!(payload["epoch"].as_str().is_some_and(|v| !v.is_empty()));
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
    let job_id = start_job(&record.run_session_id);

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
