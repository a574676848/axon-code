use super::super::tests::{fixture_request, temp_workspace};
use super::*;
use crate::run_commands::run_progress::{RunProgressSink, RunProgressWireEvent};
use crate::run_commands::run_session_store::{RunSessionPartialInput, RunSessionRecord};
use crate::run_commands::run_thread_store::{mirror_session_started, prepare_run_session};
use axon_agent_runtime::{
    AgentRuntimeNoteKind, AgentTurnRuntime, AgentTurnToolDecision, CompressionPolicy,
    ProviderStreamAccumulator, ProviderStreamProtocol, ProviderStreamingTransportOutcome,
    WindowRolloverPolicyRuntime, WindowRolloverRuntime,
};
use axon_domain::TurnId;
use axon_protocol::wire::{WireContextRollover, WireEventKind};

use super::super::super::run_compaction_projection::{
    COMPACTION_COMPLETED_KIND, COMPACTION_STARTED_KIND,
};

const TEST_CONTEXT_WINDOW: u64 = 32_000;
const TEST_ROLLOVER_LIMIT: u64 = 20_000;
const TEST_OUTPUT_RESERVE: u64 = 4_096;

fn fixture_record(request: &RunRequest) -> RunSessionRecord {
    RunSessionRecord::from_partial(
        request,
        &RunSessionPartialInput {
            provider: "fixture".to_owned(),
            model: "fixture-model".to_owned(),
            tool_calls: Vec::new(),
            tool_receipts: Vec::new(),
            reasoning_messages: Vec::new(),
            turn_events: Vec::new(),
        },
    )
}

/// 与生产收口一致：把本轮最终回复写进 thread 轮次（`mirror_session_to_thread`）。
fn mirror_completed(request: &RunRequest, response: &str) {
    let mut record = fixture_record(request);
    record.response = response.to_owned();
    crate::run_commands::run_thread_store::mirror_session_to_thread(&request.config_path, &record)
        .unwrap();
}

fn initialize_session(request: &RunRequest) -> PathBuf {
    let record = fixture_record(request);
    // 与生产一致：起跑时就把 active turn 登记进 thread，换窗靠它定位当前轮。
    mirror_session_started(&request.config_path, &record).unwrap();
    resolved_axon_root(&request.config_path)
        .unwrap()
        .join("sessions")
        .join(&request.session_id)
}

fn restore(request: &RunRequest) -> Result<Vec<AgentModelMessage>, String> {
    let identities = crate::run_commands::RunIdentities::new(
        request.session_id.clone(),
        request.run_session_id.clone(),
    );
    let prepared = prepare_run_session(&request.config_path, &identities)?.unwrap();
    serde_json::from_str(&prepared.model_history_json).map_err(|error| error.to_string())
}

fn run_rollover(request: &RunRequest) -> AxonResult<()> {
    run_rollover_with_progress(request, None)
}

