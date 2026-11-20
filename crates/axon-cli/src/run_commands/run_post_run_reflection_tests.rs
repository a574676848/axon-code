use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use axon_agent_runtime::{
    AgentModelMessage, AgentModelPayload, AgentTurnEvent, AgentTurnEventKind, ExplicitSignals,
    ProviderToolCall, ReflectionTier, SEMANTIC_SALIENCE_SCHEMA_VERSION, SelfIterationRuntime,
    SemanticMemoryCandidateType, SemanticSalienceAssessment, SemanticSalienceSignals,
    TurnOutcomeEnvelope, TurnTrace,
};
use axon_config::{AxonConfig, ChannelBalancerConfig, ModelConfig, ProviderConfig};
use axon_domain::{SessionId, TurnId};
use axon_memory::MemoryRuntimeQualityStore;
use axon_memory_read::init_memory_orchestrator;
use axon_protocol::wire::WireEventKind;

use super::{
    RunRequest,
    run_execution::{RunJsonEvent, RunResponse},
    run_json_wire_events::CompletedRunWireEventsInput,
    run_post_run_governance_worker::run_governance_worker_with_request,
    run_progress::RunProgressSink,
    run_reflection_batch::{BatchAdmission, admit_reflection_batch, reflection_cards},
    run_self_iteration_subagent::{
        SelfIterationExecutionOptions, execute_self_iteration_subagent, self_iteration_input,
    },
    run_subagent_executor::{
        RunSubagentExecutionInput, RunSubagentExecutionOutput, RunSubagentExecutionStatus,
        RunSubagentExecutor,
    },
    run_tool_loop::RunToolReceipt,
    run_turn_outcome::build_turn_outcome,
    run_turn_spec::TurnSpec,
    run_turn_terminal::complete_turn,
};

