use super::*;
use axon_agent_runtime::{QuestionWaitBudget, QuestionWaitResult};
use axon_domain::{
    ConsumerCursor, DaemonSessionId, RuntimeHostId, UserAnswerItem, UserQuestionRecord,
    UserQuestionState,
};
use axon_protocol::wire::{
    WireCorrelationId, WireEventEnvelope, WireEventKind, WireSchemaVersion, WireSessionRef,
    WireUserQuestion,
};
use axon_session::{InteractionQueryStore, RuntimeStore, SessionEventRecord, UserQuestionStore};
use std::{sync::mpsc, thread, time::Duration};

const PUBLISH_TIMEOUT: Duration = Duration::from_secs(1);
/// 守护进程问答装配的等待者注册与作答回灌超时。
const WIRING_TIMEOUT: Duration = Duration::from_secs(5);

#[test]
fn pid_guard_replaces_only_a_provably_dead_pid() {
    let directory = axon_test_support::test_temp_dir();
    let runtime_root = directory.path().to_path_buf();
    let pid_path = runtime_root.join("daemon.pid");
    std::fs::write(&pid_path, format!("{}\n", u32::MAX)).expect("写入 stale PID");

    let guard = DaemonPidFileGuard::acquire(&runtime_root).expect("接管 stale PID");
    let written = std::fs::read_to_string(&pid_path).expect("读取 PID");
    assert_eq!(written.trim(), std::process::id().to_string());
    drop(guard);
    assert!(!pid_path.exists());
}

#[test]
fn pid_guard_rejects_a_live_owner() {
    let directory = axon_test_support::test_temp_dir();
    let runtime_root = directory.path().to_path_buf();
    let guard = DaemonPidFileGuard::acquire(&runtime_root).expect("写入当前 PID");

    let error = match DaemonPidFileGuard::acquire(&runtime_root) {
        Err(error) => error,
        Ok(guard) => {
            drop(guard);
            panic!("活 PID 应拒绝接管")
        }
    };
    assert!(error.contains(&format!("pid={}", std::process::id())));
    drop(guard);
}

#[test]
fn daemon_event_publish_does_not_wait_for_process_stdout_lock() {
    let directory = axon_test_support::test_temp_dir();
    let publisher = DaemonEventPublisher::new(
        directory.path().to_path_buf(),
        directory.path().join("config.toml"),
        Arc::new(DaemonMaintenanceSignal::default()),
    );
    let event = SessionEventRecord {
        session_id: DaemonSessionId::new("stdout-lock-session").expect("Session ID 有效"),
        sequence: 1,
        emitted_at_ms: 1,
        generation: "generation-a".to_owned(),
        event_type: "turn_started".to_owned(),
        payload_json: "{}".to_owned(),
    };
    let cursor = ConsumerCursor::new("generation-a", 1);
    let (sender, receiver) = mpsc::channel();
    let _stdout_guard = std::io::stdout().lock();

    thread::spawn(move || sender.send(publisher.publish(&event, &cursor)));

    receiver
        .recv_timeout(PUBLISH_TIMEOUT)
        .expect("Daemon 事件发布不应等待 stdout 锁")
        .expect("Daemon 事件发布应成功");
}

