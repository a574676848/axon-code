//! 反思链路测试夹具：运行态隔离目录与后台会话校验。

use super::*;

pub(super) fn assert_no_running_subagent_sessions(request: &RunRequest) {
    let sessions = request
        .subagent_sessions
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert!(
        sessions
            .sessions()
            .iter()
            .all(|handle| handle.status != axon_agent_runtime::SubagentSessionStatus::Running),
        "post-run self-iteration 子代理必须完成会话闭环"
    );
}

pub(super) fn test_root(label: &str) -> std::path::PathBuf {
    static NEXT_TEST_ROOT_ID: AtomicU64 = AtomicU64::new(1);

    let root = axon_test_support::test_temp_root().join(format!(
        "axon-post-run-reflection-{label}-{}-{}",
        std::process::id(),
        NEXT_TEST_ROOT_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

pub(super) fn request_fixture(
    root: std::path::PathBuf,
    config_path: std::path::PathBuf,
) -> RunRequest {
    request_fixture_with_executor(
        root,
        config_path,
        Arc::new(crate::run_commands::run_subagent_executor::NoopRunSubagentExecutor),
    )
}

pub(super) fn request_fixture_with_executor(
    root: std::path::PathBuf,
    config_path: std::path::PathBuf,
    subagent_executor: Arc<dyn RunSubagentExecutor>,
) -> RunRequest {
    let session_id = "session-1".to_owned();
    // 每轮运行都挂在一条已登记的会话上：指标、事件与标题都落在它的运行窗口上。
    use axon_session::RuntimeStore as _;
    let run_window = crate::session_commands::open_control_store(&config_path)
        .expect("运行态库必须可打开")
        .create_session(
            &axon_domain::DaemonSessionId::new(session_id.clone()).expect("合法会话标识"),
            &root.to_string_lossy(),
            axon_time::unix_now_ms(),
        )
        .expect("会话登记必须成功")
        .current_run_id;
    let (startup, execution_tools) =
        crate::run_commands::run_startup_snapshot::test_startup_runtime(
            &config_path,
            &root,
            &session_id,
        );
    RunRequest {
        user_question_port: None,
        client_channel: axon_client_identity::ClientChannel::Cli,
        config_path,
        session_id: session_id.clone(),
        run_session_id: run_window.clone(),
        interaction_id: String::new(),
        invocation_id: format!("test-invocation:{session_id}"),
        resumed_from: None,
        parent_session_id: None,
        parent_thread_id: None,
        cwd: root,
        prompt: "生成可复用 skill".to_owned(),
        quoted_message: None,
        structured_output: None,
        selected_skill_name: None,
        selected_skill_source_ref: None,
        model_payload: AgentModelPayload {
            messages: vec![AgentModelMessage::text("user", "生成可复用 skill")],
            model_tools: Vec::new(),
            context_section_count: 0,
            agent_pane_included: false,
            injected_context_messages: Vec::new(),
        },
        persona_warnings: Vec::new(),
        startup,
        execution_tools,
        provider: ProviderConfig::default(),
        model: ModelConfig::default(),
        reasoning_effort: None,
        channel_chain: Vec::new(),
        routing_key: "fixture".to_owned(),
        balancer: ChannelBalancerConfig::default(),
        timeout: Duration::from_secs(30),
        memory_recall: None,
        goal_disclosure_policy: axon_domain::DisclosurePolicy::PrivateOnly,
        goal_transport_context: None,
        cancel_token: None,
        model_profile_id: axon_domain::ModelProfileId(0),
        permission_snapshot_id: axon_domain::PermissionSnapshotId(0),
        current_agent_path: axon_agent_runtime::AgentPath::root(),
        agent_control: std::sync::Arc::new(std::sync::Mutex::new(
            axon_agent_runtime::AgentControlRuntime::default(),
        )),
        agent_graph_store: std::sync::Arc::new(axon_agent_runtime::LocalAgentGraphStore::default()),
        subagent_sessions: std::sync::Arc::new(std::sync::Mutex::new(
            axon_agent_runtime::SubagentSessionRuntime::default(),
        )),
        subagent_executor,
        subagent_cancel_tokens: std::sync::Arc::new(std::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
        steer_inbox: None,
        workspace_files: std::sync::Arc::new(axon_agent_runtime::WorkspaceFileSession::default()),
        auto_compaction_threshold: 0.8,
        performance: None,
    }
}

#[derive(Debug)]
pub(super) struct CapturingSelfIterationExecutor {
    pub(super) captured: Arc<Mutex<Vec<RunSubagentExecutionInput>>>,
}

impl RunSubagentExecutor for CapturingSelfIterationExecutor {
    fn execute(&self, input: RunSubagentExecutionInput) -> Option<RunSubagentExecutionOutput> {
        let is_assessor = input.task_path.name().ends_with("semantic_salience");
        self.captured
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(input);
        Some(RunSubagentExecutionOutput {
            status: RunSubagentExecutionStatus::Completed,
            summary: if is_assessor {
                semantic_assessment_json()
            } else {
                "子代理自闭环完成".to_owned()
            },
            memory_write_count: 0,
            token_usage: None,
            model: None,
            budget_exhausted: false,
        })
    }
}

#[derive(Debug)]
pub(super) struct AssessorOnlyExecutor;

impl RunSubagentExecutor for AssessorOnlyExecutor {
    fn execute(&self, input: RunSubagentExecutionInput) -> Option<RunSubagentExecutionOutput> {
        input
            .task_path
            .name()
            .ends_with("semantic_salience")
            .then(|| RunSubagentExecutionOutput {
                status: RunSubagentExecutionStatus::Completed,
                summary: semantic_assessment_json(),
                memory_write_count: 0,
                token_usage: None,
                model: None,
                budget_exhausted: false,
            })
    }
}

pub(super) fn semantic_assessment_json() -> String {
    serde_json::json!({
        "schemaVersion": SEMANTIC_SALIENCE_SCHEMA_VERSION,
        "shouldReflect": true,
        "candidateTypes": ["experience"],
        "signals": {
            "identityRelevance": 0,
            "stability": 8_000,
            "futureUtility": 9_000,
            "novelty": 8_000,
            "correctionStrength": 0,
            "verificationStrength": 9_000,
            "scopeConfidence": 9_000,
            // v4 的 schema 要求 specificity。缺了它会被反序列化成 0，执行类建卡
            // 就过不了「三维同时达门」，本轮会走攒批而不是单跑。
            "specificity": 9_000
        },
        "evidenceRefs": ["receipt:3"],
        "reason": "已验证的执行经验可跨会话复用"
    })
    .to_string()
}

pub(super) fn captured_labels_contain(
    captured: &Arc<Mutex<Vec<RunSubagentExecutionInput>>>,
    expected: &[&str],
) -> bool {
    let labels = captured_labels(captured);
    expected
        .iter()
        .all(|label| labels.iter().any(|actual| actual.ends_with(label)))
}

pub(super) fn captured_labels(
    captured: &Arc<Mutex<Vec<RunSubagentExecutionInput>>>,
) -> Vec<String> {
    captured
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .map(|input| input.task_path.name().to_owned())
        .collect()
}

pub(super) fn captured_input_ending_with(
    captured: &Arc<Mutex<Vec<RunSubagentExecutionInput>>>,
    suffix: &str,
) -> Option<RunSubagentExecutionInput> {
    captured
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .find(|input| input.task_path.name().ends_with(suffix))
        .cloned()
}

pub(super) fn self_iteration_labels(request: &RunRequest, response: &RunResponse) -> Vec<String> {
    let turn_outcome = build_turn_outcome(request, response);
    let assessment = assessment_for_outcome(&turn_outcome);
    SelfIterationRuntime
        .plan_with_delegation(
            SessionId(1),
            false,
            &self_iteration_input(request, turn_outcome, Some(assessment), Vec::new()),
        )
        .expect("self-iteration 计划应有效")
        .delegation_plan
        .map_or_else(Vec::new, |plan| {
            plan.entries.into_iter().map(|entry| entry.label).collect()
        })
}

pub(super) fn assessment_for_outcome(outcome: &TurnOutcomeEnvelope) -> SemanticSalienceAssessment {
    let evidence_ref = outcome
        .execution_evidence
        .records
        .iter()
        .rev()
        .find(|record| record.runtime_verified)
        .map(|record| record.evidence_ref.clone());
    let should_reflect = evidence_ref.is_some();
    SemanticSalienceAssessment {
        schema_version: SEMANTIC_SALIENCE_SCHEMA_VERSION,
        should_reflect,
        candidate_types: should_reflect
            .then_some(SemanticMemoryCandidateType::Experience)
            .into_iter()
            .collect(),
        signals: SemanticSalienceSignals {
            future_utility: if should_reflect { 9_000 } else { 0 },
            verification_strength: if should_reflect { 9_000 } else { 0 },
            scope_confidence: if should_reflect { 9_000 } else { 0 },
            // v4 起执行类强卡要求三维同时达门；夹具必须带上 specificity，
            // 否则它只是一张「五维之和看着高、实际不达标」的弱卡，会走攒批而不是单跑。
            specificity: if should_reflect { 9_000 } else { 0 },
            ..SemanticSalienceSignals::default()
        },
        evidence_refs: evidence_ref.into_iter().collect(),
        reason: if should_reflect {
            "已验证的执行经验".to_owned()
        } else {
            "普通任务没有长期价值".to_owned()
        },
        user_affect: None,
        user_fatigue: None,
        persona_drift_bp: None,
        drift_reason: None,
        style_candidate: None,
    }
}

pub(super) fn provider_turn_events(count: u64) -> Vec<AgentTurnEvent> {
    (1..=count)
        .map(|turn_id| {
            AgentTurnEvent::new(
                TurnId(turn_id),
                AgentTurnEventKind::TurnStarted,
                format!("provider turn {turn_id} started"),
            )
        })
        .collect()
}

pub(super) fn governance_job_status(path: &std::path::Path, job_id: &str) -> String {
    rusqlite::Connection::open(path)
        .expect("governance SQLite 应可读取")
        .query_row(
            "SELECT status FROM governance_jobs WHERE job_id = ?1",
            rusqlite::params![job_id],
            |row| row.get(0),
        )
        .expect("governance job 应存在")
}

pub(super) fn governance_job_error(path: &std::path::Path, job_id: &str) -> String {
    rusqlite::Connection::open(path)
        .expect("governance SQLite 应可读取")
        .query_row(
            "SELECT last_error FROM governance_jobs WHERE job_id = ?1",
            rusqlite::params![job_id],
            |row| row.get(0),
        )
        .expect("governance job 应记录失败原因")
}
