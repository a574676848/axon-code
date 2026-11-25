//! 一页历史的补帧契约：投影优先，投影覆盖不到的轮次由事件日志定向补，会话级事实按
//! 事件类型从尾部定向取。
//!
//! 断言的是不变式而不是某一次扫描路径：页里有一轮还没收口时，其他轮次的答案与读数
//! 照样齐全；把尾扫窗口压到 0 之后它们依然齐全，说明这些事实来自按交互定向补帧，
//! 与窗口宽度无关，页内出现未收口的轮次不会把代价放大成整段回放。计划与作业同理：
//! 它们落在尾窗之外时仍必须读全，不能让长会话整段丢掉计划快照与后台作业清单。

use std::path::PathBuf;

use axon_domain::{
    AudienceContext, AudienceKind, DaemonSessionId, DisclosurePolicy, InteractionId,
    InteractionRecord, InteractionSource, ReplyRoute, ScopeRef,
};
use axon_eventlog::{RuntimeEventKind, RuntimeEventRecord};
use axon_session::{InteractionStore as _, RuntimeStore as _, SqliteRuntimeStore};

use super::{SESSION_TAIL_MAX_FRAMES, page_facts, page_facts_within};

struct Fixture {
    root: PathBuf,
    session_id: String,
    store: SqliteRuntimeStore,
    writer: axon_eventlog::EventLogWriter,
    next_received_at_ms: u64,
}

impl Fixture {
    fn new() -> Self {
        let root = axon_test_support::test_temp_root()
            .join(format!("session-readings-{}", uuid::Uuid::new_v4()));
        let data_dir = root.join(axon_config::AXON_RUNTIME_DATA_DIR);
        std::fs::create_dir_all(&data_dir).expect("创建运行态目录");
        let store = SqliteRuntimeStore::from_root(&data_dir);
        store.initialize().expect("初始化运行态库");
        let session_id = axon_session::new_session_id().as_str().to_owned();
        let session = DaemonSessionId::new(session_id.clone()).expect("合法会话标识");
        store.create_session(&session, ".", 1).expect("登记会话行");
        let writer = store.event_log(&session).writer().expect("事件日志写入");
        Self {
            root,
            session_id,
            store,
            writer,
            next_received_at_ms: 1_000,
        }
    }

    /// 登记一条入站交互，返回它的标识。
    fn append_interaction(&mut self, content: &str) -> String {
        let interaction_id = format!("interaction-{}", uuid::Uuid::new_v4());
        let received_at_ms = self.next_received_at_ms;
        self.next_received_at_ms = self.next_received_at_ms.saturating_add(1);
        let record = InteractionRecord::new(
            DaemonSessionId::new(self.session_id.clone()).expect("合法会话标识"),
            InteractionId::new(interaction_id.clone()).expect("合法交互标识"),
            InteractionSource::new("desktop", format!("msg-{}", uuid::Uuid::new_v4()))
                .expect("来源"),
            ScopeRef::workspace(format!("ws-{}", uuid::Uuid::new_v4())).expect("作用域"),
            AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly),
            ReplyRoute::new("desktop", "terminal-fixture").expect("回复路由"),
            content,
            received_at_ms,
        )
        .expect("交互记录");
        self.store.append(&record).expect("交互入库");
        interaction_id
    }

    /// 落一条投影行：索引页的答案与读数优先取自这里。
    fn seed_projection(&self, interaction_id: &str, answer: &str, active_ms: u64) {
        let metrics = turn_metrics_payload(interaction_id, active_ms, 1);
        self.store
            .with_read_connection(|connection| {
                connection.execute(
                    "INSERT INTO turn_activity (
                         session_id, interaction_id, received_at_ms, answer_text, tool_names_json,
                         metrics_json, updated_at_ms
                     ) VALUES (?1, ?2, 1, ?3, NULL, ?4, 1)",
                    rusqlite::params![
                        self.session_id,
                        interaction_id,
                        answer,
                        serde_json::to_string(&metrics).expect("指标序列化"),
                    ],
                )
            })
            .expect("写入投影行");
    }

    /// 落一轮的收口事实：最终正文与逐轮读数各一帧。
    fn append_completed(&mut self, interaction_id: &str, response: &str, active_ms: u64) {
        self.append(
            RuntimeEventKind::TurnCompleted,
            interaction_id,
            serde_json::json!({ "interactionId": interaction_id, "response": response }),
        );
        self.append(
            RuntimeEventKind::TurnMetrics,
            interaction_id,
            turn_metrics_payload(interaction_id, active_ms, 11),
        );
    }

    /// 落一帧运行中轮次的流式事件：有过程，没有收口。
    fn append_running_frame(&mut self, interaction_id: &str) {
        self.append(
            RuntimeEventKind::Other("wire_event".to_owned()),
            interaction_id,
            serde_json::json!({
                "interactionId": interaction_id,
                "kind": { "type": "textDelta", "payload": { "text": "还在写" } },
            }),
        );
    }

    /// 标记一条交互还在途：在途表只放排队与运行中的行。
    fn mark_in_flight(&self, interaction_id: &str) {
        self.store
            .with_read_connection(|connection| {
                connection.execute(
                    "INSERT INTO work_items (
                         execution_id, session_id, kind, attempt, priority, inbox_id, status,
                         created_at_ms, updated_at_ms
                     ) VALUES (?1, ?2, 'turn', 0, 0, ?3, 'running', 1, 1)",
                    rusqlite::params![
                        format!("work-{}", uuid::Uuid::new_v4()),
                        self.session_id,
                        interaction_id,
                    ],
                )
            })
            .expect("登记在途工作项");
    }

    fn append(&mut self, kind: RuntimeEventKind, interaction_id: &str, payload: serde_json::Value) {
        let sequence = self.writer.next_sequence();
        let record = RuntimeEventRecord::new(sequence, 1_000 + sequence, kind, payload)
            .with_interaction(interaction_id);
        self.writer.append_batch(&[record]).expect("写入事件");
    }

    /// 落一条会话级事件：计划与作业都不属于任何一轮，因此不带交互归属。
    fn append_session_level(&mut self, kind: RuntimeEventKind, payload: serde_json::Value) {
        let sequence = self.writer.next_sequence();
        let record = RuntimeEventRecord::new(sequence, 1_000 + sequence, kind, payload);
        self.writer.append_batch(&[record]).expect("写入事件");
    }
}

