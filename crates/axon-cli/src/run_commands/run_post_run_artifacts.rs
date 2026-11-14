//! 一轮治理后要落下的三份可见性产物：界面提示、审计回执、证据轨迹。
//!
//! 三份产物必须写在同一个 record 上、只提交一次：`commit_session_to_rollout` 按
//! `(session_id, assistant_part_id)` 整条覆盖，分两次提交会让后一次用旧 record
//! 把前一次刚写的字段抹掉。

use axon_agent_runtime::{
    SelfIterationHookKind, SelfIterationReceiptInput, SelfIterationReceiptRecord,
    SelfIterationReceiptStoreRuntime, SelfIterationSubagentStatus, SelfIterationWriteDecision,
};
use axon_memory::{MemoryRuntimeQualityObservation, MemoryRuntimeQualityStore};
use axon_memory_read::init_memory_orchestrator;

use super::{
    RunRequest,
    run_execution::RunResponse,
    run_post_run_governance_worker::DurableGovernancePayload,
    run_self_iteration_subagent::{PostRunSelfIterationQuality, PostRunSelfIterationReport},
    run_session_store::RunSessionRecord,
};

/// 一次提交本轮的三份可见性产物：界面提示、审计回执、证据轨迹。
///
/// 回执的落盘条件只看「本轮反思有没有真的跑过或入池」：跑过但零产出同样要留痕，
/// 否则「反思从来没产生过一条记忆」这种判断在审计里永远无法证伪。
pub(super) fn publish_turn_artifacts(
    payload: &DurableGovernancePayload,
    session_record: &RunSessionRecord,
    report: &PostRunSelfIterationReport,
    trace: &axon_agent_runtime::TurnTrace,
) {
    let mut record = session_record.clone();
    // 只在有提示时覆盖：已处理轮次的补写带的是默认报告，直接赋值会把首次写好的
    // 提示清空。
    if let Some(notice) = reflection_notice_text(report) {
        record.reflection_notice = Some(notice);
    }
    record.turn_trace = Some(trace.clone());
    let attempted = report.quality.reflection_attempts > 0
        || report.memory_write_count > 0
        || report.quality.reflection_deferred > 0;
    if attempted {
        append_reflection_receipt(payload, session_record, report);
    }
    let title = super::run_thread_title::derive_thread_title(&record.prompt);
    if let Err(error) =
        super::run_rollout_store::commit_session_to_rollout(&payload.config_path, &record, &title)
    {
        axon_logger::warning(&format!("本轮反思产物写回失败: {error}"));
    }
}

/// 用户可见提示：只在真正写入或有卡片入池待跑时出现，措辞与计数口径一致。
fn reflection_notice_text(report: &PostRunSelfIterationReport) -> Option<String> {
    if report.memory_write_count > 0 {
        let skipped = report.quality.reflection_skipped_candidates;
        return Some(if skipped > 0 {
            format!(
                "记忆反思：本轮新增、淘汰或归档 {} 条记忆，另有 {skipped} 条候选未写入",
                report.memory_write_count
            )
        } else {
            format!(
                "记忆反思：本轮新增、淘汰或归档 {} 条记忆",
                report.memory_write_count
            )
        });
    }
    if report.quality.reflection_deferred > 0 {
        return Some("记忆反思：本轮线索已入候选池，攒够阈值或池龄到点后统一整理".to_owned());
    }
    None
}

