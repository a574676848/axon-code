//! Memory 自治反馈写回。

use std::collections::BTreeSet;

use axon_domain::{AxonError, AxonResult, MemorySourceOrigin, MemoryState, MemoryTombstone};
use serde::Serialize;

use crate::{
    MemorySqliteRecord, memory_governance_policy::MEMORY_ARCHIVE_SCORE_THRESHOLD_BASIS_POINTS,
};

/// 记忆纠错：召回 scope 与当前任务不匹配。
pub const MEMORY_AUTONOMY_REASON_SCOPE_MISMATCH: &str = "scopeMismatch";
/// 记忆纠错：历史命令/调试经验复放失败。
pub const MEMORY_AUTONOMY_REASON_FAILED_REPLAY: &str = "failedReplay";
/// 记忆纠错：历史命令已过期或不再适合当前任务。
pub const MEMORY_AUTONOMY_REASON_STALE_COMMAND: &str = "staleCommand";
/// 记忆自治遗忘：强负反馈下记忆已不应继续召回。
pub const MEMORY_AUTONOMY_REASON_AUTONOMOUS_FORGET: &str = "autonomousForget";
/// 自治遗忘触发分数阈值，单位 basis point。
const MEMORY_AUTONOMY_FORGET_SCORE_THRESHOLD_BASIS_POINTS: u16 = 1_000;

/// 单次 run 结束后的 AI 自观测反馈。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryAutonomyFeedbackInput {
    /// 当前 SQLite 权威记录视图。
    pub records: Vec<MemorySqliteRecord>,
    /// 本轮被模型采用/引用的 content refs。
    pub adopted_hit_refs: Vec<String>,
    /// usefulness 判定需要降权的 content refs。
    pub downrank_hit_refs: Vec<String>,
    /// scope mismatch 的 content refs。
    pub scope_mismatch_hit_refs: Vec<String>,
    /// 失败工具数。
    pub failed_tool_count: usize,
    /// run 是否完成。
    pub run_completed: bool,
    /// 当前时间。
    pub now_ms: u64,
}

/// AI 自生成的记忆纠错事件；只包含 ref 和原因。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryAutonomyCorrectionEvent {
    /// 被纠错的 content ref。
    pub content_ref: String,
    /// 纠错原因。
    pub reason: &'static str,
}

/// AI 自治反馈报告。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryAutonomyFeedbackReport {
    /// 扫描记录数。
    pub scanned_records: usize,
    /// 本轮采用的记忆数。
    pub adopted_hit_count: usize,
    /// 本轮降权候选数。
    pub downrank_hit_count: usize,
    /// 升权记录数。
    pub boosted_records: usize,
    /// 降权记录数。
    pub downranked_records: usize,
    /// 自动归档记录数。
    pub archived_records: usize,
    /// 自动遗忘 tombstone 记录数。
    pub forgotten_records: usize,
    /// procedural/skill 记忆升权数。
    pub procedural_health_boosted_records: usize,
    /// procedural/skill 记忆降权数。
    pub procedural_health_downranked_records: usize,
    /// AI 自生成纠错事件。
    pub correction_events: Vec<MemoryAutonomyCorrectionEvent>,
    /// 是否保持 ref-only。
    pub ref_only: bool,
    /// 是否持久化 prompt 原文。
    pub prompt_text_persisted: bool,
    /// 是否持久化原始 payload。
    pub raw_payload_persisted: bool,
    /// 是否允许远控写入。
    pub remote_write_allowed: bool,
}

/// AI 自治反馈输出。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryAutonomyFeedbackOutput {
    /// 写回后的 SQLite 权威记录视图。
    pub records: Vec<MemorySqliteRecord>,
    /// 可暴露给 run JSON 的 ref-only 报告。
    pub report: MemoryAutonomyFeedbackReport,
}

/// Memory 自治反馈 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct MemoryAutonomyFeedbackRuntime;