/// 一份完整的 `turn_metrics` 载荷；缺字段会让反序列化整体失败。
fn turn_metrics_payload(
    interaction_id: &str,
    active_ms: u64,
    output_tokens: u64,
) -> serde_json::Value {
    serde_json::json!({
        "turnId": interaction_id,
        "activeMs": active_ms,
        "toolCalls": 2,
        "inputTokens": 5,
        "outputTokens": output_tokens,
        "reasoningTokens": 0,
        "cacheReadTokens": 0,
        "cacheWriteTokens": 0,
        "providerDurationMs": active_ms,
        "toolMs": 0,
        "backgroundJobsStarted": 0,
        "backgroundJobsRunning": 0,
    })
}

/// 页里有一轮还在跑时，其他轮次的答案与读数照样齐全，且不依赖放大的尾扫窗口。
#[test]
fn running_turn_keeps_the_page_facts_without_a_wider_window() {
    let mut fixture = Fixture::new();
    let projected = fixture.append_interaction("投影已经覆盖的轮次");
    let logged = fixture.append_interaction("只在事件日志里的轮次");
    let running = fixture.append_interaction("还在跑的轮次");
    // 投影行：答案与读数都不必再看事件日志。
    fixture.seed_projection(&projected, "投影里的结论", 900);
    // 事件日志里还留着同一轮更旧的结论：投影优先，不能被它覆盖。
    fixture.append_completed(&projected, "日志里被覆盖的结论", 700);
    // 只有事件日志的那一轮：定向补帧必须把答案与读数补上。
    fixture.append_completed(&logged, "日志里的结论", 650);
    // 在途轮次：没有 turn_completed，也没有 turn_metrics，只有一路流式帧。
    fixture.mark_in_flight(&running);
    for _ in 0..8 {
        fixture.append_running_frame(&running);
    }

    let ids = [projected.as_str(), logged.as_str(), running.as_str()];
    let facts = page_facts(&fixture.root, &fixture.session_id, &ids);

    assert_eq!(
        facts.answers.get(&projected).map(String::as_str),
        Some("投影里的结论"),
        "投影优先，事件日志里更旧的结论不得覆盖它"
    );
    assert_eq!(
        facts.answers.get(&logged).map(String::as_str),
        Some("日志里的结论"),
        "投影覆盖不到的一轮必须由事件日志补上"
    );
    assert!(
        !facts.answers.contains_key(&running),
        "还没收口的轮次没有最终正文"
    );
    assert_eq!(
        facts.readings.get(&projected).map(|row| row.active_ms),
        Some(Some(900)),
        "投影里的读数原样上屏"
    );
    assert_eq!(
        facts.readings.get(&logged).map(|row| row.output_tokens),
        Some(11),
        "定向补帧要补上逐轮读数"
    );
    assert!(
        !facts.readings.contains_key(&running),
        "在途轮次不参与定向补帧"
    );

    /*
     * 尾扫窗口压到 0：逐轮答案与读数必须一条不少，它们只能来自按交互定向补帧，
     * 与尾扫窗口的宽度无关。这条会话本来就没有计划帧，因此计划快照照旧为空。
     */
    let no_window = page_facts_within(&fixture.root, &fixture.session_id, &ids, 0);
    assert!(
        no_window.plan.is_none(),
        "这条会话没有计划帧，计划快照恒为空"
    );
    assert_eq!(
        no_window.answers.get(&logged).map(String::as_str),
        Some("日志里的结论"),
        "窗口为 0 时答案仍在，说明它来自定向补帧"
    );
    assert_eq!(
        no_window.readings.get(&logged).map(|row| row.output_tokens),
        Some(11),
        "窗口为 0 时读数仍在，说明它来自定向补帧"
    );
    assert_eq!(
        no_window.answers.get(&projected).map(String::as_str),
        Some("投影里的结论")
    );
}