fn run_rollover_with_progress(
    request: &RunRequest,
    progress: Option<&RunProgressSink<'_>>,
) -> AxonResult<()> {
    let input = super::super::provider_streaming_transport_input(
        request,
        &request.model,
        &request.provider,
    )
    .unwrap();
    let mut rollover = InlineWindowRollover::new(request, progress);
    let compression = WindowRolloverRuntime::new(
        WindowRolloverPolicyRuntime::new(CompressionPolicy {
            trigger_tokens: TEST_ROLLOVER_LIMIT,
            context_window: TEST_CONTEXT_WINDOW,
        }),
        &mut rollover,
    );
    let result = AgentTurnRuntime.run(
        &input,
        |transport_request| {
            assert!(!transport_request.request.body_text.contains("不应复活的旧工具输出"));
            let mut stream = ProviderStreamAccumulator::new(ProviderStreamProtocol::OpenAiCompatible, Vec::new());
            stream.consume(r#"{"choices":[{"delta":{"content":"换窗后最终回复"},"finish_reason":"stop"}]}"#);
            ProviderStreamingTransportOutcome::AggregatedStream {
                http_status: 200, stream: Box::new(stream.finish().unwrap()),
            }
        },
        |_, _, _| -> AxonResult<AgentTurnToolDecision<()>> { panic!("不应调用工具") },
        None, Some(compression),
    ).map(|_| ());
    // 生产收口会把最终回复写进 thread；这里补上同一步，恢复窗口才有最后一条消息。
    mirror_completed(request, "换窗后最终回复");
    result
}

fn lifecycle_kinds(events: &[RunProgressWireEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match &event.event.kind {
            WireEventKind::TurnLifecycle(lifecycle) => lifecycle.kind.clone(),
            _ => None,
        })
        .collect()
}

fn context_rollovers(events: &[RunProgressWireEvent]) -> Vec<WireContextRollover> {
    events
        .iter()
        .filter_map(|event| match &event.event.kind {
            WireEventKind::ContextRollover(rollover) => Some(rollover.clone()),
            _ => None,
        })
        .collect()
}

/// 读到快照后别人又追加了一次：这是可恢复的序号冲突，重取序号重试一次即成功。
///
/// 旧行为把冲突当普通失败：整次换窗回滚、事件流报错，调用方还会抬高自动换窗门槛。
#[test]
fn sequence_conflict_retries_projection_once_and_completes_rollover() {
    let root = temp_workspace("inline-rollover-cas-retry");
    let request = request_with_rollover(root.clone());
    let session_dir = initialize_session(&request);
    SessionNotesRuntime::write_note_file(&session_dir, "progress.md", "已验证事实", 1).unwrap();
    let before = SessionNotes::load_from_session(&session_dir)
        .unwrap()
        .expect("会话便签必须已初始化");

    // 第一次投影用过期序号，制造真实 CAS 冲突；重试会重取快照。
    super::FORCE_STALE_SEQUENCE.with(|counter| counter.set(1));
    run_rollover(&request).expect("序号冲突必须被重试吸收，不得判成换窗失败");

    let notes = SessionNotes::load_from_session(&session_dir)
        .unwrap()
        .expect("换窗后便签仍在");
    assert_eq!(
        notes.window_number,
        before.window_number + 1,
        "重试成功后窗口号必须真的推进，而不是回滚"
    );
    let projected = axon_session::ThreadService::new(axon_session::JsonlThreadEventStore)
        .read_thread(&root, &request.run_session_id)
        .expect("thread 必须可读");
    assert!(
        projected.compacted_history.is_some(),
        "重试必须把活动窗口投影真的装进 thread"
    );
    std::fs::remove_dir_all(root).unwrap();
}

/// 只重试一次：连续两次冲突仍是失败，且必须整体回滚，不能留下半提交窗口。
#[test]
fn repeated_sequence_conflicts_still_roll_back_the_rollover() {
    let root = temp_workspace("inline-rollover-cas-retry-exhausted");
    let request = request_with_rollover(root.clone());
    let session_dir = initialize_session(&request);
    SessionNotesRuntime::write_note_file(&session_dir, "progress.md", "已验证事实", 1).unwrap();
    let before = SessionNotes::load_from_session(&session_dir)
        .unwrap()
        .expect("会话便签必须已初始化");

    // 两次都注入陈旧序号：首次失败进入重试，重试再失败即判定本轮换窗失败。
    super::FORCE_STALE_SEQUENCE.with(|counter| counter.set(2));
    let error = commit_rollover_directly(&request).expect_err("连续冲突必须如实失败");

    assert!(
        is_thread_sequence_conflict(&error),
        "失败原因必须仍指向序号冲突：{error}"
    );
    let after = SessionNotes::load_from_session(&session_dir)
        .unwrap()
        .expect("回滚后便签仍在");
    assert_eq!(after.window_number, before.window_number, "窗口号必须回滚");
    assert_eq!(
        after.current_window_id, before.current_window_id,
        "窗口身份必须回滚"
    );
    assert!(
        WindowRolloverEngine::read_archive_shard(&session_dir, 1)
            .map(|shard| shard.is_empty())
            .unwrap_or(true),
        "归档分片必须被回滚：{error}"
    );
    std::fs::remove_dir_all(root).unwrap();
}

/// 非冲突错误不重试：注入的投影失败只该出现一次，仍然整体回滚。
#[test]
fn non_conflict_projection_failures_are_not_retried() {
    let root = temp_workspace("inline-rollover-no-retry");
    let request = request_with_rollover(root.clone());
    let session_dir = initialize_session(&request);
    SessionNotesRuntime::write_note_file(&session_dir, "progress.md", "已验证事实", 1).unwrap();
    let before = SessionNotes::load_from_session(&session_dir)
        .unwrap()
        .expect("会话便签必须已初始化");

    super::FORCE_PROJECTION_FAILURE.with(|flag| flag.set(true));
    // 只注入一次：如果代码错误地在非冲突错误上重试，第二次会成功，这里就拿不到 Err。
    let error = commit_rollover_directly(&request).expect_err("非冲突投影失败必须直接失败");
    assert!(error.contains("活动窗口投影失败"), "{error}");
    let after = SessionNotes::load_from_session(&session_dir)
        .unwrap()
        .expect("回滚后便签仍在");
    assert_eq!(after.window_number, before.window_number);
    std::fs::remove_dir_all(root).unwrap();
}

/// 直接走 prepare → commit，绕开 run 的模型请求断言，专门观察提交阶段的成败。
fn commit_rollover_directly(request: &RunRequest) -> Result<(), String> {
    let messages = &request.model_payload.messages;
    let mut rollover = InlineWindowRollover::new(request, None);
    let checkpoint = rollover
        .prepare(messages, TurnId(1))
        .expect("prepare 必须成功");
    let policy_runtime = WindowRolloverPolicyRuntime::new(CompressionPolicy {
        trigger_tokens: TEST_ROLLOVER_LIMIT,
        context_window: TEST_CONTEXT_WINDOW,
    });
    let window = policy_runtime.provider_visible_window(messages);
    let replacement = policy_runtime
        .build_validated_replacement(
            &window,
            &checkpoint,
            WindowRolloverValidationMode::Requested,
        )
        .expect("replacement 必须通过校验");
    rollover
        .commit(messages, &checkpoint, &replacement)
        .map_err(|error| format!("{error:?}"))
}

fn request_with_rollover(root: PathBuf) -> RunRequest {
    let mut request = fixture_request(root);
    request.provider.name = "fixture".to_owned();
    request.provider.base_url = "https://provider.example/v1".to_owned();
    request.model.id = "fixture-model".to_owned();
    request.model.provider = request.provider.name.clone();
    request.model.context_window = TEST_CONTEXT_WINDOW;
    request.model.max_output_tokens = Some(TEST_OUTPUT_RESERVE);
    request.model_payload.messages = vec![
        AgentModelMessage::text("user", "原始目标"),
        AgentModelMessage::text("assistant", "不应复活的旧工具输出"),
        AgentModelMessage::text("user", "当前输入"),
        AgentModelMessage::runtime_note(AgentRuntimeNoteKind::ContextRolloverRequest, "开启新窗口"),
    ];
    request
}

#[test]
fn two_inline_rollovers_restore_only_active_window_and_keep_archives() {
    let root = temp_workspace("two-inline-rollovers");
    let mut request = request_with_rollover(root.clone());
    let session_dir = initialize_session(&request);
    SessionNotesRuntime::write_note_file(
        &session_dir,
        "progress.md",
        "已验证事实：保留原始目标",
        1,
    )
    .unwrap();
    run_rollover(&request).unwrap();
    let first = restore(&request).unwrap();
    assert!(
        !first
            .iter()
            .any(|message| message.content.contains("不应复活的旧工具输出"))
    );
    assert!(
        first
            .iter()
            .any(|message| message.is_real_user_turn() && message.content == "当前输入")
    );
    assert_eq!(first.last().unwrap().content, "换窗后最终回复");
    request.invocation_id = "second-window-invocation".to_owned();
    request.prompt = "第二个窗口输入".to_owned();
    request.model_payload.messages = first;
    request.model_payload.messages.extend([
        AgentModelMessage::text("assistant", "第二个窗口的旧进度"),
        AgentModelMessage::text("user", &request.prompt),
        AgentModelMessage::runtime_note(
            AgentRuntimeNoteKind::ContextRolloverRequest,
            "再次开启新窗口",
        ),
    ]);
    initialize_session(&request);
    SessionNotesRuntime::append_note_file(&session_dir, "progress.md", "第二窗口进度已验证", 2)
        .unwrap();
    run_rollover(&request).unwrap();
    let restored = restore(&request).unwrap();
    assert!(
        !restored
            .iter()
            .any(|message| message.content == "第二个窗口的旧进度")
    );
    assert!(
        !restored
            .iter()
            .any(|message| message.is_real_user_turn() && message.content == "当前输入")
    );
    assert!(
        restored
            .iter()
            .any(|message| message.is_real_user_turn() && message.content == request.prompt)
    );
    assert_eq!(restored.last().unwrap().content, "换窗后最终回复");
    let notes = SessionNotes::load_from_session(&session_dir)
        .unwrap()
        .unwrap();
    assert_eq!(notes.window_number, 3);
    for window in [1, 2] {
        assert!(
            !WindowRolloverEngine::read_archive_shard(&session_dir, window)
                .unwrap()
                .is_empty()
        );
    }
    let detail = axon_session::ThreadService::new(axon_session::JsonlThreadEventStore)
        .read_thread(&root, &request.session_id)
        .unwrap();
    assert_eq!(detail.turns.len(), 2, "两次收尾各落一轮，不得重复开轮");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rollover_falls_back_to_the_active_turn_when_part_id_changed() {
    let root = temp_workspace("inline-projection-backfill");
    let mut request = request_with_rollover(root.clone());
    let session_dir = initialize_session(&request);
    SessionNotesRuntime::write_note_file(&session_dir, "progress.md", "已验证事实", 1).unwrap();
    let previous = SessionNotes::load_from_session(&session_dir)
        .unwrap()
        .unwrap();
    // 本轮收尾还没发生，assistant_part_id 与起跑时不同：换窗必须回退到 active turn。
    request.invocation_id = "unregistered-invocation".to_owned();
    run_rollover(&request).unwrap();
    let notes = SessionNotes::load_from_session(&session_dir)
        .unwrap()
        .unwrap();
    // 回退命中了起跑时登记的 active turn：窗口身份真的推进、旧窗口真的归档。
    assert_eq!(notes.window_number, previous.window_number + 1);
    assert_ne!(notes.current_window_id, previous.current_window_id);
    assert_ne!(
        notes.last_rollover_lifecycle_id,
        previous.last_rollover_lifecycle_id
    );
    assert!(
        !WindowRolloverEngine::list_archive_shards(&session_dir)
            .unwrap()
            .is_empty()
    );
    let projected = axon_session::ThreadService::new(axon_session::JsonlThreadEventStore)
        .read_thread(&root, &request.session_id)
        .unwrap();
    assert!(
        projected.compacted_history.is_some(),
        "活动窗口投影必须写进 thread"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn advanced_window_without_projection_is_rejected_instead_of_rebuilding_old_chat() {
    let root = temp_workspace("missing-window-projection");
    let request = request_with_rollover(root.clone());
    let session_dir = initialize_session(&request);
    SessionNotesRuntime::write_note_file(&session_dir, "progress.md", "已验证事实", 1).unwrap();
    let notes = SessionNotes::load_from_session(&session_dir)
        .unwrap()
        .unwrap();
    WindowRolloverEngine::archive_active_window(
        &session_dir,
        notes.window_number,
        notes.current_window_id,
        notes.revision,
        "missing-projection",
        &request.model_payload.messages,
        2,
    )
    .unwrap();
    assert!(
        restore(&request)
            .unwrap_err()
            .contains("拒绝恢复旧聊天历史")
    );
    assert!(
        !WindowRolloverEngine::read_archive_shard(&session_dir, 1)
            .unwrap()
            .is_empty()
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn inline_rollover_projects_started_completed_and_context_rollover() {
    let root = temp_workspace("inline-rollover-projection");
    let request = request_with_rollover(root.clone());
    let session_dir = initialize_session(&request);
    SessionNotesRuntime::write_note_file(&session_dir, "progress.md", "已验证事实", 1).unwrap();

    let mut events = Vec::new();
    {
        let mut emit = |event: RunProgressWireEvent| events.push(event);
        let sink = RunProgressSink::with_session_id(&mut emit, request.session_id.clone());
        run_rollover_with_progress(&request, Some(&sink)).unwrap();
    }

    let kinds = lifecycle_kinds(&events);
    assert_eq!(
        kinds,
        vec![
            COMPACTION_STARTED_KIND.to_owned(),
            COMPACTION_COMPLETED_KIND.to_owned()
        ],
        "内联轮转必须投影开始与完成，且完成携带量化结果"
    );
    let rollovers = context_rollovers(&events);
    let rollover = rollovers.first().expect("必须发布窗口轮转权威事实");
    assert!(
        !rollover.window_id.trim().is_empty(),
        "轮转事实必须携带窗口身份"
    );
    assert!(
        rollover.archived_messages_count > 0,
        "归档条数不得为 0：{rollover:?}"
    );
    assert!(
        rollover.notes_injected,
        "内联轮转必须注入 checkpoint 交接投影"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn handoff_deferral_is_not_projected_as_failure() {
    let root = temp_workspace("inline-rollover-handoff-deferral");
    let request = request_with_rollover(root.clone());
    initialize_session(&request);

    let mut events = Vec::new();
    {
        let mut emit = |event: RunProgressWireEvent| events.push(event);
        let sink = RunProgressSink::with_session_id(&mut emit, request.session_id.clone());
        let mut rollover = InlineWindowRollover::new(&request, Some(&sink));
        // 便签未写入时门禁必然失败：这是可自愈的推迟，不是用户可见的失败。
        assert!(
            rollover
                .validate_memory_handoff(WindowRolloverValidationMode::Automatic)
                .is_err(),
            "未写入便签的会话必须被换窗门禁拒绝"
        );
        rollover.abort();
    }

    assert!(
        lifecycle_kinds(&events).is_empty(),
        "便签门禁拒绝是可自愈的推迟，不得发布失败行；真正的失败仍由 prepare 与 commit 路径投影"
    );
    std::fs::remove_dir_all(root).unwrap();
}

/// 归档已写、投影失败时必须整体回滚：窗口身份、notes 与归档分片都不能半提交。
#[test]
fn projection_failure_rolls_back_archive_and_window_identity() {
    let root = temp_workspace("inline-rollover-projection-rollback");
    let request = request_with_rollover(root.clone());
    let session_dir = initialize_session(&request);
    SessionNotesRuntime::write_note_file(&session_dir, "progress.md", "已验证事实", 1).unwrap();
    let before = SessionNotes::load_from_session(&session_dir)
        .unwrap()
        .expect("会话便签必须已初始化");

    let messages = &request.model_payload.messages;
    let mut rollover = InlineWindowRollover::new(&request, None);
    let checkpoint = rollover
        .prepare(messages, TurnId(1))
        .expect("prepare 必须成功");
    let policy_runtime = WindowRolloverPolicyRuntime::new(CompressionPolicy {
        trigger_tokens: TEST_ROLLOVER_LIMIT,
        context_window: TEST_CONTEXT_WINDOW,
    });
    let window = policy_runtime.provider_visible_window(messages);
    let replacement = policy_runtime
        .build_validated_replacement(
            &window,
            &checkpoint,
            WindowRolloverValidationMode::Requested,
        )
        .expect("replacement 必须通过校验");

    super::FORCE_PROJECTION_FAILURE.with(|flag| flag.set(true));
    let error = rollover
        .commit(messages, &checkpoint, &replacement)
        .expect_err("投影失败必须返回错误");
    let message = format!("{error:?}");
    assert!(
        message.contains("活动窗口投影失败"),
        "失败原因必须点明投影步骤：{message}"
    );

    let after = SessionNotes::load_from_session(&session_dir)
        .unwrap()
        .expect("回滚后便签必须仍在");
    assert_eq!(
        after.window_number, before.window_number,
        "窗口号必须回滚到换窗前：{message}"
    );
    assert_eq!(
        after.current_window_id, before.current_window_id,
        "窗口身份必须回滚到换窗前：{message}"
    );
    assert!(
        WindowRolloverEngine::read_archive_shard(&session_dir, 1)
            .map(|shard| shard.is_empty())
            .unwrap_or(true),
        "归档分片必须被回滚，不能留下半提交窗口：{message}"
    );
    std::fs::remove_dir_all(root).unwrap();
}

/// 换窗成功必须把摘要来源记回便签，后续窗口才能判断摘要是否为模型原话。
#[test]
fn successful_rollover_records_projected_checkpoint_source() {
    let root = temp_workspace("inline-rollover-source-projection");
    let request = request_with_rollover(root.clone());
    let session_dir = initialize_session(&request);
    SessionNotesRuntime::write_note_file(&session_dir, "progress.md", "已验证事实", 1).unwrap();

    run_rollover_with_progress(&request, None).unwrap();

    let notes = SessionNotes::load_from_session(&session_dir)
        .unwrap()
        .expect("换窗后便签必须仍在");
    assert_eq!(
        notes.checkpoint_source,
        Some(CheckpointSource::Projection),
        "没有 checkpoint.md 时必须记录为投影来源"
    );
    assert_eq!(
        notes.checkpoint_window_number,
        Some(1),
        "摘要对应的应是换窗前的窗口号"
    );
    std::fs::remove_dir_all(root).unwrap();
}

/// 模型写入便签时，来源必须记为 Model，而不是笼统的投影。
#[test]
fn checkpoint_note_is_recorded_as_model_source() {
    let root = temp_workspace("inline-rollover-source-model");
    let request = request_with_rollover(root.clone());
    let session_dir = initialize_session(&request);
    let submitted = "P0:\nTASK:\n模型提交的任务\n\nCONSTRAINTS:\n- 约束\n\nVERIFIED:\n- 事实\n\nOPEN:\n- 无\n\nTOOLS:\n- none";
    SessionNotesRuntime::write_note_file(&session_dir, "checkpoint.md", submitted, 1).unwrap();
    SessionNotesRuntime::write_note_file(&session_dir, "progress.md", "已验证事实", 1).unwrap();

    run_rollover_with_progress(&request, None).unwrap();

    let notes = SessionNotes::load_from_session(&session_dir)
        .unwrap()
        .expect("换窗后便签必须仍在");
    assert_eq!(
        notes.checkpoint_source,
        Some(CheckpointSource::Model),
        "模型提交的摘要必须记录为 Model 来源"
    );
    std::fs::remove_dir_all(root).unwrap();
}

/// 摘要来源落库失败时，Thread 投影不得先于它写入。
///
/// 整体回滚只覆盖 session 目录，撤销不了已经写进 thread 的快照：顺序一旦颠倒，
/// 来源失败时 thread 已指向新窗口、notes 却回滚到旧窗口，恢复侧会拿新窗口历史
/// 去配旧窗口的边界。
#[test]
fn source_failure_happens_before_thread_projection() {
    let root = temp_workspace("inline-rollover-source-failure-order");
    let request = request_with_rollover(root.clone());
    let session_dir = initialize_session(&request);
    SessionNotesRuntime::write_note_file(&session_dir, "progress.md", "已验证事实", 1).unwrap();

    let messages = &request.model_payload.messages;
    let mut rollover = InlineWindowRollover::new(&request, None);
    let checkpoint = rollover
        .prepare(messages, TurnId(1))
        .expect("prepare 必须成功");
    let policy_runtime = WindowRolloverPolicyRuntime::new(CompressionPolicy {
        trigger_tokens: TEST_ROLLOVER_LIMIT,
        context_window: TEST_CONTEXT_WINDOW,
    });
    let window = policy_runtime.provider_visible_window(messages);
    let replacement = policy_runtime
        .build_validated_replacement(
            &window,
            &checkpoint,
            WindowRolloverValidationMode::Requested,
        )
        .expect("replacement 必须通过校验");

    super::FORCE_SOURCE_FAILURE.with(|flag| flag.set(true));
    rollover
        .commit(messages, &checkpoint, &replacement)
        .expect_err("来源落库失败必须让整次提交流产");

    let notes = SessionNotes::load_from_session(&session_dir)
        .unwrap()
        .expect("回滚后便签必须仍在");
    assert_eq!(notes.window_number, 1, "窗口号必须回滚到换窗前");
    assert!(
        notes.checkpoint_source.is_none(),
        "来源字段不得留下半状态：{notes:?}"
    );

    let detail = axon_session::ThreadService::new(axon_session::JsonlThreadEventStore)
        .read_thread(&root, &request.run_session_id)
        .expect("thread 必须仍可读");
    assert!(
        detail.compacted_history.is_none(),
        "来源落库排在投影之前：它失败时 thread 不得已带压缩投影"
    );
    std::fs::remove_dir_all(root).unwrap();
}

/// 会话身份与 Thread 身份是两个空间：`session_id` 是会话 ULID，`run_session_id` 才是
/// Thread 事件流的键。
///
/// 夹具默认把两者设成同一个值，换窗因此曾把会话 ULID 当成 Thread 键：真实会话上每次轮转
/// 都在读 Thread 时失败并整轮回滚，事件流只报「未通过门禁」，24 次全部失败而原因不明。
#[test]
fn rollover_targets_the_run_window_as_thread_identity() {
    let root = temp_workspace("inline-rollover-thread-identity");
    let mut request = request_with_rollover(root.clone());
    // 必须放在初始化之前：起跑登记与轮转投影读的都是这一刻的 thread 身份。
    request.run_session_id = format!("{}-thread", request.session_id);
    let session_dir = initialize_session(&request);
    SessionNotesRuntime::write_note_file(&session_dir, "progress.md", "已验证事实", 1).unwrap();

    run_rollover(&request).expect("两个身份不同时，换窗必须成功");

    let notes = SessionNotes::load_from_session(&session_dir)
        .unwrap()
        .expect("换窗后便签必须仍在");
    assert_eq!(notes.window_number, 2, "窗口号必须真的推进");

    let projected = axon_session::ThreadService::new(axon_session::JsonlThreadEventStore)
        .read_thread(&root, &request.run_session_id)
        .expect("活动窗口投影必须写进 run_session_id 对应的 thread");
    assert!(
        projected.compacted_history.is_some(),
        "换窗后 thread 必须携带活动窗口投影"
    );
    assert!(
        axon_session::ThreadService::new(axon_session::JsonlThreadEventStore)
            .read_thread(&root, &request.session_id)
            .is_err(),
        "会话 ULID 不是 Thread 键，轮转不得拿它去读 thread"
    );
    std::fs::remove_dir_all(root).unwrap();
}