impl MemoryAutonomyFeedbackRuntime {
    /// 将 run usefulness 反馈自动写回 ranking 元数据。
    pub fn apply(
        &self,
        input: MemoryAutonomyFeedbackInput,
    ) -> AxonResult<MemoryAutonomyFeedbackOutput> {
        validate_input(&input)?;
        let failed_tool_count = input.failed_tool_count;
        let run_completed = input.run_completed;
        let now_ms = input.now_ms;
        let adopted_refs = ref_set(input.adopted_hit_refs);
        let downrank_refs = ref_set(input.downrank_hit_refs);
        let scope_mismatch_refs = ref_set(input.scope_mismatch_hit_refs);
        let mut records = input.records;
        let mut report = MemoryAutonomyFeedbackReport {
            scanned_records: records.len(),
            adopted_hit_count: adopted_refs.len(),
            downrank_hit_count: downrank_refs.len(),
            boosted_records: 0,
            downranked_records: 0,
            archived_records: 0,
            forgotten_records: 0,
            procedural_health_boosted_records: 0,
            procedural_health_downranked_records: 0,
            correction_events: Vec::new(),
            ref_only: true,
            prompt_text_persisted: false,
            raw_payload_persisted: false,
            remote_write_allowed: false,
        };

        for record in &mut records {
            let Some(content_ref) =
                normalized_ref(record.record.content_ref.as_deref()).map(ToOwned::to_owned)
            else {
                continue;
            };
            let adopted = adopted_refs.contains(&content_ref);
            let scope_mismatch = scope_mismatch_refs.contains(&content_ref);
            let downranked = downrank_refs.contains(&content_ref) || scope_mismatch;
            if adopted && !downranked && run_completed && failed_tool_count == 0 {
                boost_record(record, now_ms);
                report.boosted_records = report.boosted_records.saturating_add(1);
                if is_procedural(record) {
                    report.procedural_health_boosted_records =
                        report.procedural_health_boosted_records.saturating_add(1);
                }
                continue;
            }
            if downranked {
                let strong = strong_negative(run_completed, failed_tool_count);
                let reason = correction_reason(record, scope_mismatch, failed_tool_count);
                downrank_record(record, now_ms, strong);
                report.downranked_records = report.downranked_records.saturating_add(1);
                if is_procedural(record) {
                    report.procedural_health_downranked_records = report
                        .procedural_health_downranked_records
                        .saturating_add(1);
                }
                if should_forget(record, strong) {
                    tombstone_record(record, &content_ref, now_ms);
                    report.forgotten_records = report.forgotten_records.saturating_add(1);
                } else if should_archive(record) {
                    record.archived_at_ms = Some(now_ms);
                    report.archived_records = report.archived_records.saturating_add(1);
                }
                report
                    .correction_events
                    .push(MemoryAutonomyCorrectionEvent {
                        content_ref,
                        reason,
                    });
            }
        }
        report.correction_events.sort_by(|left, right| {
            left.content_ref
                .cmp(&right.content_ref)
                .then(left.reason.cmp(right.reason))
        });
        report.correction_events.dedup();
        Ok(MemoryAutonomyFeedbackOutput { records, report })
    }
}

fn validate_input(input: &MemoryAutonomyFeedbackInput) -> AxonResult<()> {
    if input.now_ms == 0 {
        return Err(AxonError::runtime_failure("Memory 自治反馈输入无效"));
    }
    Ok(())
}

fn ref_set(values: Vec<String>) -> BTreeSet<String> {
    values
        .into_iter()
        .filter_map(|value| {
            let value = value.trim();
            (!value.is_empty()).then(|| value.to_owned())
        })
        .collect()
}