/// 落一帧计划更新：`planUpdate` 是 wire 帧，载荷按协议类型解析。
fn append_plan_update(fixture: &mut Fixture, interaction_id: &str, title: &str) {
    fixture.append_session_level(
        RuntimeEventKind::Other("wire_event".to_owned()),
        serde_json::json!({
            "interactionId": interaction_id,
            "kind": {
                "type": "planUpdate",
                "payload": {
                    "partId": "plan:1",
                    "title": title,
                    "steps": [{ "step": "审计现状", "status": "completed" }],
                },
            },
        }),
    );
}

/// 落一条作业启动事件。
fn append_job_started(fixture: &mut Fixture, job_id: &str, kind: &str, started_at_ms: u64) {
    fixture.append_session_level(
        RuntimeEventKind::JobStarted,
        serde_json::json!({
            "jobId": job_id,
            "kind": kind,
            "owner": "fixture-owner",
            "startedAtMs": started_at_ms,
        }),
    );
}

/// 落一条作业收口事件。
fn append_job_completed(
    fixture: &mut Fixture,
    job_id: &str,
    status: &str,
    elapsed_ms: u64,
    exit_code: i32,
) {
    fixture.append_session_level(
        RuntimeEventKind::JobCompleted,
        serde_json::json!({
            "jobId": job_id,
            "status": status,
            "elapsedMs": elapsed_ms,
            "exitCode": exit_code,
        }),
    );
}

/// 垫一段远超尾窗宽度的过程帧，把此前写入的会话级事实推出任何固定尾窗。
fn pad_beyond_the_tail_window(fixture: &mut Fixture, interaction_id: &str) {
    for _ in 0..(SESSION_TAIL_MAX_FRAMES + 64) {
        fixture.append_running_frame(interaction_id);
    }
}

/// 计划与作业落在尾窗之外时仍必须读全：会话级事实不随尾窗宽度丢失。
#[test]
fn session_facts_survive_far_older_than_the_tail_window() {
    let mut fixture = Fixture::new();
    let projected = fixture.append_interaction("投影已经覆盖的轮次");
    fixture.seed_projection(&projected, "投影里的结论", 900);
    // 计划与作业都停在两千帧之前：只读尾窗的路径一定整段丢掉它们。
    append_plan_update(&mut fixture, &projected, "整理历史页");
    append_job_started(&mut fixture, "job-7", "shell", 1_500);
    append_job_completed(&mut fixture, "job-7", "completed", 4_200, 0);
    pad_beyond_the_tail_window(&mut fixture, &projected);
    let tail = fixture.append_interaction("尾部轮次");
    fixture.append_completed(&tail, "尾部结论", 500);

    let ids = [projected.as_str(), tail.as_str()];
    let facts = page_facts(&fixture.root, &fixture.session_id, &ids);

    assert_eq!(
        facts.plan.as_ref().map(|plan| plan.title.as_str()),
        Some("整理历史页"),
        "计划快照必须从尾部定向取到，不能因为落在两千帧之前就整段丢失"
    );
    let job = facts
        .jobs
        .iter()
        .find(|row| row.job_id == "job-7")
        .expect("作业必须出现在会话级清单里");
    assert_eq!(job.kind, "shell", "启动态给出作业类型");
    assert_eq!(job.started_at_ms, Some(1_500), "启动态给出启动时刻");
    assert_eq!(job.status.as_deref(), Some("completed"), "终态必须取到");
    assert_eq!(job.elapsed_ms, Some(4_200), "终态给出后端实测耗时");
    assert_eq!(job.exit_code, Some(0), "终态给出退出码");
}

/// 很久以前启动、最近才收口的作业：启动事实与终态必须合成同一行。
#[test]
fn job_started_far_back_merges_with_a_recent_completion() {
    let mut fixture = Fixture::new();
    let projected = fixture.append_interaction("投影已经覆盖的轮次");
    fixture.seed_projection(&projected, "投影里的结论", 900);
    append_job_started(&mut fixture, "job-9", "subagent", 2_500);
    pad_beyond_the_tail_window(&mut fixture, &projected);
    // 收口落在尾窗内，启动留在尾窗之外：两段事实必须按同一作业标识合并成一行。
    append_job_completed(&mut fixture, "job-9", "timeout", 9_000, 124);

    let facts = page_facts(&fixture.root, &fixture.session_id, &[projected.as_str()]);
    let matched: Vec<_> = facts
        .jobs
        .iter()
        .filter(|row| row.job_id == "job-9")
        .collect();
    assert_eq!(matched.len(), 1, "同一作业只能出现一行");
    let job = matched[0];
    assert_eq!(
        job.kind, "subagent",
        "启动态类型在尾窗之外，必须由定向取补上"
    );
    assert_eq!(job.started_at_ms, Some(2_500), "启动时刻同样来自定向取");
    assert_eq!(
        job.status.as_deref(),
        Some("timeout"),
        "终态来自尾窗内的收口帧"
    );
    assert_eq!(job.elapsed_ms, Some(9_000));
    assert_eq!(job.exit_code, Some(124));
}
