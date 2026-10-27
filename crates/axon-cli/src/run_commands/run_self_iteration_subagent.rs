use axon_agent_runtime::{
    AgentExecutionLease, AgentPath, AgentSpawnRequest, ProviderStructuredOutputContract,
    ProviderStructuredOutputMode, SEMANTIC_SALIENCE_SCHEMA_VERSION, SelfIterationHookKind,
    SelfIterationInput, SelfIterationRuntime, SelfIterationSkillExecution,
    SelfIterationSkillSource, SelfIterationSubagentResult, SelfIterationSubagentStatus,
    SemanticMemoryCandidateType, SemanticSalienceAssessment, SubagentDelegationEntry,
    SubagentForkContextInput, SubagentForkContextRuntime, SubagentForkTurns, SubagentInheritance,
    SubagentSessionStartInput, TurnOutcomeEnvelope,
};
use axon_domain::{AxonResult, PaneId, SessionId};

use crate::stable_ids::stable_numeric_id;

use super::{
    RunRequest,
    run_execution::{RunResponse, now_ms},
    run_skill_evolution::{SkillEvolutionEvent, draft_skill_for_run, skill_evolution_for_run},
    run_subagent_completion::log_completion_failure,
    run_subagent_executor::{
        RunSubagentCompletionSink, RunSubagentExecutionInput, RunSubagentExecutionOutput,
        RunSubagentExecutionStatus, RunSubagentModelIntent,
    },
    run_turn_outcome::build_turn_outcome,
};

const SELF_ITERATION_AGENT_PREFIX: &str = "self_iteration";
const SELF_ITERATION_AGENT_ROLE: &str = "self_iteration";
const SELF_ITERATION_MEMORY_SUMMARY: &str = "自闭环记忆更新";
const SELF_ITERATION_SKILL_SUMMARY: &str = "自闭环 Skill 反思";
const SELF_ITERATION_EXECUTOR_UNAVAILABLE: &str = "自我迭代子代理执行器不可用";
const SELF_ITERATION_SESSION_SETUP_FAILED: &str = "自我迭代子代理会话初始化失败";
const SEMANTIC_SALIENCE_LABEL: &str = "semantic_salience";
const SEMANTIC_SALIENCE_SUMMARY: &str = "轻量语义显著性评估";
const SEMANTIC_SALIENCE_MAX_RUNTIME_MS: u64 = 15_000;
const SEMANTIC_SALIENCE_SYSTEM_PROMPT: &str = "你是 AXON 的轻量语义显著性评估器。只判断随附 turn outcome 是否包含值得跨会话复用的长期信息，不执行工具、不延续任务、不依据关键词或正则做判断。执行事实只能来自 runtimeVerified=true 的 evidence；assistantOutcomeSummary 不能单独证明执行。用户明确、稳定、可复用的画像或偏好可引用 userMessageRef；experience、knowledge、procedure 必须同时有运行时执行证据。普通任务、临时状态和一次性事实必须 shouldReflect=false，此时 candidateTypes 与 evidenceRefs 必须为空。userAffect 判断用户在本轮表达出的状态，证据不足写 unknown。personaDrift 判断助手回复偏离人格底色的程度（0 至 10000 基点），正常遵循基线时写 0。用户疲劳度写进 userFatigue（low / normal / high）：从措辞、时段与连轴转迹象判断，拿不准填 normal。用户明确表达对说话方式、措辞、语气或详略的偏好时，candidateTypes 使用 style，并把这条偏好本身写进 styleCandidate（不超过 120 字），不要把判断理由写进去。personaDrift 达到 7000 时必须同时给出 driftReason（不超过 80 字），说明偏在哪。只输出符合 schema 的 JSON。";