fn normalized_ref(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn boost_record(record: &mut MemorySqliteRecord, now_ms: u64) {
    record.confidence_basis_points = record
        .confidence_basis_points
        .saturating_add(500)
        .min(10_000);
    record.importance_basis_points = record
        .importance_basis_points
        .saturating_add(300)
        .min(10_000);
    record.last_active_at_ms = now_ms;
    record.updated_at_ms = now_ms;
    if record.confidence_basis_points >= 8_000 && record.importance_basis_points >= 7_000 {
        record.record.state = MemoryState::Strong;
    }
}

fn downrank_record(record: &mut MemorySqliteRecord, now_ms: u64, strong: bool) {
    let confidence_penalty = if strong { 1_500 } else { 700 };
    let importance_penalty = if strong { 1_000 } else { 400 };
    record.confidence_basis_points = record
        .confidence_basis_points
        .saturating_sub(confidence_penalty);
    record.importance_basis_points = record
        .importance_basis_points
        .saturating_sub(importance_penalty);
    record.updated_at_ms = now_ms;
    if record.record.state == MemoryState::Strong && record.confidence_basis_points < 5_000 {
        record.record.state = MemoryState::Weak;
    }
}

fn should_archive(record: &MemorySqliteRecord) -> bool {
    record.archived_at_ms.is_none()
        && governance_score_basis_points(record) < MEMORY_ARCHIVE_SCORE_THRESHOLD_BASIS_POINTS
}

fn should_forget(record: &MemorySqliteRecord, strong: bool) -> bool {
    strong
        && record.archived_at_ms.is_none()
        && governance_score_basis_points(record)
            < MEMORY_AUTONOMY_FORGET_SCORE_THRESHOLD_BASIS_POINTS
}

fn governance_score_basis_points(record: &MemorySqliteRecord) -> u16 {
    u16::try_from(u32::midpoint(
        u32::from(record.confidence_basis_points),
        u32::from(record.importance_basis_points),
    ))
    .unwrap_or(u16::MAX)
}

fn tombstone_record(record: &mut MemorySqliteRecord, blocked_ref: &str, now_ms: u64) {
    record.record.state = MemoryState::Tombstone;
    record.archived_at_ms = Some(now_ms);
    record.record.content_ref = None;
    record.record.tombstone = Some(MemoryTombstone {
        blocked_ref: blocked_ref.to_owned(),
        reason: MEMORY_AUTONOMY_REASON_AUTONOMOUS_FORGET.to_owned(),
    });
    record.updated_at_ms = now_ms;
}

fn strong_negative(run_completed: bool, failed_tool_count: usize) -> bool {
    !run_completed || failed_tool_count > 0
}

fn correction_reason(
    record: &MemorySqliteRecord,
    scope_mismatch: bool,
    failed_tool_count: usize,
) -> &'static str {
    if scope_mismatch {
        return MEMORY_AUTONOMY_REASON_SCOPE_MISMATCH;
    }
    if is_command_like(record) && failed_tool_count > 0 {
        return MEMORY_AUTONOMY_REASON_STALE_COMMAND;
    }
    MEMORY_AUTONOMY_REASON_FAILED_REPLAY
}

fn is_command_like(record: &MemorySqliteRecord) -> bool {
    matches!(
        record
            .record
            .source_trace
            .as_ref()
            .map(|trace| trace.origin),
        Some(
            MemorySourceOrigin::AgentExperience
                | MemorySourceOrigin::CommandCompletionFeedback
                | MemorySourceOrigin::DebuggingExperience
        )
    )
}

fn is_procedural(record: &MemorySqliteRecord) -> bool {
    matches!(
        record
            .record
            .source_trace
            .as_ref()
            .map(|trace| trace.origin),
        Some(MemorySourceOrigin::AgentProcedure | MemorySourceOrigin::SkillReflection)
    )
}

#[cfg(test)]
mod tests {
    use axon_domain::{MemoryCandidateId, MemoryId, MemoryKind, MemoryRecord, MemorySourceTrace};

    use super::*;

