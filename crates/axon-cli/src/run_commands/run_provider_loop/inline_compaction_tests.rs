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
use axon_protocol::wire::{WireContextRollover, WireEventKind};

use super::super::super::run_compaction_projection::{
    COMPACTION_COMPLETED_KIND, COMPACTION_FAILED_KIND, COMPACTION_STARTED_KIND,
};

const TEST_CONTEXT_WINDOW: u64 = 32_000;
const TEST_ROLLOVER_LIMIT: u64 = 20_000;
const TEST_OUTPUT_RESERVE: u64 = 4_096;

fn initialize_session(request: &RunRequest) -> PathBuf {
    let record = RunSessionRecord::from_partial(
        request,
        &RunSessionPartialInput {
            provider: "fixture".to_owned(),
            model: "fixture-model".to_owned(),
            tool_calls: Vec::new(),
            tool_receipts: Vec::new(),
            reasoning_messages: Vec::new(),
            turn_events: Vec::new(),
        },
    );
    crate::run_commands::run_rollout_store::commit_session_to_rollout(
        &request.config_path,
        &record,
        "窗口测试",
    )
    .unwrap();
    mirror_session_started(&request.config_path, &record).unwrap();
    resolved_axon_root(&request.config_path)
        .unwrap()
        .join("sessions")
        .join(&request.session_id)
}

fn restore(request: &RunRequest) -> Result<Vec<AgentModelMessage>, String> {
    let command = crate::RunCommand {
        prompt: "继续".to_owned(),
        model: None,
        reasoning_effort: None,
        format: None,
        continue_last: false,
        session: Some(request.session_id.clone()),
        file: Vec::new(),
        timeout: None,
        tool: Vec::new(),
    };
    let prepared = prepare_run_session(&request.config_path, &command, None)?.unwrap();
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
    AgentTurnRuntime.run(
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
    ).map(|_| ())
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
    let db = crate::run_commands::resolved_rollout_db_path(&request.config_path).unwrap();
    let count = rusqlite::Connection::open(db)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM rollout_turns WHERE session_id = ?1",
            rusqlite::params![request.session_id],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    assert_eq!(count, 2);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn missing_current_turn_rolls_back_archive_and_window_identity() {
    let root = temp_workspace("inline-projection-rollback");
    let mut request = request_with_rollover(root.clone());
    let session_dir = initialize_session(&request);
    SessionNotesRuntime::write_note_file(&session_dir, "progress.md", "已验证事实", 1).unwrap();
    let previous = SessionNotes::load_from_session(&session_dir)
        .unwrap()
        .unwrap();
    request.invocation_id = "unregistered-invocation".to_owned();
    run_rollover(&request).unwrap();
    let notes = SessionNotes::load_from_session(&session_dir)
        .unwrap()
        .unwrap();
    assert_eq!(notes.window_number, previous.window_number);
    assert_eq!(notes.current_window_id, previous.current_window_id);
    assert_eq!(
        notes.last_rollover_lifecycle_id,
        previous.last_rollover_lifecycle_id
    );
    assert!(
        WindowRolloverEngine::list_archive_shards(&session_dir)
            .unwrap()
            .is_empty()
    );
    let window = crate::run_commands::run_rollout_store::load_rollout_history_window(
        &request.config_path,
        &request.session_id,
    )
    .unwrap();
    assert!(window.compaction.is_none());
    assert_eq!(window.turns.len(), 1);
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
fn handoff_failure_before_prepare_is_projected_instead_of_silent() {
    let root = temp_workspace("inline-rollover-handoff-failure");
    let request = request_with_rollover(root.clone());
    initialize_session(&request);

    let mut events = Vec::new();
    {
        let mut emit = |event: RunProgressWireEvent| events.push(event);
        let sink = RunProgressSink::with_session_id(&mut emit, request.session_id.clone());
        let mut rollover = InlineWindowRollover::new(&request, Some(&sink));
        // 便签未写入时门禁必然失败，此时尚未进入 prepare：仍必须投影失败事实。
        assert!(
            rollover
                .validate_memory_handoff(WindowRolloverValidationMode::Automatic)
                .is_err(),
            "未写入便签的会话必须被换窗门禁拒绝"
        );
        rollover.abort();
    }

    assert_eq!(
        lifecycle_kinds(&events),
        vec![COMPACTION_FAILED_KIND.to_owned()],
        "prepare 之前的门禁失败此前完全静默，必须补上失败投影"
    );
    std::fs::remove_dir_all(root).unwrap();
}