#[test]
fn mcp_refresh_request_clears_health_cache_and_invalidates_snapshot() {
    let directory = axon_test_support::test_temp_dir();
    let root = directory.path();
    let config_path = root.join("runtime").join("config.toml");
    std::fs::create_dir_all(config_path.parent().expect("配置目录")).expect("配置目录可创建");
    axon_config::ConfigLoader::save_to_path(&config_path, &axon_config::AxonConfig::default())
        .expect("写入配置");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).expect("工作区可创建");
    let axon_root = axon_config::runtime_axon_root_path(&config_path).expect("Axon 根目录");
    let cache_path = axon_root
        .join("cache")
        .join("mcp_registry_health_negative.json");
    std::fs::create_dir_all(cache_path.parent().expect("缓存目录")).expect("缓存目录可创建");
    std::fs::write(
        &cache_path,
        r#"{"failures":[{"configRef":"mcp://team/.mcp.json","failedAtMs":1,"reason":"启动失败"}]}"#,
    )
    .expect("写入健康负缓存");

    let context = McpRefreshContext {
        config_path: config_path.clone(),
        snapshots: crate::run_commands::StartupSnapshotCache::default(),
    };
    context
        .snapshots
        .acquire(&config_path, &workspace)
        .expect("首次应构建快照");
    assert!(
        context
            .snapshots
            .contains_for_tests(&config_path, &workspace)
    );

    let responses =
        handle_mcp_refresh_request(&context, "req-mcp-refresh", &workspace.to_string_lossy());

    assert_eq!(responses.len(), 1);
    assert!(responses[0].ok);
    assert_eq!(responses[0].request_id, "req-mcp-refresh");
    assert_eq!(responses[0].body["refreshed"], true);
    assert_eq!(responses[0].body["mcpHealthCacheCleared"], 1);
    let remaining: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cache_path).expect("读取缓存"))
            .expect("缓存可解析");
    assert_eq!(
        remaining["failures"].as_array().map(Vec::len),
        Some(0),
        "刷新必须清空 MCP 健康负缓存，让下一次构建现场重探"
    );
}

#[test]
fn mcp_refresh_request_rejects_blank_cwd() {
    let directory = axon_test_support::test_temp_dir();
    let context = McpRefreshContext {
        config_path: directory.path().join("config.toml"),
        snapshots: crate::run_commands::StartupSnapshotCache::default(),
    };

    let responses = handle_mcp_refresh_request(&context, "req-blank-cwd", "   ");

    assert_eq!(responses.len(), 1);
    assert!(!responses[0].ok);
    assert_eq!(responses[0].body["error"], "cwd 不能为空");
}

#[test]
fn daemon_question_wiring_serves_the_waiting_primary_turn_from_one_host_instance() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::from_root(directory.path());
    let wiring = QuestionWiring::assemble(
        store.clone(),
        ActivePrimaryTurnRegistry::default(),
        None,
        test_wake_hook(),
    );
    assert!(
        !wiring.channel_sink_attached,
        "未提供渠道 sink 时不应并入扇出"
    );
    let port = wiring
        .primary_turn_port()
        .expect("守护进程主 turn 装配路径必须携带用户提问端口");

    let record = pending_question_record("call-daemon-wiring");
    store.insert_question(&record).expect("提问记录落库");

    let (outcome_tx, outcome_rx) = mpsc::channel();
    let waiter = thread::spawn({
        let port = Arc::clone(&port);
        let record = record.clone();
        move || {
            let _ = outcome_tx.send(port.wait_for_answer(
                &record,
                QuestionWaitBudget {
                    initial_window_ms: 5_000,
                    backoff_factor: 2,
                    total_budget_ms: 10_000,
                    poll_interval_ms: 20,
                },
                &axon_agent_runtime::RunCancelToken::default(),
            ));
        }
    });

    // 等待者注册表在宿主实例内部：答案服务必须与主 turn 端口共用同一实例才能回灌作答。
    let deadline = std::time::Instant::now() + WIRING_TIMEOUT;
    while wiring.host.waiter_count() == 0 && std::time::Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        wiring.host.waiter_count(),
        1,
        "主 turn 端口应把等待者注册到答案服务所用的同一个宿主实例"
    );

    let settled = wiring
        .service
        .submit_answer(
            &record.lane_id,
            &record.tool_call_id,
            vec![UserAnswerItem {
                id: "q1".to_owned(),
                selected: vec!["继续".to_owned()],
                custom: None,
            }],
            "test",
        )
        .expect("答案服务应把作答投给等待中的提问工具");
    assert_eq!(
        settled.delivery_mode.as_deref(),
        Some("tool_return"),
        "存在活等待者时必须走 tool_return 投递，而不是退回消息通道"
    );

    match outcome_rx.recv_timeout(WIRING_TIMEOUT).expect("等待应返回") {
        Ok(QuestionWaitResult::Answered { answer_json, .. }) => {
            assert!(
                answer_json.contains("继续"),
                "作答正文应原样回灌给提问工具: {answer_json}"
            );
        }
        other => panic!("期望 Answered，得到 {other:?}"),
    }
    waiter.join().expect("等待线程不应 panic");
    assert_eq!(wiring.host.waiter_count(), 0);
}

