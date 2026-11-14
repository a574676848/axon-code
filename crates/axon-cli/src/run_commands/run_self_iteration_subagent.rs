use axon_agent_runtime::{
    AgentExecutionLease, AgentPath, AgentSpawnRequest, ProviderStructuredOutputContract,
    ProviderStructuredOutputMode, ReflectionCard, ReflectionLedgerRuntime, SalienceVector,
    SelfIterationHookKind, SelfIterationInput, SelfIterationRuntime, SelfIterationSubagentResult,
    SelfIterationSubagentStatus, SemanticSalienceAssessment, SubagentForkContextInput,
    SubagentForkContextRuntime, SubagentForkTurns, SubagentInheritance, SubagentSessionStartInput,
    TurnOutcomeEnvelope, TurnTrace,
};
use axon_domain::{AxonResult, PaneId, SessionId};

use crate::stable_ids::stable_numeric_id;

use super::{
    RunRequest,
    run_execution::{RunResponse, now_ms},
    run_reflection_apply::apply_reflection_report,
    run_reflection_batch::{BatchAdmission, admit_reflection_batch, reflection_cards},
    run_semantic_salience::{
        SEMANTIC_SALIENCE_LABEL, assess_semantic_salience, semantic_salience_output_schema,
        semantic_salience_system_prompt,
    },
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
const SELF_ITERATION_EXECUTOR_UNAVAILABLE: &str = "自我迭代子代理执行器不可用";
const SELF_ITERATION_SESSION_SETUP_FAILED: &str = "自我迭代子代理会话初始化失败";
const SEMANTIC_SALIENCE_SUMMARY: &str = "轻量语义显著性评估";

#[derive(Debug, Default)]
pub(super) struct PostRunSelfIterationReport {
    /// 本轮语义评估结果；缺失表示评估未完成或不可信。
    pub(super) semantic_assessment: Option<SemanticSalienceAssessment>,
    /// 本轮用户消息的证据引用，用于校验风格候选的来源。
    pub(super) user_message_ref: Option<String>,
    pub(super) failed_entry_count: usize,
    /// 本轮反思实际写入的记忆条数合计。
    pub(super) memory_write_count: u64,
    /// 本轮反思此前已经跑过：治理队列重试同一条 payload 时置真，调用方据此跳过产物与计数的重复写入。
    pub(super) already_processed: bool,
    pub(super) quality: PostRunSelfIterationQuality,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct PostRunSelfIterationQuality {
    pub(super) reflection_attempts: u64,
    pub(super) reflection_deferred: u64,
    /// 通过准入但未写入、或在准入处被拒的候选条数。
    pub(super) reflection_skipped_candidates: u64,
    pub(super) reflection_yields: u64,
    pub(super) semantic_assessments: u64,
    pub(super) false_memory_rejections: u64,
}

pub(super) fn run_post_run_self_iteration_subagents(
    request: &RunRequest,
    response: &RunResponse,
    trace: &TurnTrace,
) -> PostRunSelfIterationReport {
    // 治理队列对失败 payload 会重试，重试会让评估与反思子代理整条重跑。台账按
    // `session:turn` 记「这一轮跑过了」，重试直接返回，避免同一批候选被写两遍。
    let ledger_root = super::run_session_store::resolved_axon_root(&request.config_path).ok();
    let turn_key = format!("{}:{}", trace.session_id, trace.turn_id);
    let processed_path = ledger_root
        .as_deref()
        .map(ReflectionLedgerRuntime::processed_path);
    if let Some(path) = processed_path.as_deref()
        && ReflectionLedgerRuntime::is_done(path, &turn_key)
    {
        return PostRunSelfIterationReport {
            already_processed: true,
            ..PostRunSelfIterationReport::default()
        };
    }
    // 两态标记：跑前记 `started`，跑完才记 `done`，只有 `done` 会让重试整条跳过。
    // 崩在子代理中途留下的只是 `started`，重试会重跑，但重复写入被 `written` 台账的
    // 候选指纹挡住，用「可重跑 + 写去重」代替「宁可不跑」，不用拿合法重试换幂等。
    if let Some(path) = processed_path.as_deref()
        && let Err(error) = ReflectionLedgerRuntime::mark_started(path, &turn_key, now_ms())
    {
        axon_logger::warning(&format!("反思台账写入失败: {error}"));
    }
    let report = run_reflection_phase(request, response, trace);
    if report.failed_entry_count == 0 {
        if let Some(path) = processed_path.as_deref()
            && let Err(error) = ReflectionLedgerRuntime::mark_done(path, &turn_key, now_ms())
        {
            axon_logger::warning(&format!("反思台账收尾失败: {error}"));
        }
        // 只有跑完这一批才清 inflight：失败时留着，让下一次入池把整批卡片放回队列。
        if let Some(root) = ledger_root.as_deref() {
            ReflectionLedgerRuntime::clear_inflight(
                &ReflectionLedgerRuntime::inflight_path(root),
                &trace.turn_id,
            );
        }
    }
    report
}

/// 反思的实际执行阶段：触发器、子代理与写入都发生在这里。
fn run_reflection_phase(
    request: &RunRequest,
    response: &RunResponse,
    trace: &TurnTrace,
) -> PostRunSelfIterationReport {
    let turn_outcome = build_turn_outcome(request, response);
    let semantic_outcome = assess_semantic_salience(request, &turn_outcome);
    let cards = reflection_cards(trace, semantic_outcome.assessment.as_ref(), &request.cwd);
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
    // 卡片先入池并判定批处理：未触发就留到下次，触发才跑，避免每轮都起一个子代理。
    let cards = match admit_reflection_batch(
        request,
        trace,
        cards,
        semantic_outcome.assessment.as_ref(),
        &turn_outcome,
    ) {
        BatchAdmission::Run(cards) => {
            // 先记 1：后续 plan 失败等提前返回也要让本轮留下一条回执，
            // 否则「决定了要跑却没跑成」在审计里完全不可见。
            report.quality.reflection_attempts = 1;
            cards
        }
        BatchAdmission::Deferred => {
            report.quality.reflection_deferred = 1;
            return report;
        }
        BatchAdmission::NotApplicable => return report,
    };
    // 攒批可能跨多轮，每张卡自带上轮的用户消息引用。个人类候选要求
    // `evidence_refs` 含 user_message_ref，若一律拿「最后一轮」去比，批次里早期卡片的
    // 候选会被判成缺引用而丢掉，所以把本批所有卡片的引用一起带进去。
    // 批次键：本批所有 card_id 排序后取摘要。重试时池顺序可能变（inflight 恢复到
    // 队尾），只取「第一张卡」会让批次号与 written 台账一起去重失效。
    let batch_key = batch_key_for(&cards, trace);
    let mut user_refs: Vec<String> = cards
        .iter()
        .map(|card| card.what.intent_ref.clone())
        .filter(|reference| !reference.is_empty())
        .collect();
    user_refs.push(turn_outcome.user_message_ref.clone());
    user_refs.sort();
    user_refs.dedup();
    // 批次显著度按维取最大：一批里只要有一张强卡，候选就按强卡的量级过准入；
    // 若沿用最后一轮（常常是弱卡或 Skip 轮）的向量，早期强卡的候选会被判成不达标。
    let batch_signals = cards
        .iter()
        .map(|card| card.signals)
        .fold(SalienceVector::default(), SalienceVector::merge_max);
    let input = self_iteration_input(
        request,
        turn_outcome,
        semantic_outcome.assessment,
        cards.clone(),
    );
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
    let mut failed_entry_count: usize = 0;
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
            _ => continue,
        };
        // 写入不再由子代理自己发起：这里收候选、过准入与配额、统一落库。
        let applied = apply_reflection_report(
            request,
            output.as_ref(),
            input.semantic_assessment.as_ref(),
            trace,
            &cards,
            &user_refs,
            batch_signals,
            &batch_key,
        );
        report.memory_write_count = report.memory_write_count.saturating_add(applied.written);
        report.quality.reflection_skipped_candidates = report
            .quality
            .reflection_skipped_candidates
            .saturating_add(applied.skipped);
        // 真实失败（写入报错、报告解析失败）不能让本轮记成已完成，否则重试被永久跳过。
        if applied.failed > 0 {
            failed_entry_count = failed_entry_count
                .saturating_add(usize::try_from(applied.failed).unwrap_or(usize::MAX));
        }
        if applied.written > 0 {
            report.quality.reflection_yields = report.quality.reflection_yields.saturating_add(1);
        }
        results.push(subagent_result(entry, hook, status, output.as_ref()));
    }
    let _ = SelfIterationRuntime.merge_delegation_results(&plan, 0, results);
    report.semantic_assessment = input.semantic_assessment.clone();
    report.failed_entry_count = failed_entry_count;
    report
}