/// 把本轮反思写入落成一条可审计回执。
///
/// 回执是可见性的地基：它让「反思写了多少条、什么时候写的」在本机可查，不依赖任何后台调度器。
/// 落盘失败只告警，不影响治理主流程。
fn append_reflection_receipt(
    payload: &DurableGovernancePayload,
    session_record: &RunSessionRecord,
    report: &PostRunSelfIterationReport,
) {
    let Ok(root) = super::run_session_store::resolved_axon_root(&payload.config_path) else {
        axon_logger::warning("reflection receipt 落盘失败: 无法解析运行态根目录");
        return;
    };
    let path = SelfIterationReceiptStoreRuntime::resolve_store_path(root);
    let written = report.memory_write_count;
    let (status, decision, reason) = if written > 0 {
        (
            SelfIterationSubagentStatus::CompletedWithWrite,
            SelfIterationWriteDecision::Persisted,
            format!("reflection wrote {written} memories"),
        )
    } else if report.quality.reflection_deferred > 0 {
        (
            SelfIterationSubagentStatus::CompletedNoWrite,
            SelfIterationWriteDecision::NotAttempted,
            "reflection deferred to candidate pool".to_owned(),
        )
    } else {
        (
            SelfIterationSubagentStatus::CompletedNoWrite,
            SelfIterationWriteDecision::SkippedNoChange,
            "reflection produced no candidate".to_owned(),
        )
    };
    let input = SelfIterationReceiptInput {
        parent_session_id: session_record.session_id.clone(),
        parent_cell_id: None,
        recorded_at_ms: super::run_execution::now_ms(),
        hook: SelfIterationHookKind::MemoryUpdate,
        source_trace: format!(
            "self-iteration:memory_update:{}:{}",
            session_record.session_id, session_record.assistant_part_id
        ),
        status,
        cost_micros: 0,
        write_decision: decision,
        reason,
        summary_hash: None,
        hook_iterations: 0,
    };
    let record = match SelfIterationReceiptRecord::from_input(input) {
        Ok(record) => record,
        Err(error) => {
            axon_logger::warning(&format!("reflection receipt 组装失败: {error:?}"));
            return;
        }
    };
    if let Err(error) = SelfIterationReceiptStoreRuntime::append_record(&path, &record) {
        axon_logger::warning(&format!("reflection receipt 落盘失败: {error:?}"));
    }
}

/// 把本轮的质量观测写进质量表；写失败只告警，不影响主流程。
pub(super) fn record_runtime_quality(
    payload: &DurableGovernancePayload,
    session_record: &RunSessionRecord,
    config: &axon_config::AxonConfig,
    response: &RunResponse,
    quality: &PostRunSelfIterationQuality,
) -> Result<(), String> {
    let orchestrator = init_memory_orchestrator(&payload.config_path, config)?;
    let usage = response.token_usage.unwrap_or_default();
    MemoryRuntimeQualityStore::open(orchestrator.config())
        .map_err(|error| format!("{error:?}"))?
        .record_observation(&MemoryRuntimeQualityObservation {
            observation_id: format!(
                "post-run-governance:{}:{}",
                session_record.session_id, session_record.assistant_part_id
            ),
            cache_read_input_tokens: usage.cache_read_input_tokens,
            complete_input_tokens: usage.complete_input_tokens(),
            reflection_attempts: quality.reflection_attempts,
            reflection_deferred: quality.reflection_deferred,
            reflection_yields: quality.reflection_yields,
            semantic_assessments: quality.semantic_assessments,
            false_memory_rejections: quality.false_memory_rejections,
            recorded_at_ms: axon_time::unix_now_ms(),
        })
        .map_err(|error| format!("{error:?}"))
}

/// 构建本轮证据轨迹，供反思卡片与轨迹落盘共用。
///
/// 轨迹只含引用与指纹：输出指纹与字节数直接取自 `output_ref`（内容寻址，
/// 不读归档正文），参数指纹按 provider 调用标识从本轮工具调用里取。
/// 轮序按本轮自己的 `assistant_part_id` 点查，与 `builtin.turn_trace` 的
/// `trace:<session>:<sequence>` 口径一致。
pub(super) fn build_turn_trace(
    payload: &DurableGovernancePayload,
    session_record: &RunSessionRecord,
    request: &RunRequest,
    response: &RunResponse,
) -> axon_agent_runtime::TurnTrace {
    let outcome = super::run_turn_outcome::build_turn_outcome(request, response);
    // 治理队列是异步消费的，「按当前链长减一」在本轮之后又落了新轮次时会指向别的轮，
    // 两轮还会拿到同一个 trace_id，让 `builtin.turn_trace` 静默串轮。
    let sequence = sequence_option(payload, session_record);
    let now_ms = super::run_execution::now_ms();
    let signals = collect_explicit_signals(payload, session_record, now_ms, sequence);
    let mut trace = axon_agent_runtime::TurnTrace::from_outcome(
        &outcome,
        format!(
            "trace:{}:{}",
            session_record.session_id,
            sequence.unwrap_or(0)
        ),
        now_ms,
        signals,
    );
    trace.user = trace.user.with_digest(&outcome.user_intent_summary);
    trace.assistant = trace
        .assistant
        .with_digest(&outcome.assistant_outcome_summary);
    trace.apply_receipt_fingerprints(&collect_trace_fingerprints(response));
    trace
}