#[test]
fn daemon_question_wiring_attaches_the_channel_sink_when_provided() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::from_root(directory.path());
    let channel = Arc::new(NoopChannelSink);
    let wiring = QuestionWiring::assemble(
        store,
        ActivePrimaryTurnRegistry::default(),
        Some(Arc::clone(&channel) as Arc<dyn crate::user_question::UserQuestionEventSink>),
        test_wake_hook(),
    );
    assert!(
        wiring.channel_sink_attached,
        "提供渠道 sink 后必须并入事件扇出，否则提问不会当场出卡"
    );
}

/// 装配自检用的主 turn 唤醒钩子：只确认接线，不承载等待语义。
fn test_wake_hook() -> crate::user_question::lane_steerer::PrimaryTurnWakeHook {
    Arc::new(|| Ok(()))
}

/// Daemon IPC 作答必须走统一答案入口：只结算不投递的路径会让答案落库却永远送不到
/// 等待中的提问工具，也不会开启续跑的一轮。
#[test]
fn ipc_answer_question_delivers_to_the_lane_and_wakes_the_primary_turn() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::from_root(directory.path());
    let lane_id = seed_answer_lane(&store);
    let record = UserQuestionRecord {
        lane_id: lane_id.clone(),
        session_id: lane_id.clone(),
        tool_call_id: "call-ipc-answer".to_owned(),
        state: UserQuestionState::Suspended,
        ..pending_question_record("call-ipc-answer")
    };
    store.insert_question(&record).expect("提问记录落库");

    let wakes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let wiring = QuestionWiring::assemble(
        store.clone(),
        ActivePrimaryTurnRegistry::default(),
        None,
        counting_wake_hook(Arc::clone(&wakes)),
    );

    let response = super::answer_ipc::answer_question_ipc_response(
        &wiring.service,
        "request-answer-1",
        &lane_id,
        "call-ipc-answer",
        vec![UserAnswerItem {
            id: "q1".to_owned(),
            selected: vec!["release".to_owned()],
            custom: None,
        }],
        "tui",
    );

    assert!(response.ok, "IPC 作答必须成功结算: {:?}", response.body);
    assert_eq!(response.body["deliveryMode"], "user_message");
    let answer_id = axon_domain::InteractionId::new("question-answer-uq-call-ipc-answer")
        .expect("合法答案 Interaction ID");
    let delivered = store
        .get_interaction(&answer_id)
        .expect("读取答案 Interaction")
        .expect("IPC 作答必须把答案投递到车道，而不是只结算权威记录");
    assert_eq!(
        delivered.status,
        axon_domain::InteractionStatus::Received,
        "答案先落成待领取态"
    );
    assert_eq!(
        wakes.load(std::sync::atomic::Ordering::Acquire),
        1,
        "答案落库后必须唤醒一次主 turn 领取"
    );
}

/// 唤醒计数钩子：只记录唤醒次数。
fn counting_wake_hook(
    wakes: Arc<std::sync::atomic::AtomicUsize>,
) -> crate::user_question::lane_steerer::PrimaryTurnWakeHook {
    Arc::new(move || {
        wakes.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        Ok(())
    })
}

/// 在测试仓库里登记一条车道，返回车道标识。
///
/// 答案注入要按车道行回读作用域与回复路由，因此作答之前车道必须已有 Interaction。
fn seed_answer_lane(store: &SqliteRuntimeStore) -> String {
    use axon_domain::{
        AudienceContext, AudienceKind, DisclosurePolicy, InteractionId, InteractionRecord,
        InteractionSource, ReplyRoute, ScopeRef,
    };
    let seed = InteractionRecord::new(
        InteractionId::new("interaction-ipc-answer").expect("合法 ID"),
        InteractionSource::new("tui", "seed-message-1").expect("合法来源"),
        ScopeRef::workspace("workspace-ipc-answer").expect("合法作用域"),
        AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly),
        ReplyRoute::new("tui", "terminal-ipc-answer").expect("合法回复路由"),
        "历史提问",
        1,
    )
    .expect("合法 Interaction");
    let lane_id = axon_session::interaction_session_lane_id(&seed).expect("种子记录绑定 Workspace");
    axon_session::InteractionStore::append(store, &seed).expect("种子 Interaction 落库");
    lane_id
}

