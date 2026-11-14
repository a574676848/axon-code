//! 反思报告的落地：候选 → 准入 → 三态动作 → 统一写入。
//!
//! 分工是硬边界：子代理只判断「有什么值得记」，「记不记、记几条、是替换还是归档」
//! 由运行时的纯函数决定，配额与三态动作不是子代理的自觉。

use axon_agent_runtime::{
    MemoryAction, ReflectionCandidate, ReflectionCard, ReflectionLedgerRuntime, SalienceVector,
    SemanticMemoryCandidateType, SemanticSalienceAssessment, TurnTrace, parse_reflection_report,
    plan_reflection_actions,
};

use crate::stable_ids::stable_numeric_id;

use super::{
    RunRequest,
    run_execution::now_ms,
    run_memory_write::{AgentMemoryWriteInput, AgentMemoryWriteRuntime},
    run_self_iteration_subagent::session_id,
    run_subagent_executor::RunSubagentExecutionOutput,
};

/// 一次反思报告的落地结果。
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct ReflectionApplyOutcome {
    /// 真正写进 Memory 的条数（含归档，归档计入写入是因为它确实改变了召回面）。
    pub(super) written: u64,
    /// 被准入、配额或能力缺口挡下的条数：这是「判过但没写」，重试也不会变。
    pub(super) skipped: u64,
    /// 真正失败、重试可能成功的条数：只有它为 0 才允许把本轮记成已完成。
    pub(super) failed: u64,
}

/// 候选自己引用的证据落在哪张卡上；命中的卡里有已验证执行才算「有执行证据」。
///
/// 不能拿「当前轮 trace」套给整批：批次跨多轮，早期强卡的候选会被判缺证据，
/// `receipt:<n>` 里的 n；非该形态返回 None。
fn receipt_index(reference: &str) -> Option<u32> {
    reference.strip_prefix("receipt:")?.parse::<u32>().ok()
}