#[derive(Debug, Default)]
pub(super) struct PostRunSelfIterationReport {
    pub(super) skill_evolution: Option<SkillEvolutionEvent>,
    /// 本轮语义评估结果；缺失表示评估未完成或不可信。
    pub(super) semantic_assessment: Option<SemanticSalienceAssessment>,
    /// 本轮用户消息的证据引用，用于校验风格候选的来源。
    pub(super) user_message_ref: Option<String>,
    pub(super) failed_entry_count: usize,
    pub(super) quality: PostRunSelfIterationQuality,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct PostRunSelfIterationQuality {
    pub(super) reflection_attempts: u64,
    pub(super) reflection_yields: u64,
    pub(super) semantic_assessments: u64,
    pub(super) false_memory_rejections: u64,
}

#[derive(Debug, Default)]
struct SemanticAssessmentOutcome {
    assessment: Option<SemanticSalienceAssessment>,
    completed: bool,
    rejected: bool,
}

pub(super) fn run_post_run_self_iteration_subagents(
    request: &RunRequest,
    response: &RunResponse,
) -> PostRunSelfIterationReport {
    let turn_outcome = build_turn_outcome(request, response);
    let semantic_outcome = assess_semantic_salience(request, &turn_outcome);
    let mut report = PostRunSelfIterationReport {
        quality: PostRunSelfIterationQuality {
            semantic_assessments: u64::from(semantic_outcome.completed),
            false_memory_rejections: u64::from(semantic_outcome.rejected),
            ..PostRunSelfIterationQuality::default()
        },
        // 评估结果先于钩子规划落到报告上：没有钩子的普通回合同样要把它交给人格结算。
        semantic_assessment: semantic_outcome.assessment.clone(),
        user_message_ref: Some(turn_outcome.user_message_ref.clone()),
        ..PostRunSelfIterationReport::default()
    };
    let input = self_iteration_input(request, turn_outcome, semantic_outcome.assessment);
    let Ok(plan) = SelfIterationRuntime.plan_with_delegation(session_id(request), false, &input)
    else {
        return report;
    };
    let Some(delegation_plan) = plan.delegation_plan.as_ref() else {
        return report;
    };
    report.quality.reflection_attempts =
        u64::try_from(delegation_plan.entries.len()).unwrap_or(u64::MAX);

    let mut results = Vec::with_capacity(delegation_plan.entries.len());
    let mut skill_evolution = None;
    let mut failed_entry_count = 0;
    for entry in &delegation_plan.entries {
        let output = execute_self_iteration_subagent(
            request,
            entry,
            SelfIterationExecutionOptions::reflection(),
        );
        let status = subagent_result_status(output.as_ref());
        if !subagent_completed(status) {
            failed_entry_count += 1;
        }
        let hook = match entry.label.as_str() {
            "memory_update" => SelfIterationHookKind::MemoryUpdate,
            "skill_reflection" => SelfIterationHookKind::SkillReflection,
            _ => continue,
        };
        let should_evolve_skill =
            hook == SelfIterationHookKind::SkillReflection && request.selected_skill_name.is_some();
        let mut entry_yielded = output
            .as_ref()
            .is_some_and(|result| result.memory_write_count > 0);
        if subagent_completed(status) && should_evolve_skill {
            let reflection = output
                .as_ref()
                .map(|result| result.summary.as_str())
                .unwrap_or_default();
            let event = skill_evolution_for_run(request, response, reflection);
            entry_yielded |= event.is_some();
            skill_evolution = skill_evolution.or(event);
        }
        let should_draft_skill = hook == SelfIterationHookKind::MemoryUpdate
            && input
                .semantic_assessment
                .as_ref()
                .is_some_and(|assessment| {
                    assessment
                        .candidate_types
                        .contains(&SemanticMemoryCandidateType::Procedure)
                });
        if subagent_completed(status) && should_draft_skill {
            let reflection = output
                .as_ref()
                .map(|result| result.summary.as_str())
                .unwrap_or_default();
            let evidence_refs = input
                .semantic_assessment
                .as_ref()
                .map(|assessment| assessment.evidence_refs.as_slice())
                .unwrap_or_default();
            let event = draft_skill_for_run(request, response, reflection, evidence_refs);
            entry_yielded |= event.is_some();
            skill_evolution = skill_evolution.or(event);
        }
        if entry_yielded {
            report.quality.reflection_yields = report.quality.reflection_yields.saturating_add(1);
        }
        results.push(subagent_result(entry, hook, status, output.as_ref()));
    }
    let _ = SelfIterationRuntime.merge_delegation_results(&plan, 0, results);
    report.semantic_assessment = input.semantic_assessment.clone();
    report.skill_evolution = skill_evolution;
    report.failed_entry_count = failed_entry_count;
    report
}

#[derive(Debug, Clone)]
struct SelfIterationExecutionOptions {
    structured_output: Option<ProviderStructuredOutputContract>,
    system_prompt_override: Option<String>,
    tools_enabled: bool,
    model_intent: RunSubagentModelIntent,
}

impl SelfIterationExecutionOptions {
    fn reflection() -> Self {
        Self {
            structured_output: None,
            system_prompt_override: None,
            tools_enabled: true,
            model_intent: RunSubagentModelIntent::Inherit,
        }
    }