    #[test]
    fn boosts_adopted_success_and_downranks_failed_replay() {
        let output = MemoryAutonomyFeedbackRuntime
            .apply(MemoryAutonomyFeedbackInput {
                records: vec![
                    record(1, "mem:good", Some(MemorySourceOrigin::SkillReflection)),
                    record(
                        2,
                        "mem:bad",
                        Some(MemorySourceOrigin::CommandCompletionFeedback),
                    ),
                ],
                adopted_hit_refs: vec!["mem:good".to_owned()],
                downrank_hit_refs: vec!["mem:bad".to_owned()],
                scope_mismatch_hit_refs: Vec::new(),
                failed_tool_count: 0,
                run_completed: true,
                now_ms: 2_000,
            })
            .expect("自治反馈应成功");
        assert_eq!(output.report.boosted_records, 1);
        assert_eq!(output.report.downranked_records, 1);
        assert_eq!(output.report.procedural_health_boosted_records, 1);
        assert_eq!(
            output.report.correction_events[0].reason,
            MEMORY_AUTONOMY_REASON_FAILED_REPLAY
        );
        assert!(output.records[0].confidence_basis_points > 7_000);
        assert!(output.records[1].confidence_basis_points < 7_000);
    }

    #[test]
    fn archives_consecutive_low_value_records() {
        let mut low = record(1, "mem:low", None);
        low.confidence_basis_points = 1_700;
        low.importance_basis_points = 1_700;
        let output = MemoryAutonomyFeedbackRuntime
            .apply(MemoryAutonomyFeedbackInput {
                records: vec![low],
                adopted_hit_refs: Vec::new(),
                downrank_hit_refs: vec!["mem:low".to_owned()],
                scope_mismatch_hit_refs: Vec::new(),
                failed_tool_count: 0,
                run_completed: true,
                now_ms: 2_000,
            })
            .expect("自治反馈应成功");
        assert_eq!(output.report.archived_records, 1);
        assert_eq!(output.records[0].archived_at_ms, Some(2_000));
        assert_eq!(output.records[0].record.state, MemoryState::Weak);
    }

    #[test]
    fn forgets_strong_negative_low_value_records_with_tombstone() {
        let mut low = record(1, "mem:stale", None);
        low.confidence_basis_points = 1_200;
        low.importance_basis_points = 1_200;
        let output = MemoryAutonomyFeedbackRuntime
            .apply(MemoryAutonomyFeedbackInput {
                records: vec![low],
                adopted_hit_refs: Vec::new(),
                downrank_hit_refs: vec!["mem:stale".to_owned()],
                scope_mismatch_hit_refs: Vec::new(),
                failed_tool_count: 1,
                run_completed: false,
                now_ms: 2_000,
            })
            .expect("自治反馈应成功");
        assert_eq!(output.report.forgotten_records, 1);
        assert_eq!(output.records[0].record.state, MemoryState::Tombstone);
        assert_eq!(output.records[0].record.content_ref, None);
        assert_eq!(
            output.records[0]
                .record
                .tombstone
                .as_ref()
                .map(|item| item.blocked_ref.as_str()),
            Some("mem:stale")
        );
        assert_eq!(
            output.records[0]
                .record
                .tombstone
                .as_ref()
                .map(|item| item.reason.as_str()),
            Some(MEMORY_AUTONOMY_REASON_AUTONOMOUS_FORGET)
        );
    }

    fn record(
        memory_id: u64,
        content_ref: &str,
        source_origin: Option<MemorySourceOrigin>,
    ) -> MemorySqliteRecord {
        MemorySqliteRecord {
            memory_kind: MemoryKind::Knowledge,
            record: MemoryRecord {
                id: MemoryId(memory_id),
                candidate_id: MemoryCandidateId(memory_id),
                state: MemoryState::Weak,
                content_ref: Some(content_ref.to_owned()),
                source_trace: source_origin.map(|origin| MemorySourceTrace {
                    origin,
                    session_id: None,
                    cell_id: None,
                    summary: format!("summary-{memory_id}"),
                    evidence_ref: Some(format!("evidence-{memory_id}")),
                }),
                tombstone: None,
            },
            namespace_ref: Some("memory-scope:local".to_owned()),
            repo_ref: None,
            user_ref: None,
            created_at_ms: 1_000,
            updated_at_ms: 1_000,
            last_active_at_ms: 1_000,
            ttl_expires_at_ms: None,
            confidence_basis_points: 7_000,
            importance_basis_points: 7_000,
            vector_id: None,
            archived_at_ms: None,
            prompt_text_persisted: false,
            raw_payload_persisted: false,
            remote_write_allowed: false,
        }
    }
}