/// 本轮在会话内的轮序；查不到时返回 `None` 并告警。
///
/// 查不到时必须让调用方看得见：拿 0 兜底会让同一会话所有轮都撞成
/// `trace:<session>:0`，`card_id`、`batch_key` 与 `builtin.turn_trace` 的寻址一起失效。
/// 信号采集还需要分清「缺失」与「第 0 轮」，因此返回 `Option` 而不是就地退 0。
pub(super) fn sequence_option(
    payload: &DurableGovernancePayload,
    session_record: &RunSessionRecord,
) -> Option<u64> {
    match super::run_rollout_store::load_turn_sequence(
        &payload.config_path,
        &session_record.session_id,
        &session_record.assistant_part_id,
    ) {
        Ok(Some(sequence)) => Some(sequence),
        Ok(None) => {
            axon_logger::warning(&format!(
                "轮序缺失，本轮轨迹按序 0 落盘: session={} part={}",
                session_record.session_id, session_record.assistant_part_id
            ));
            None
        }
        Err(error) => {
            axon_logger::warning(&format!("轮序读取失败，本轮轨迹按序 0 落盘: {error:?}"));
            None
        }
    }
}

/// 近 30 分钟密度的统计窗口。
const EXPLICIT_SIGNALS_WINDOW_MS: u64 = 30 * 60 * 1_000;
/// 采集历史轮数的上限：够算连续失败与上一轮长度，又不把整条会话读进来。
const EXPLICIT_SIGNALS_HISTORY_TURNS: u32 = 24;

/// 采集本轮的零模型客观信号。
///
/// 全部取自 rollout 账本与时钟，不引入任何模型判定：这段代码只回答
/// 「第几轮、多密、上一轮多长、连败几次、上一轮状态序列」，
/// 判读交给 `persona_rules`。账本读不到时退化为默认值，不阻断收口。
///
/// `turns_last_30min` 按落盘时间统计并含本轮：治理队列是异步消费的，窗口会整体
/// 后移，因此它衡量的是「这台机器最近多密」，而不是「用户几点说的」。
fn collect_explicit_signals(
    payload: &DurableGovernancePayload,
    session_record: &RunSessionRecord,
    now_ms: u64,
    sequence: Option<u64>,
) -> axon_agent_runtime::ExplicitSignals {
    let facts = match super::run_rollout_store::load_recent_turn_facts(
        &payload.config_path,
        &session_record.session_id,
        EXPLICIT_SIGNALS_HISTORY_TURNS,
    ) {
        Ok(facts) => facts,
        Err(error) => {
            // 静默退化会让「规则层从不生效」变成第二个不可见故障，必须留痕。
            axon_logger::warning(&format!("显式信号采集失败，本轮按默认值结算: {error}"));
            Vec::new()
        }
    };
    let current = sequence.unwrap_or(0);
    let prior = || facts.iter().filter(move |fact| fact.sequence < current);
    let consecutive_turns = u32::try_from(current.saturating_add(1)).unwrap_or(u32::MAX);
    let window_start = now_ms.saturating_sub(EXPLICIT_SIGNALS_WINDOW_MS);
    let turns_last_30min = u32::try_from(
        prior()
            .filter(|fact| fact.recorded_at_ms >= window_start)
            .count()
            .saturating_add(1),
    )
    .unwrap_or(u32::MAX);
    // 账本按轮序倒序返回，第一条满足「小于本轮」的就是上一轮。
    let previous = prior().next();
    let current_chars = u32::try_from(session_record.prompt.chars().count()).unwrap_or(u32::MAX);
    let input_len_delta = previous.map_or(0, |fact| {
        i32::try_from(current_chars)
            .unwrap_or(i32::MAX)
            .saturating_sub(i32::try_from(fact.prompt_chars).unwrap_or(i32::MAX))
    });
    let failure_streak =
        u32::try_from(prior().take_while(|fact| fact.failed).count()).unwrap_or(u32::MAX);
    let mut affect_sequence = previous
        .map(|fact| fact.affect_sequence.clone())
        .unwrap_or_default();
    let affect_limit = axon_agent_runtime::PERSONA_RULES_AFFECT_SEQUENCE_MAX;
    if affect_sequence.len() > affect_limit {
        let overflow = affect_sequence.len() - affect_limit;
        affect_sequence.drain(..overflow);
    }
    axon_agent_runtime::ExplicitSignals {
        hour_bucket: axon_agent_runtime::hour_bucket_of(
            now_ms,
            axon_agent_runtime::AXON_DEFAULT_UTC_OFFSET_MINUTES,
        ),
        consecutive_turns,
        turns_last_30min,
        input_len_delta,
        affect_sequence,
        failure_streak,
    }
}