    fn semantic_salience() -> Self {
        // 快速模型通道可能不支持 `json_schema` 严格模式；transport 会按需降级为
        // prompt-only 契约，输出仍由 `parse_semantic_assessment` 独立校验。
        let structured_output = ProviderStructuredOutputContract {
            schema: semantic_salience_output_schema(),
            mode: ProviderStructuredOutputMode::JsonSchema,
        };
        Self {
            structured_output: Some(structured_output),
            system_prompt_override: Some(SEMANTIC_SALIENCE_SYSTEM_PROMPT.to_owned()),
            tools_enabled: false,
            model_intent: RunSubagentModelIntent::FastDelegate,
        }
    }
}

fn assess_semantic_salience(
    request: &RunRequest,
    envelope: &TurnOutcomeEnvelope,
) -> SemanticAssessmentOutcome {
    if !request.current_agent_path.is_root() {
        return SemanticAssessmentOutcome::default();
    }
    let Ok(envelope_json) = serde_json::to_string(envelope) else {
        return SemanticAssessmentOutcome::default();
    };
    let message = format!("评估以下有界事实包：\n{envelope_json}");
    let entry = SubagentDelegationEntry {
        dispatch_order: 0,
        label: SEMANTIC_SALIENCE_LABEL.to_owned(),
        task_message: message,
        inheritance: SubagentInheritance::ModelToolsOnly,
        max_runtime_ms: Some(SEMANTIC_SALIENCE_MAX_RUNTIME_MS),
    };
    let Some(output) = execute_self_iteration_subagent(
        request,
        &entry,
        SelfIterationExecutionOptions::semantic_salience(),
    ) else {
        return SemanticAssessmentOutcome::default();
    };
    if output.status != RunSubagentExecutionStatus::Completed {
        return SemanticAssessmentOutcome::default();
    }
    completed_semantic_assessment(output.summary.trim(), envelope)
}

fn completed_semantic_assessment(
    raw: &str,
    envelope: &TurnOutcomeEnvelope,
) -> SemanticAssessmentOutcome {
    let assessment = parse_semantic_assessment(raw, envelope);
    SemanticAssessmentOutcome {
        rejected: assessment.is_none(),
        assessment,
        completed: true,
    }
}

fn parse_semantic_assessment(
    raw: &str,
    envelope: &TurnOutcomeEnvelope,
) -> Option<SemanticSalienceAssessment> {
    let assessment: SemanticSalienceAssessment = serde_json::from_str(raw).ok()?;
    assessment.is_valid_for(envelope).then_some(assessment)
}

fn semantic_salience_output_schema() -> serde_json::Value {
    let signal = serde_json::json!({ "type": "integer", "minimum": 0, "maximum": 10_000 });
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "schemaVersion": { "type": "integer", "const": SEMANTIC_SALIENCE_SCHEMA_VERSION },
            "shouldReflect": { "type": "boolean" },
            "candidateTypes": {
                "type": "array",
                "items": { "type": "string", "enum": ["profile", "preference", "experience", "knowledge", "procedure", "style"] }
            },
            "signals": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "identityRelevance": signal.clone(),
                    "stability": signal.clone(),
                    "futureUtility": signal.clone(),
                    "novelty": signal.clone(),
                    "correctionStrength": signal.clone(),
                    "verificationStrength": signal.clone(),
                    "scopeConfidence": signal
                },
                "required": ["identityRelevance", "stability", "futureUtility", "novelty", "correctionStrength", "verificationStrength", "scopeConfidence"]
            },
            "evidenceRefs": { "type": "array", "items": { "type": "string" } },
            "reason": { "type": "string", "minLength": 1, "maxLength": 500 },
            "userAffect": { "type": "string", "enum": ["neutral", "positive", "frustrated", "urgent", "unknown"] },
            "userFatigue": { "type": "string", "enum": ["low", "normal", "high"] },
            "personaDrift": { "type": "integer", "minimum": 0, "maximum": 10_000 },
            "driftReason": { "type": "string", "minLength": 1, "maxLength": 80 },
            "styleCandidate": { "type": "string", "minLength": 1, "maxLength": 120 }
        },
        "required": ["schemaVersion", "shouldReflect", "candidateTypes", "signals", "evidenceRefs", "reason", "userAffect", "personaDrift"]
    })
}