/// 候选自己引用的证据落在哪张卡上；只在那张卡里确实有已验证执行时才算数。
///
/// 不能拿「当前轮 trace」套给整批：批次跨多轮，早期强卡的候选会被判缺证据，
/// 弱卡轮批次又会误判成有证据。一个 ref 都没命中区间时，退回「本批是否出现过
/// 已验证执行」，避免候选只引用用户消息时被一律判死。
fn candidate_has_verified_execution(cards: &[ReflectionCard], evidence_refs: &[String]) -> bool {
    let mut matched: Vec<&ReflectionCard> = Vec::new();
    for reference in evidence_refs {
        let Some(index) = receipt_index(reference) else {
            continue;
        };
        for card in cards {
            let first = receipt_index(&card.evidence_range.first_ref);
            let last = receipt_index(&card.evidence_range.last_ref);
            if let (Some(first), Some(last)) = (first, last)
                && (first..=last).contains(&index)
            {
                matched.push(card);
            }
        }
    }
    if matched.is_empty() {
        return cards
            .iter()
            .any(|card| card.evidence_range.verified_count > 0);
    }
    matched
        .iter()
        .any(|card| card.evidence_range.verified_count > 0)
}
/// 把反思子代理交回的候选清单落成真实写入。
pub(super) fn apply_reflection_report(
    request: &RunRequest,
    output: Option<&RunSubagentExecutionOutput>,
    assessment: Option<&SemanticSalienceAssessment>,
    trace: &TurnTrace,
    cards: &[ReflectionCard],
    user_refs: &[String],
    batch_signals: SalienceVector,
    batch_key: &str,
) -> ReflectionApplyOutcome {
    let (Some(output), Some(assessment)) = (output, assessment) else {
        return ReflectionApplyOutcome::default();
    };
    let Some(report) = parse_reflection_report(&output.summary) else {
        // 解析失败是「跑完了但没按契约说话」：留一行日志，但**不计入失败**。
        // 格式不合契约重试大概率还是不合，计成失败会把它变成永久重试；
        // 只有写入侧的存储故障才算真实失败。
        axon_logger::warning(&format!(
            "反思报告解析失败，正文 {} 字节，开头：{}",
            output.summary.len(),
            output.summary.chars().take(120).collect::<String>()
        ));
        return ReflectionApplyOutcome::default();
    };
    // 白名单来自本批所有卡片的证据区间，而不是当前轮 trace：批次跨多轮，
    // 拿当前轮的 refs 当白名单会把早期轮次的合法引用判成伪造，再兜底成当前轮的
    // refs，等于把候选挂到错的轮次上。
    let mut allowed_refs: Vec<String> = Vec::new();
    for card in cards {
        if let (Some(first), Some(last)) = (
            receipt_index(&card.evidence_range.first_ref),
            receipt_index(&card.evidence_range.last_ref),
        ) {
            for index in first..=last {
                allowed_refs.push(format!("receipt:{index}"));
            }
        }
    }
    allowed_refs.extend(user_refs.iter().cloned());
    allowed_refs.push(trace.user.message_ref.clone());
    allowed_refs.push(trace.assistant.message_ref.clone());
    allowed_refs.sort();
    allowed_refs.dedup();
    let (report, dropped) = report.sanitized(&allowed_refs);
    let mut outcome = ReflectionApplyOutcome {
        written: 0,
        skipped: u64::try_from(dropped.saturating_add(report.skipped.len())).unwrap_or(u64::MAX),
        failed: 0,
    };
    if report.candidates.is_empty() {
        return outcome;
    }
    // 候选的显著度取「批次显著度」，不取本轮评估：一批可能跨多轮，用最后一轮的
    // 向量会把早期强卡的候选压成不达标，也让 Skip 轮的补漏批次必然写不进去。
    let signals = batch_signals;
    let candidates: Vec<ReflectionCandidate> = report
        .candidates
        .iter()
        .map(|draft| ReflectionCandidate {
            kind: draft.kind,
            signals,
            scope_confidence_bp: assessment.signals.scope_confidence,
            correction_strength_bp: assessment.signals.correction_strength,
            evidence_refs: draft.evidence_refs.clone(),
            // 用候选自己引用的那条用户消息：批次里不同卡片对应不同轮次，
            // 一律取最后一轮会让早期卡片的个人类候选被判成缺引用。
            user_message_ref: user_refs
                .iter()
                .find(|reference| draft.evidence_refs.contains(reference))
                .cloned()
                .or_else(|| user_refs.first().cloned()),
            has_verified_execution: candidate_has_verified_execution(cards, &draft.evidence_refs),
            target_memory_id: draft.target_memory_id,
        })
        .collect();
    // 单轮 3 条与单批 5 条是两套配额：一张卡的批次按单轮算，跨轮批次只受单批上限约束。
    // 跨批时传 `None` 表示「本轮不设单轮上限」，不能拿 `usize::MAX` 当哨兵：
    // 判定是 `>=`，"置满"会被读成"恒耗尽"，整批新增全被跳过。
    let turn_quota_used = if cards.len() > 1 { None } else { Some(0) };
    let plan = plan_reflection_actions(&candidates, turn_quota_used, 0);
    outcome.skipped = outcome
        .skipped
        .saturating_add(u64::try_from(plan.skipped.len()).unwrap_or(u64::MAX));

    let runtime = AgentMemoryWriteRuntime::default().with_identity_root(
        super::run_session_store::resolved_axon_root(&request.config_path).ok(),
    );
    let now = now_ms();
    // 批次号就是 `memory_write` 的 turn_id 位：同一批重试时数值不变，库里的记录因此
    // 带上可辨认的批次身份；配合下面的候选指纹台账，半批失败后的重试不会写第二遍。
    let cell = AgentMemoryWriteRuntime::reflection_write_cell(stable_numeric_id(batch_key));
    let written_path = super::run_session_store::resolved_axon_root(&request.config_path)
        .ok()
        .map(ReflectionLedgerRuntime::written_path);
    let mut remaining: Vec<usize> = (0..candidates.len()).collect();
    for planned in &plan.actions {
        let Some(position) = remaining
            .iter()
            .position(|index| candidates[*index] == planned.candidate)
        else {
            continue;
        };
        let draft = &report.candidates[remaining.remove(position)];
        // 归档只退出召回，不写新记录；先处理，避免被下面的 kind 判定提前跳过。
        if planned.action == MemoryAction::Archive {
            match draft.target_memory_id.filter(|target| *target > 0) {
                Some(target) => {
                    let archived = runtime.archive(
                        &request.config_path,
                        &[u64::try_from(target).unwrap_or(u64::MAX)],
                        now,
                    );
                    match archived {
                        Ok(count) if count > 0 => {
                            outcome.written = outcome
                                .written
                                .saturating_add(u64::try_from(count).unwrap_or(0));
                        }
                        Ok(_) => {
                            outcome.skipped = outcome.skipped.saturating_add(1);
                        }
                        Err(error) => {
                            axon_logger::warning(&format!("反思归档写入失败: {error}"));
                            outcome.failed = outcome.failed.saturating_add(1);
                        }
                    }
                }
                None => {
                    // 没有目标 ID 的归档无处落地，记账而不是静默吞掉。
                    outcome.skipped = outcome.skipped.saturating_add(1);
                }
            }
            continue;
        }
        let Some(kind) = candidate_kind_label(draft.kind) else {
            outcome.skipped = outcome.skipped.saturating_add(1);
            continue;
        };
        let supersedes: Vec<u64> = match planned.action {
            MemoryAction::Add => Vec::new(),
            MemoryAction::Retire => match draft.target_memory_id.filter(|target| *target > 0) {
                Some(target) => vec![u64::try_from(target).unwrap_or(u64::MAX)],
                None => {
                    outcome.skipped = outcome.skipped.saturating_add(1);
                    continue;
                }
            },
            MemoryAction::Archive => unreachable!("归档分支已在前面处理"),
        };
        // 候选指纹：同批次重试时用它判断「这条是不是上次已经写进去了」。
        let digest = axon_agent_runtime::MessageRef::digest_text(&format!(
            "{kind}\u{1}{}\u{1}{}",
            draft.summary, draft.evidence
        ));
        if let Some(path) = written_path.as_deref()
            && ReflectionLedgerRuntime::is_written(path, batch_key, &digest)
        {
            // 上一轮已经写过这一条：记账但不重写，避免同一条结论在库里出现两份。
            outcome.skipped = outcome.skipped.saturating_add(1);
            continue;
        }
        let written = runtime.trigger(AgentMemoryWriteInput {
            config_path: &request.config_path,
            cwd: &request.cwd,
            session_id: session_id(request),
            cell_id: cell,
            kind,
            summary: &draft.summary,
            evidence: &draft.evidence,
            supersedes_memory_ids: &supersedes,
            now_ms: now,
            // 反思读取的是 root 代理本轮的证据，写入同样代表 root：若这里按子代理
            // 身份落库，profile/preference 会在作用域解析处被直接拒绝，永远写不进个人记忆。
            is_root_agent: request.current_agent_path.is_root(),
        });
        match written {
            Ok(report) if report.record.is_some() => {
                outcome.written = outcome.written.saturating_add(1);
                if let Some(path) = written_path.as_deref()
                    && let Err(error) =
                        ReflectionLedgerRuntime::mark_written(path, batch_key, &digest, now)
                {
                    axon_logger::warning(&format!("反思已写台账落盘失败: {error}"));
                }
            }
            // 记录为空表示被 MemoryAdmissionGate 拦下：这是判过不写，重试不会变。
            Ok(_) => {
                outcome.skipped = outcome.skipped.saturating_add(1);
            }
            // 写入报错是真实故障：计入失败，本轮不记 done，让重试有机会补上。
            Err(error) => {
                axon_logger::warning(&format!("反思记忆写入失败（{kind}）: {error}"));
                outcome.failed = outcome.failed.saturating_add(1);
            }
        }
    }
    outcome
}

/// 候选类型到 `memory_write` 的 kind 字面量；风格候选不经记忆线。
const fn candidate_kind_label(kind: SemanticMemoryCandidateType) -> Option<&'static str> {
    match kind {
        SemanticMemoryCandidateType::Profile => Some("profile"),
        SemanticMemoryCandidateType::Preference => Some("preference"),
        SemanticMemoryCandidateType::Experience => Some("experience"),
        SemanticMemoryCandidateType::Knowledge => Some("knowledge"),
        SemanticMemoryCandidateType::Procedure => Some("procedure"),
        SemanticMemoryCandidateType::Style => None,
    }
}