/// 只用于装配自检的渠道 sink 桩：确认扇出接线，不关心投递内容。
struct NoopChannelSink;

impl crate::user_question::UserQuestionEventSink for NoopChannelSink {
    fn emit_question_event(
        &self,
        _session_id: &str,
        _part_id: &str,
        _call_id: &str,
        _status: &str,
        _questions: &[axon_domain::UserQuestionItem],
        _answers: &[UserAnswerItem],
        _handoff: Option<&str>,
        _deadline_at_ms: i64,
    ) {
    }
}

/// 没有活跃 run 时，问答帧必须落进会话事件流。
///
/// 迟到作答发生在回合之外，那时车道上没有 run 承接这一帧；若就地丢弃，按历史回放的
/// 客户端永远看不到「已结算」，会把答过的提问一直摆成待答并给出注定被拒绝的回入口。
#[test]
fn question_event_without_active_run_lands_in_the_session_event_stream() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::from_root(directory.path());
    let lane_id = "session-lane-durable-question";
    let session_id = DaemonSessionId::new(lane_id).expect("合法车道标识");
    let host_id = RuntimeHostId::new("durable-question-host").expect("合法宿主标识");
    // 落盘用真实时钟过代际围栏，宿主租约也必须按真实时钟申请，否则会被判为过期宿主。
    let now_ms = crate::daemon_commands::now_ms();
    let host = store
        .claim_host(&host_id, 1, None, now_ms, 600_000)
        .expect("宿主租约");
    store
        .create_session(&session_id, ".", now_ms)
        .expect("车道会话登记");

    let bridge = std::sync::Arc::new(axon_daemon::EventStreamBridge::new());
    let writer =
        super::question_durable::durable_question_event_writer(store.clone(), host, bridge);

    let call_id = "call-durable-1";
    let envelope = WireEventEnvelope {
        schema_version: WireSchemaVersion::default(),
        event_id: WireCorrelationId(format!("question:{call_id}:7")),
        correlation_id: WireCorrelationId(format!("question:{call_id}")),
        session: WireSessionRef {
            session_id: lane_id.to_owned(),
            run_id: None,
            parent_session_id: None,
        },
        sequence: 7,
        recorded_at_ms: 12,
        kind: WireEventKind::UserQuestion(WireUserQuestion {
            part_id: format!("part:question:{call_id}"),
            call_id: call_id.to_owned(),
            status: "settled".to_owned(),
            questions: Vec::new(),
            answers: Vec::new(),
            handoff: None,
            deadline_at_ms: 11,
        }),
    };

    writer(lane_id, &envelope).expect("没有活跃 run 时必须落盘成功");

    let events = store
        .replay_events(&session_id, 0, 10)
        .expect("回放会话事件");
    assert_eq!(events.len(), 1, "这一帧必须真的落进会话事件流");
    assert_eq!(
        events[0].event_type, "wire_event",
        "事件类型与当轮 run 写入的一致"
    );
    assert!(
        events[0].payload_json.contains("\"settled\"") && events[0].payload_json.contains(call_id),
        "落盘的载荷必须就是这一条结算帧: {}",
        events[0].payload_json
    );
}

fn pending_question_record(call_id: &str) -> UserQuestionRecord {
    UserQuestionRecord {
        question_id: format!("uq-{call_id}"),
        idempotency_key: format!("idem-{call_id}"),
        lane_id: "lane-daemon-wiring".to_owned(),
        session_id: "session-daemon-wiring".to_owned(),
        interaction_id: "interaction-daemon-wiring".to_owned(),
        turn_id: "turn-daemon-wiring".to_owned(),
        tool_call_id: call_id.to_owned(),
        questions_json: r#"[{"id":"q1","question":"是否继续？"}]"#.to_owned(),
        handoff_text: Some("等待用户决策".to_owned()),
        state: UserQuestionState::Pending,
        deadline_at_ms: 10_000,
        answer_json: None,
        answered_at_ms: None,
        answered_channel: None,
        delivered: false,
        delivery_mode: None,
        created_at_ms: 1_000,
        expires_at_ms: 86_400_000,
    }
}