fn subagent_result(
    entry: &axon_agent_runtime::SubagentDelegationEntry,
    hook: SelfIterationHookKind,
    status: SelfIterationSubagentStatus,
    output: Option<&RunSubagentExecutionOutput>,
) -> SelfIterationSubagentResult {
    SelfIterationSubagentResult {
        hook,
        label: entry.label.clone(),
        cost_micros: 0,
        status,
        bounded_summary: output_summary(output, hook_summary(hook)),
    }
}

fn execute_self_iteration_subagent(
    request: &RunRequest,
    entry: &axon_agent_runtime::SubagentDelegationEntry,
    options: SelfIterationExecutionOptions,
) -> Option<RunSubagentExecutionOutput> {
    let spawn_request = self_iteration_spawn_request(request, entry);
    let planned_task_path = spawn_request
        .parent_path
        .join(&spawn_request.task_name)
        .ok()?;
    let fork_context = SubagentForkContextRuntime
        .plan(SubagentForkContextInput {
            parent_messages: request.model_payload.messages.clone(),
            fork_turns: fork_turns(entry.inheritance),
            parent_usage_hint_texts: Vec::new(),
            child_usage_hint_text: Some(child_usage_hint(
                &request.current_agent_path,
                &planned_task_path,
            )),
            usage_hint_enabled: true,
        })
        .ok()?;
    let (task_path, execution_lease) = spawn_self_iteration_agent(request, spawn_request).ok()?;
    let started_at_ms = now_ms();
    let input = RunSubagentExecutionInput {
        task_path,
        message: entry.task_message.clone(),
        structured_output: options.structured_output,
        system_prompt_override: options.system_prompt_override,
        tools_enabled: options.tools_enabled,
        fork_context,
        started_at_ms,
        max_runtime_ms: entry.max_runtime_ms,
        model_intent: options.model_intent,
    };
    let completion = RunSubagentCompletionSink::new(
        request.agent_control.clone(),
        request.agent_graph_store.clone(),
        request.subagent_sessions.clone(),
        execution_lease.clone(),
    );
    if let Err(error) =
        start_self_iteration_session(request, entry, &input, execution_lease.execution_id())
    {
        complete_self_iteration_setup_failure(&completion, &input, &error);
        return None;
    }

    let output = request.subagent_executor.execute(input.clone());
    let completion_output = output
        .clone()
        .unwrap_or_else(|| RunSubagentExecutionOutput {
            status: RunSubagentExecutionStatus::Errored,
            summary: SELF_ITERATION_EXECUTOR_UNAVAILABLE.to_owned(),
            memory_write_count: 0,
            token_usage: None,
            model: None,
            budget_exhausted: false,
        });
    if let Err(error) = completion.complete(&input, &completion_output) {
        log_completion_failure(&input.task_path, &error);
    }
    output
}