/// 把本轮评估出的用户状态追加进轨迹信号序列，供下一轮读回。
///
/// 只有评估器给出状态才追加：编造一个 `unknown` 会把「没评估」污染成「中性」。
pub(super) fn append_turn_affect(trace: &mut axon_agent_runtime::TurnTrace, affect: Option<&str>) {
    let Some(affect) = affect.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };
    trace.signals.affect_sequence.push(affect.to_owned());
    let limit = axon_agent_runtime::PERSONA_RULES_AFFECT_SEQUENCE_MAX;
    if trace.signals.affect_sequence.len() > limit {
        let overflow = trace.signals.affect_sequence.len() - limit;
        trace.signals.affect_sequence.drain(..overflow);
    }
}

/// 采集本轮每条工具回执的指纹；缺失参数或输出时留空串与 0，不伪造。
fn collect_trace_fingerprints(
    response: &RunResponse,
) -> Vec<axon_agent_runtime::ToolReceiptFingerprint> {
    response
        .tool_receipts
        .iter()
        .enumerate()
        .map(
            |(index, receipt)| axon_agent_runtime::ToolReceiptFingerprint {
                receipt_ref: format!("receipt:{}", index.saturating_add(1)),
                args_digest: receipt_args_digest(response, &receipt.provider_call_id),
                output_digest: receipt
                    .output_ref
                    .as_ref()
                    .map(|reference| reference.sha256.clone())
                    .unwrap_or_default(),
                byte_len: receipt
                    .output_ref
                    .as_ref()
                    .map(|reference| u32::try_from(reference.byte_len).unwrap_or(u32::MAX))
                    .unwrap_or(0),
            },
        )
        .collect()
}

/// 按 provider 调用标识取参数原文的指纹；找不到对应调用时留空串。
fn receipt_args_digest(response: &RunResponse, provider_call_id: &str) -> String {
    let Some(call) = response
        .tool_calls
        .iter()
        .find(|call| call.id == provider_call_id)
    else {
        return String::new();
    };
    let raw = match &call.input {
        axon_domain::ToolCallInput::Json(value) => value.to_string(),
        axon_domain::ToolCallInput::Text(text) => text.clone(),
    };
    axon_agent_runtime::MessageRef::digest_text(&raw)
}

#[cfg(test)]
mod notice_tests {
    use super::*;

    #[test]
    fn notice_states_follow_real_outcome() {
        let mut report = PostRunSelfIterationReport::default();
        assert!(
            reflection_notice_text(&report).is_none(),
            "既没写入也没入池时不应该出现提示"
        );

        report.quality.reflection_deferred = 1;
        assert!(
            reflection_notice_text(&report)
                .expect("待跑必须有提示")
                .contains("候选池")
        );

        report.memory_write_count = 2;
        report.quality.reflection_skipped_candidates = 1;
        let wording = reflection_notice_text(&report).expect("有写入必须有提示");
        assert!(wording.contains("2 条记忆"));
        assert!(wording.contains("1 条候选未写入"));
    }
}