#[test]
fn ordinary_completed_turn_does_not_delegate_deep_reflection() {
    let root = test_root("trivial-self-iteration");
    let request = request_fixture(root.clone(), root.join("config.toml"));
    let response = response_with_receipts(&[]);

    assert!(self_iteration_labels(&request, &response).is_empty());

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn selected_project_skill_without_execution_evidence_is_not_reflected() {
    let root = test_root("short-skill-reflection");
    let mut request = request_fixture(root.clone(), root.join("config.toml"));
    request.selected_skill_name = Some("release".to_owned());
    request.selected_skill_source_ref = Some("skill://axon-project-skills/release".to_owned());
    let response = response_with_receipts(&[]);

    assert!(self_iteration_labels(&request, &response).is_empty());

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn completed_run_governs_reflection_off_main_path() {
    let root = test_root("low-risk-post-run");
    let config_path = root.join("runtime").join("config.toml");
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    let skill_file = root
        .join(".axon")
        .join("skills")
        .join("release")
        .join("SKILL.md");
    std::fs::create_dir_all(skill_file.parent().unwrap()).unwrap();
    std::fs::write(
        &skill_file,
        "---\nname: release\ndescription: 验证发布流程\n---\n\n# Release\n",
    )
    .unwrap();
    let captured = Arc::new(Mutex::new(Vec::new()));
    let mut request = request_fixture_with_executor(
        root.clone(),
        config_path.clone(),
        Arc::new(CapturingSelfIterationExecutor {
            captured: captured.clone(),
        }),
    );
    request.selected_skill_name = Some("release".to_owned());
    request.selected_skill_source_ref = Some("skill://axon-project-skills/release".to_owned());
    let mut response = response_with_receipts(&[
        receipt(
            "call-skill",
            "builtin.invoke_skill",
            "completed",
            "Skill instructions loaded",
            false,
        ),
        receipt(
            "call-edit",
            "builtin.edit",
            "completed",
            "patched files",
            true,
        ),
        receipt(
            "call-test",
            "builtin.shell_command",
            "completed",
            "validation completed",
            false,
        ),
    ]);
    response.turn_events.extend(provider_turn_events(7));
    let mut events = Vec::new();
    let mut collect = |event| events.push(event);
    let sink = RunProgressSink::with_session_id(&mut collect, "test-session".to_owned());

    let completed = complete_turn(&TurnSpec::primary(), &request, response);
    drop(sink);

    let receipt = completed
        .post_run_governance
        .as_ref()
        .expect("完成后必须启动 post-run governance");
    assert_eq!(receipt.status, "started");
    let status_path = receipt.status_path.as_ref().expect("必须返回 statusPath");
    assert_eq!(
        run_governance_worker_with_request(&config_path, &AxonConfig::default(), &request),
        Ok(1)
    );
    let json_event = RunJsonEvent::completed(CompletedRunWireEventsInput {
        request: &request,
        response: &completed.response,
        session_title: completed.session_title.as_deref(),
        memory_usefulness: completed.memory_usefulness.as_ref(),
        memory_autonomy: completed.memory_autonomy.as_ref(),
        post_run_governance: completed.post_run_governance.as_ref(),
        recorded_at_ms: super::run_execution::wire_recorded_at_ms(),
        total_elapsed_ms: 0,
    });
    let json_value = serde_json::to_value(json_event).unwrap();
    assert_eq!(json_value["status"], "success");
    let wire_events = json_value["wireEvents"]
        .as_array()
        .expect("必须输出 wireEvents");
    assert_eq!(
        wire_events
            .iter()
            .filter(|event| event["kind"]["type"] == "toolResult")
            .count(),
        3
    );
    let governance = wire_events
        .iter()
        .find(|event| event["kind"]["type"] == "governanceStatus")
        .expect("必须输出 governanceStatus wire event");
    assert_eq!(governance["kind"]["payload"]["status"], "started");
    assert_eq!(governance["kind"]["payload"]["statusPath"], *status_path);

    assert_eq!(
        governance_job_status(std::path::Path::new(status_path), &receipt.job_id),
        "completed"
    );

    // 终态记账必须给这一轮留下指标：缺行时界面上的耗时与用量没有基线。
    let store = crate::run_commands::run_agent_store::shared_runtime_store(&config_path)
        .expect("共享 runtime 库必须可解析");
    let session_id =
        axon_domain::DaemonSessionId::new(request.session_id.clone()).expect("合法会话标识");
    let replayed = axon_session::EventLogStore::replay_events(&store, &session_id, 0, 64)
        .expect("必须能读回本轮事件");
    let metrics = replayed
        .iter()
        .find(|event| event.event_type == "turn_metrics")
        .unwrap_or_else(|| {
            panic!(
                "完成一轮后必须落下 turn_metrics，实际事件类型 {:?}",
                replayed
                    .iter()
                    .map(|event| event.event_type.as_str())
                    .collect::<Vec<_>>()
            )
        });
    // 轮次键必须就是这一轮的 interaction 标识：客户端按 	urnId === turn.id 匹配收尾指标帧。
    let payload: serde_json::Value =
        serde_json::from_str(&metrics.payload_json).expect("turn_metrics 载荷");
    assert_eq!(
        payload["turnId"].as_str(),
        Some(request.interaction_id.as_str()),
        "逐轮读数必须挂在 interaction 标识上，而不是消息分段标识"
    );

    let kinds = events
        .iter()
        .map(|event| event.event.kind.clone())
        .collect::<Vec<_>>();
    assert!(
        kinds.iter().all(|kind| !matches!(
            kind,
            WireEventKind::SkillStatus(_) | WireEventKind::ReviewStatus(_)
        )),
        "反思链路不应产生主路径技能状态或人审状态事件"
    );
    assert!(captured_labels_contain(&captured, &["memory_update"]));
    let assessor_input = captured_input_ending_with(&captured, "semantic_salience")
        .expect("post-run 应先执行 semantic salience assessor");
    assert!(!assessor_input.tools_enabled);
    assert!(assessor_input.structured_output.is_some());
    assert!(
        assessor_input
            .system_prompt_override
            .as_deref()
            .is_some_and(|prompt| prompt.contains("只输出符合 schema 的 JSON"))
    );
    let orchestrator = init_memory_orchestrator(&config_path, &AxonConfig::default()).unwrap();
    let quality = MemoryRuntimeQualityStore::open(orchestrator.config())
        .unwrap()
        .report()
        .unwrap();
    assert_eq!(quality.observation_count, 1);
    assert_eq!(quality.semantic_assessments, 1);
    assert_eq!(quality.reflection_attempts, 1);
    // 该 executor 不写记忆，反思产出恒为 0；与线上实测完全一致。
    assert_eq!(quality.reflection_yields, 0);
    assert_eq!(quality.reflection_yield_basis_points, 0);
    assert_eq!(quality.false_memory_rejections, 0);
    assert_no_running_subagent_sessions(&request);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn completed_run_keeps_high_risk_selected_skill_untouched() {
    let root = test_root("high-risk-post-run");
    let config_path = root.join("runtime").join("config.toml");
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    let skill_file = root
        .join(".axon")
        .join("skills")
        .join("release")
        .join("SKILL.md");
    std::fs::create_dir_all(skill_file.parent().unwrap()).unwrap();
    let original_skill = "---\nname: release\ndescription: 验证发布流程\n---\n\n# Release\n";
    std::fs::write(&skill_file, original_skill).unwrap();
    let captured = Arc::new(Mutex::new(Vec::new()));
    let mut request = request_fixture_with_executor(
        root.clone(),
        config_path.clone(),
        Arc::new(CapturingSelfIterationExecutor {
            captured: captured.clone(),
        }),
    );
    request.selected_skill_name = Some("release".to_owned());
    request.selected_skill_source_ref = Some("skill://axon-project-skills/release".to_owned());
    let response = response_with_receipts(&[
        receipt(
            "call-skill",
            "builtin.invoke_skill",
            "completed",
            "Skill instructions loaded",
            false,
        ),
        receipt(
            "call-edit",
            "builtin.edit",
            "completed",
            "authorization: bearer secret",
            true,
        ),
        receipt(
            "call-test",
            "builtin.shell_command",
            "completed",
            "validation completed",
            false,
        ),
    ]);
    let mut events = Vec::new();
    let mut collect = |event| events.push(event);
    let sink = RunProgressSink::with_session_id(&mut collect, "test-session".to_owned());

    let completed = complete_turn(&TurnSpec::primary(), &request, response);
    drop(sink);

    let receipt = completed
        .post_run_governance
        .as_ref()
        .expect("完成后必须启动 post-run governance");
    assert_eq!(receipt.status, "started");
    assert!(receipt.status_path.is_some(), "必须返回 statusPath");
    assert_eq!(
        run_governance_worker_with_request(&config_path, &AxonConfig::default(), &request),
        Ok(1)
    );
    assert_eq!(
        std::fs::read_to_string(&skill_file).unwrap(),
        original_skill
    );
    assert!(
        events
            .iter()
            .all(|event| !matches!(event.event.kind, WireEventKind::SkillStatus(_))),
        "高风险静默时不应向 TUI 回显技能状态"
    );
    assert!(captured_labels_contain(&captured, &["memory_update"]));
    assert_no_running_subagent_sessions(&request);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn durable_governance_requeues_failed_subagent_execution() {
    let root = test_root("failed-subagent-retry");
    let config_path = root.join("runtime").join("config.toml");
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    let request = request_fixture_with_executor(
        root.clone(),
        config_path.clone(),
        Arc::new(AssessorOnlyExecutor),
    );
    let mut response = response_with_receipts(&[
        receipt("call-1", "builtin.edit", "completed", "patched files", true),
        receipt(
            "call-2",
            "builtin.shell_command",
            "completed",
            "cargo check passed",
            false,
        ),
        receipt(
            "call-3",
            "builtin.shell_command",
            "completed",
            "cargo test passed",
            false,
        ),
    ]);
    response.turn_events.extend(provider_turn_events(7));

    let completed = complete_turn(&TurnSpec::primary(), &request, response);
    let receipt = completed
        .post_run_governance
        .as_ref()
        .expect("完成后必须启动 post-run governance");
    let status_path = receipt.status_path.as_ref().expect("必须返回 statusPath");

    assert_eq!(
        run_governance_worker_with_request(&config_path, &AxonConfig::default(), &request),
        Ok(0)
    );
    assert_eq!(
        governance_job_status(std::path::Path::new(status_path), &receipt.job_id),
        "queued"
    );
    assert!(
        governance_job_error(std::path::Path::new(status_path), &receipt.job_id)
            .contains("1 个子任务未完成")
    );
    let _ = std::fs::remove_dir_all(root);
}

/// 构造一条形态真实的一轮响应：每个回执都有自己的调用、生命周期事件与唯一 part。
///
/// thread 事件流是轮次事实的唯一来源，`mirror_session_to_thread` 只投影「有调用、
/// 有 started 事件、part 互不相同」的回执；夹具必须跟生产形态一致，否则治理读到
/// 的轮次就不是这一轮。
fn response_with_receipts(receipts: &[RunToolReceipt]) -> RunResponse {
    let started = receipts
        .iter()
        .enumerate()
        .map(|(index, _)| {
            AgentTurnEvent::new(
                TurnId(u64::try_from(index).unwrap_or(u64::MAX).saturating_add(1)),
                AgentTurnEventKind::ToolExecutionStarted,
                "tool execution started",
            )
        })
        .collect::<Vec<_>>();
    let tool_receipts = receipts
        .iter()
        .enumerate()
        .map(|(index, receipt)| RunToolReceipt {
            tool_call_id: u64::try_from(index).unwrap_or(u64::MAX).saturating_add(1),
            part_id: started[index].part_id.clone(),
            ..receipt.clone()
        })
        .collect::<Vec<_>>();
    let tool_calls = tool_receipts
        .iter()
        .map(|receipt| {
            ProviderToolCall::new(
                receipt.provider_call_id.clone(),
                receipt.tool_name.clone(),
                tool_call_input(&receipt.tool_name),
            )
        })
        .collect();
    RunResponse {
        content: "done".to_owned(),
        assistant_part_id: "assistant-part:post-run-reflection".to_owned(),
        provider: "fixture".to_owned(),
        model: "fixture-chat".to_owned(),
        token_usage: None,
        http_status: 200,
        channel_attempts: Vec::new(),
        reasoning_messages: Vec::new(),
        tool_receipts,
        tool_calls,
        turn_events: started,
        max_tokens_truncated: false,
    }
}

/// 与工具名一致的调用输入：参数指纹与证据归因都按真实调用形态取值。
fn tool_call_input(tool_name: &str) -> String {
    match tool_name {
        "builtin.invoke_skill" => r#"{"skill_name":"release"}"#.to_owned(),
        "builtin.edit" => r#"{"path":"src/lib.rs"}"#.to_owned(),
        _ => r#"{"command":"cargo test"}"#.to_owned(),
    }
}

fn receipt(
    provider_call_id: &str,
    tool_name: &str,
    status: &str,
    summary: &str,
    wrote: bool,
) -> RunToolReceipt {
    RunToolReceipt {
        tool_call_id: 0,
        part_id: "tool-part:0".to_owned(),
        provider_call_id: provider_call_id.to_owned(),
        tool_name: tool_name.to_owned(),
        status: status.to_owned(),
        summary: summary.to_owned(),
        metadata: None,
        error_type: None,
        remote_trigger_allowed: false,
        filesystem_write_performed: wrote,
        output_ref: None,
        duration_ms: None,
    }
}

#[path = "run_post_run_reflection_fixtures.rs"]
mod fixtures;
use fixtures::*;

// ── 反思准入与轨迹寻址 ────────────────────────────────────────────────────────

/// 拿不到轮序时轨迹必须不可寻址：`trace:<session>:0` 会让同一会话所有轮撞号。
#[test]
fn missing_turn_sequence_leaves_the_trace_unaddressable() {
    let root = test_root("trace-addressable");
    let config_path = root.join("config.toml");
    let request = request_fixture(root.clone(), config_path);
    let record = crate::run_commands::run_session_store::RunSessionRecord::from_partial(
        &request,
        &crate::run_commands::run_session_store::RunSessionPartialInput {
            provider: "fixture".to_owned(),
            model: "fixture-chat".to_owned(),
            tool_calls: Vec::new(),
            tool_receipts: Vec::new(),
            reasoning_messages: Vec::new(),
            turn_events: Vec::new(),
        },
    );

    assert_eq!(
        super::run_post_run_artifacts::trace_id_for(&record, None),
        "",
        "轮序缺失必须留空串，不能用 :0 兜底"
    );
    assert_eq!(
        super::run_post_run_artifacts::trace_id_for(&record, Some(3)),
        format!("trace:{}:3", record.session_id)
    );
    let _ = std::fs::remove_dir_all(root);
}

/// 不可寻址的轮次不入池，但候选数要进入可见的 skipped 计数。
#[test]
fn unaddressable_trace_skips_the_pool_and_counts_candidates() {
    let root = test_root("unaddressable-trace");
    let config_path = root.join("config.toml");
    let request = request_fixture(root.clone(), config_path);
    let response = response_with_receipts(&[receipt(
        "call-edit",
        "builtin.edit",
        "completed",
        "patched files",
        true,
    )]);
    let envelope = build_turn_outcome(&request, &response);
    let assessment = assessment_for_outcome(&envelope);
    let trace = TurnTrace::from_outcome(&envelope, "", 1_000, ExplicitSignals::default());
    let cards = reflection_cards(&trace, Some(&assessment), &request.cwd);
    assert!(!cards.is_empty(), "夹具必须产出至少一张卡");

    let outcome = admit_reflection_batch(
        &request,
        &trace,
        cards.clone(),
        Some(&assessment),
        &envelope,
    );

    assert!(matches!(outcome.admission, BatchAdmission::NotApplicable));
    assert_eq!(outcome.skipped, u64::try_from(cards.len()).unwrap());
    let pool_path = axon_agent_runtime::ReflectionPoolStoreRuntime::resolve_store_path(
        super::run_session_store::resolved_axon_root(&request.config_path).unwrap(),
    );
    assert!(
        axon_agent_runtime::ReflectionPoolStoreRuntime::load_entries(pool_path).is_empty(),
        "不可寻址的轮次不得把卡片放进池里"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// Skip 档里满足执行类三维底线的强卡仍要接纳：组织结论当场就该记住。
#[test]
fn skip_tier_strong_card_is_still_admitted() {
    let root = test_root("skip-tier-strong");
    let config_path = root.join("config.toml");
    let request = request_fixture(root.clone(), config_path);
    let response = response_with_receipts(&[
        receipt(
            "call-edit",
            "builtin.edit",
            "completed",
            "patched files",
            true,
        ),
        receipt(
            "call-test",
            "builtin.shell_command",
            "completed",
            "验证通过",
            false,
        ),
    ]);
    let envelope = build_turn_outcome(&request, &response);
    let mut assessment = assessment_for_outcome(&envelope);
    // 档位判 Skip，但三维仍在线：这正是「Skip 档强卡」的形态。
    assessment.should_reflect = false;
    assessment.candidate_types.clear();
    assessment.evidence_refs.clear();
    assert_eq!(assessment.reflection_tier(&envelope), ReflectionTier::Skip);

    let trace = TurnTrace::from_outcome(
        &envelope,
        "trace:skip-strong:7",
        1_000,
        ExplicitSignals::default(),
    );
    let cards = reflection_cards(&trace, Some(&assessment), &request.cwd);
    assert_eq!(cards.len(), 1);
    let card_id = cards[0].card_id.clone();

    let outcome = admit_reflection_batch(&request, &trace, cards, Some(&assessment), &envelope);

    assert_eq!(outcome.skipped, 0, "强卡不得计入 skipped");
    let pool_path = axon_agent_runtime::ReflectionPoolStoreRuntime::resolve_store_path(
        super::run_session_store::resolved_axon_root(&request.config_path).unwrap(),
    );
    let admitted = match &outcome.admission {
        BatchAdmission::Run(batch) => batch.iter().any(|card| card.card_id == card_id),
        BatchAdmission::Deferred => {
            axon_agent_runtime::ReflectionPoolStoreRuntime::load_entries(&pool_path)
                .iter()
                .any(|entry| entry.card.card_id == card_id)
        }
        BatchAdmission::NotApplicable => false,
    };
    assert!(
        admitted,
        "Skip 档的强卡必须被接纳（skipped={}）",
        outcome.skipped
    );
    let _ = std::fs::remove_dir_all(root);
}

/// 达不到执行类底线的弱卡在 Skip 档里不入池，但要计入可见 skipped。
#[test]
fn skip_tier_weak_card_counts_as_skipped() {
    let root = test_root("skip-tier-weak");
    let config_path = root.join("config.toml");
    let request = request_fixture(root.clone(), config_path);
    let response = response_with_receipts(&[receipt(
        "call-edit",
        "builtin.edit",
        "completed",
        "patched files",
        true,
    )]);
    let envelope = build_turn_outcome(&request, &response);
    let mut assessment = assessment_for_outcome(&envelope);
    assessment.should_reflect = false;
    assessment.candidate_types.clear();
    assessment.evidence_refs.clear();
    // 三维全部为 0：既判 Skip，也不满足执行类底线。
    assessment.signals = SemanticSalienceSignals::default();
    assert_eq!(assessment.reflection_tier(&envelope), ReflectionTier::Skip);

    let trace = TurnTrace::from_outcome(
        &envelope,
        "trace:skip-weak:8",
        1_000,
        ExplicitSignals::default(),
    );
    let cards = reflection_cards(&trace, Some(&assessment), &request.cwd);
    assert_eq!(cards.len(), 1);
    let card_id = cards[0].card_id.clone();

    let outcome = admit_reflection_batch(&request, &trace, cards, Some(&assessment), &envelope);

    assert_eq!(outcome.skipped, 1, "弱卡必须计入 skipped");
    let pool_path = axon_agent_runtime::ReflectionPoolStoreRuntime::resolve_store_path(
        super::run_session_store::resolved_axon_root(&request.config_path).unwrap(),
    );
    assert!(
        !axon_agent_runtime::ReflectionPoolStoreRuntime::load_entries(&pool_path)
            .iter()
            .any(|entry| entry.card.card_id == card_id),
        "弱卡不得入池"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// 同一 card_id 的跨轮卡片入池前必须被拒收，不能覆盖或重复排队。
#[test]
fn duplicate_card_id_is_rejected_before_pool_append() {
    let root = test_root("duplicate-card-id");
    let config_path = root.join("config.toml");
    let request = request_fixture(root.clone(), config_path);
    let response = response_with_receipts(&[receipt(
        "call-edit",
        "builtin.edit",
        "completed",
        "patched files",
        true,
    )]);
    let envelope = build_turn_outcome(&request, &response);
    let assessment = assessment_for_outcome(&envelope);
    let trace =
        TurnTrace::from_outcome(&envelope, "trace:dup:9", 1_000, ExplicitSignals::default());
    let cards = reflection_cards(&trace, Some(&assessment), &request.cwd);
    let card = cards[0].clone();

    // 池里先放一条同 card_id、不同轮次的条目。
    let pool_path = axon_agent_runtime::ReflectionPoolStoreRuntime::resolve_store_path(
        super::run_session_store::resolved_axon_root(&request.config_path).unwrap(),
    );
    let seeded = axon_agent_runtime::ReflectionPoolEntry {
        correction_bp: assessment.signals.correction_strength,
        turn_id: "turn:someone-else".to_owned(),
        trace_id: card.trace_id.clone(),
        enqueued_at_ms: 1,
        card: card.clone(),
        bucket: axon_agent_runtime::pool_bucket_key(&request.cwd.to_string_lossy(), None),
    };
    axon_agent_runtime::ReflectionPoolStoreRuntime::append_entry(&pool_path, &seeded).unwrap();

    let outcome = admit_reflection_batch(&request, &trace, cards, Some(&assessment), &envelope);

    assert_eq!(outcome.skipped, 1, "重复 card_id 必须计入 skipped");
    // 同 card_id 只能出现一次：要么留在池里，要么被这一批取走去执行，不能两处都有。
    let in_pool = axon_agent_runtime::ReflectionPoolStoreRuntime::load_entries(&pool_path)
        .into_iter()
        .filter(|entry| entry.card.card_id == card.card_id)
        .count();
    let in_batch = match &outcome.admission {
        BatchAdmission::Run(batch) => batch
            .iter()
            .filter(|candidate| candidate.card_id == card.card_id)
            .count(),
        BatchAdmission::Deferred | BatchAdmission::NotApplicable => 0,
    };
    assert_eq!(
        in_pool + in_batch,
        1,
        "同 card_id 只能占一个位置（pool={in_pool} batch={in_batch}）"
    );
    let seeded_entry = axon_agent_runtime::ReflectionPoolStoreRuntime::load_entries(&pool_path)
        .into_iter()
        .find(|entry| entry.turn_id == "turn:someone-else");
    if let Some(entry) = seeded_entry {
        assert_eq!(entry.card.card_id, card.card_id, "原条目不得被覆盖");
    }
    let _ = std::fs::remove_dir_all(root);
}

#[cfg(test)]
#[path = "run_post_run_reflection_report_tests.rs"]
mod report_tests;