fn start_self_iteration_session(
    request: &RunRequest,
    entry: &axon_agent_runtime::SubagentDelegationEntry,
    input: &RunSubagentExecutionInput,
    execution_id: axon_agent_runtime::AgentExecutionId,
) -> AxonResult<()> {
    let mut sessions = request
        .subagent_sessions
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let handle = sessions.start(SubagentSessionStartInput {
        parent_session_id: session_id(request),
        task_path: input.task_path.to_string(),
        forked_from_session_id: Some(session_id(request)),
        pane_id: PaneId(0),
        cwd: request.cwd.to_string_lossy().to_string(),
        model_profile_id: request.model_profile_id,
        permission_snapshot_id: request.permission_snapshot_id,
        created_at_ms: input.started_at_ms,
        summary: session_summary(&entry.label).to_owned(),
    })?;
    if let Err(error) = sessions.begin_execution_by_task_path(
        &handle.task_path,
        execution_id,
        entry.task_message.clone(),
    ) {
        let _ = sessions.cancel(handle.session_id, format!("{error:?}"));
        return Err(error);
    }
    Ok(())
}

fn complete_self_iteration_setup_failure(
    completion: &RunSubagentCompletionSink,
    input: &RunSubagentExecutionInput,
    error: &impl std::fmt::Debug,
) {
    let output = RunSubagentExecutionOutput {
        status: RunSubagentExecutionStatus::Errored,
        summary: format!("{SELF_ITERATION_SESSION_SETUP_FAILED}: {error:?}"),
        memory_write_count: 0,
        token_usage: None,
        model: None,
        budget_exhausted: false,
    };
    if let Err(completion_error) = completion.complete(input, &output) {
        log_completion_failure(&input.task_path, &completion_error);
    }
}

fn self_iteration_spawn_request(
    request: &RunRequest,
    entry: &axon_agent_runtime::SubagentDelegationEntry,
) -> AgentSpawnRequest {
    AgentSpawnRequest {
        parent_path: request.current_agent_path.clone(),
        task_name: format!("{SELF_ITERATION_AGENT_PREFIX}_{}", entry.label),
        message: entry.task_message.clone(),
        nickname: None,
        role: Some(SELF_ITERATION_AGENT_ROLE.to_owned()),
    }
}

fn spawn_self_iteration_agent(
    request: &RunRequest,
    spawn_request: AgentSpawnRequest,
) -> AxonResult<(AgentPath, AgentExecutionLease)> {
    let mut control = request
        .agent_control
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let report = control.spawn(spawn_request)?;
    let execution_lease = control.lease_execution(&report.path)?;
    Ok((report.path, execution_lease))
}

fn subagent_result_status(
    output: Option<&RunSubagentExecutionOutput>,
) -> SelfIterationSubagentStatus {
    match output {
        Some(output) if output.status == RunSubagentExecutionStatus::Completed => {
            persisted_status(output.memory_write_count > 0)
        }
        Some(output) if output.status == RunSubagentExecutionStatus::Interrupted => {
            SelfIterationSubagentStatus::Interrupted
        }
        Some(_) | None => SelfIterationSubagentStatus::FailedNonFatal,
    }
}