#[derive(Debug, Clone)]
pub(super) struct SelfIterationExecutionOptions {
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

    pub(super) fn semantic_salience() -> Self {
        // 快速模型通道可能不支持 `json_schema` 严格模式；transport 会按需降级为
        // prompt-only 契约，输出仍由 `parse_semantic_assessment` 独立校验。
        let structured_output = ProviderStructuredOutputContract {
            schema: semantic_salience_output_schema(),
            mode: ProviderStructuredOutputMode::JsonSchema,
        };
        Self {
            structured_output: Some(structured_output),
            system_prompt_override: Some(semantic_salience_system_prompt()),
            tools_enabled: false,
            model_intent: RunSubagentModelIntent::FastDelegate,
        }
    }
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

pub(super) fn execute_self_iteration_subagent(
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
        // 自迭代子任务不接受调用方选型：始终按 options 的意图继承或走快速槽位。
        model_override: None,
        reasoning_effort_override: None,
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
        SEMANTIC_SALIENCE_LABEL => SEMANTIC_SALIENCE_SUMMARY,
        _ => SELF_ITERATION_MEMORY_SUMMARY,
    }
}

pub(super) fn session_id(request: &RunRequest) -> SessionId {
    SessionId(stable_numeric_id(&request.session_id).max(1))
}

pub(super) fn self_iteration_input(
    request: &RunRequest,
    turn_outcome: TurnOutcomeEnvelope,
    semantic_assessment: Option<SemanticSalienceAssessment>,
    reflection_cards: Vec<ReflectionCard>,
) -> SelfIterationInput {
    SelfIterationInput {
        is_subagent: !request.current_agent_path.is_root(),
        turn_outcome,
        reflection_cards,
        semantic_assessment,
        parent_total_cost_micros: 0,
        subagent_results: Vec::new(),
    }
}

/// 批次键：本批所有 card_id 排序后取摘要，顺序变化不影响结果。
///
/// 只取「第一张卡」会在重试时漂移，inflight 恢复把卡片放回队尾，首卡就换了人，
/// 批次号与 written 台账的候选去重会一起失效，同一条结论被写第二遍。
fn batch_key_for(cards: &[ReflectionCard], trace: &TurnTrace) -> String {
    if cards.is_empty() {
        return format!("reflection:{}", trace.turn_id);
    }
    let mut ids: Vec<&str> = cards.iter().map(|card| card.card_id.as_str()).collect();
    ids.sort_unstable();
    format!(
        "reflection:{}",
        axon_agent_runtime::MessageRef::digest_text(&ids.join("\u{1}"))
    )
}

#[cfg(test)]
#[path = "run_semantic_salience_tests.rs"]
mod semantic_salience_tests;