const fn subagent_completed(status: SelfIterationSubagentStatus) -> bool {
    matches!(
        status,
        SelfIterationSubagentStatus::CompletedNoWrite
            | SelfIterationSubagentStatus::CompletedWithWrite
    )
}

const fn persisted_status(persisted: bool) -> SelfIterationSubagentStatus {
    if persisted {
        SelfIterationSubagentStatus::CompletedWithWrite
    } else {
        SelfIterationSubagentStatus::CompletedNoWrite
    }
}

const fn hook_summary(hook: SelfIterationHookKind) -> &'static str {
    match hook {
        SelfIterationHookKind::MemoryUpdate => SELF_ITERATION_MEMORY_SUMMARY,
        SelfIterationHookKind::SkillReflection => SELF_ITERATION_SKILL_SUMMARY,
    }
}

fn output_summary(output: Option<&RunSubagentExecutionOutput>, fallback: &str) -> String {
    output
        .map(|output| output.summary.trim())
        .filter(|summary| !summary.is_empty())
        .map_or_else(|| fallback.to_owned(), str::to_owned)
}

const fn fork_turns(inheritance: SubagentInheritance) -> SubagentForkTurns {
    match inheritance {
        SubagentInheritance::ModelToolsHistory => SubagentForkTurns::All,
        SubagentInheritance::ModelToolsOnly => SubagentForkTurns::None,
    }
}

fn child_usage_hint(parent_path: &AgentPath, task_path: &AgentPath) -> String {
    format!(
        "你正在作为 AXON 自我迭代子代理运行。\n\
         - 当前 canonical task path: {task_path}\n\
         - 直接父代理路径: {parent_path}\n\
         - 只处理本轮 post-run 反思/记忆闭环，不向用户发起审阅或确认。"
    )
}

fn session_summary(label: &str) -> &'static str {
    match label {
        "memory_update" => SELF_ITERATION_MEMORY_SUMMARY,
        "skill_reflection" => SELF_ITERATION_SKILL_SUMMARY,
        SEMANTIC_SALIENCE_LABEL => SEMANTIC_SALIENCE_SUMMARY,
        _ => SELF_ITERATION_MEMORY_SUMMARY,
    }
}

fn session_id(request: &RunRequest) -> SessionId {
    SessionId(stable_numeric_id(&request.session_id).max(1))
}

pub(super) fn self_iteration_input(
    request: &RunRequest,
    turn_outcome: TurnOutcomeEnvelope,
    semantic_assessment: Option<SemanticSalienceAssessment>,
) -> SelfIterationInput {
    let skill_execution = request.selected_skill_name.as_ref().map(|skill_name| {
        let selected = turn_outcome
            .selected_skill
            .as_ref()
            .filter(|skill| skill.skill_name == *skill_name);
        SelfIterationSkillExecution {
            skill_name: skill_name.clone(),
            source: skill_source(request.selected_skill_source_ref.as_deref()),
            execution_verified: selected.is_some_and(|skill| skill.execution_verified),
            evidence_refs: selected
                .map(|skill| skill.evidence_refs.clone())
                .unwrap_or_default(),
        }
    });
    SelfIterationInput {
        is_subagent: !request.current_agent_path.is_root(),
        turn_outcome,
        semantic_assessment,
        skill_execution,
        parent_total_cost_micros: 0,
        subagent_results: Vec::new(),
    }
}

fn skill_source(source_ref: Option<&str>) -> SelfIterationSkillSource {
    match source_ref.unwrap_or_default() {
        value if value.starts_with("skill://axon-project-skills/") => {
            SelfIterationSkillSource::Project
        }
        value if value.starts_with("skill://axon-system/") => SelfIterationSkillSource::Default,
        value if value.starts_with("skill://axon-brand/") => SelfIterationSkillSource::Brand,
        _ => SelfIterationSkillSource::User,
    }
}

#[cfg(test)]
#[path = "run_semantic_salience_tests.rs"]
mod semantic_salience_tests;
