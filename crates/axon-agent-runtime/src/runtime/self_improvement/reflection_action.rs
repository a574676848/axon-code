//! 反思产出的准入与三态动作。
//!
//! 准入不合成标量，按候选类型挑维度组合，全部满足才允许写入；
//! 弱档只降低「是否值得跑」的门槛，不降低「是否允许写入」的门槛。
//! 动作只有三态，明确不做 UPDATE：纠正旧认知等价于「新增 + supersede」。

use serde::{Deserialize, Serialize};

use crate::{
    CORRECTION_TRIGGER_BP, EXECUTION_SPECIFICITY_MIN_BP,
    EXPERIENCE_FUTURE_UTILITY_MIN_BASIS_POINTS, EXPERIENCE_VERIFICATION_MIN_BASIS_POINTS,
    PERSONAL_FUTURE_UTILITY_MIN_BASIS_POINTS, PERSONAL_SCOPE_CONFIDENCE_MIN_BASIS_POINTS,
    PERSONAL_STABILITY_MIN_BASIS_POINTS, SalienceVector, SemanticMemoryCandidateType,
    SemanticSalienceAssessment,
};

/// 单轮最多新增条数。
pub const REFLECTION_TURN_ADD_LIMIT: usize = 3;
/// 单批最多新增条数。
pub const REFLECTION_BATCH_ADD_LIMIT: usize = 5;

/// 三态记忆动作。明确不做 UPDATE。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MemoryAction {
    /// 新增：出现可复用新结论。
    Add,
    /// 淘汰：已召回的旧结论被证明失效或错误，旧 id 置 tombstone 并 supersede。
    Retire,
    /// 归档：旧结论暂不需要但未失效。
    Archive,
}

impl MemoryAction {
    /// 稳定的机器标识，用于回执。
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Retire => "retire",
            Self::Archive => "archive",
        }
    }

    /// 是否占用「新增」配额。
    #[must_use]
    pub const fn consumes_add_quota(self) -> bool {
        matches!(self, Self::Add)
    }
}

/// 准入未通过的原因；写进回执，不吞掉。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AdmissionSkipReason {
    /// 风格候选不经记忆线，由人格系统承接。
    StyleHandledByPersona,
    /// 缺 user_message_ref。
    MissingUserMessageRef,
    /// 缺运行时确认的执行证据。
    MissingVerifiedExecution,
    /// 某一维度未达门槛。
    BelowDimensionThreshold,
    /// 单轮或单批新增配额已满，按显著度降序截断。
    AddQuotaExhausted,
}

impl AdmissionSkipReason {
    /// 稳定的机器标识，用于回执的 `reason` 字段。
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::StyleHandledByPersona => "style_handled_by_persona",
            Self::MissingUserMessageRef => "missing_user_message_ref",
            Self::MissingVerifiedExecution => "missing_verified_execution",
            Self::BelowDimensionThreshold => "skipped_below_admission",
            Self::AddQuotaExhausted => "add_quota_exhausted",
        }
    }
}

/// 一条反思候选。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReflectionCandidate {
    /// 候选类型。
    pub kind: SemanticMemoryCandidateType,
    /// 五维显著度。
    pub signals: SalienceVector,
    /// 作用域归属置信度；personal 类必须达到门槛。
    pub scope_confidence_bp: u16,
    /// 用户当场纠正的强度；决定带目标的候选是淘汰还是归档。
    pub correction_strength_bp: u16,
    /// 证据引用。
    pub evidence_refs: Vec<String>,
    /// 本轮用户消息引用；personal 类的证据必须包含它。
    pub user_message_ref: Option<String>,
    /// 是否存在 runtime_verified 且非 Skill 调用的执行证据。
    pub has_verified_execution: bool,
    /// Retire 与 Archive 的目标记忆。
    pub target_memory_id: Option<i64>,
}

impl ReflectionCandidate {
    /// 准入判定：不通过时给出唯一原因。
    pub fn admission(&self) -> Result<(), AdmissionSkipReason> {
        if matches!(self.kind, SemanticMemoryCandidateType::Style) {
            return Err(AdmissionSkipReason::StyleHandledByPersona);
        }
        if self.is_personal() {
            if self.signals.persistence < PERSONAL_STABILITY_MIN_BASIS_POINTS
                || self.signals.utility < PERSONAL_FUTURE_UTILITY_MIN_BASIS_POINTS
                || self.scope_confidence_bp < PERSONAL_SCOPE_CONFIDENCE_MIN_BASIS_POINTS
            {
                return Err(AdmissionSkipReason::BelowDimensionThreshold);
            }
            let Some(user_ref) = self.user_message_ref.as_deref() else {
                return Err(AdmissionSkipReason::MissingUserMessageRef);
            };
            if !self
                .evidence_refs
                .iter()
                .any(|reference| reference == user_ref)
            {
                return Err(AdmissionSkipReason::MissingUserMessageRef);
            }
            return Ok(());
        }
        if self.signals.verification < EXPERIENCE_VERIFICATION_MIN_BASIS_POINTS
            || self.signals.utility < EXPERIENCE_FUTURE_UTILITY_MIN_BASIS_POINTS
            || self.signals.specificity < EXECUTION_SPECIFICITY_MIN_BP
        {
            return Err(AdmissionSkipReason::BelowDimensionThreshold);
        }
        if !self.has_verified_execution {
            return Err(AdmissionSkipReason::MissingVerifiedExecution);
        }
        Ok(())
    }

    /// 是否 personal 类候选。
    #[must_use]
    pub fn is_personal(&self) -> bool {
        matches!(
            self.kind,
            SemanticMemoryCandidateType::Profile | SemanticMemoryCandidateType::Preference
        )
    }

    /// 显著度排序键：五维之和；用于配额截断。
    #[must_use]
    pub fn salience_total(&self) -> u32 {
        self.signals.total()
    }
}

/// 一条计划中的动作。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedAction {
    /// 动作。
    pub action: MemoryAction,
    /// 候选本体。
    pub candidate: ReflectionCandidate,
}

/// 一条被跳过的候选。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkippedCandidate {
    /// 候选类型。
    pub kind: SemanticMemoryCandidateType,
    /// 跳过原因。
    pub reason: AdmissionSkipReason,
}

/// 一次批处理的计划结果。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReflectionActionPlan {
    /// 允许执行的动作。
    pub actions: Vec<PlannedAction>,
    /// 被跳过的候选与原因。
    pub skipped: Vec<SkippedCandidate>,
}

impl ReflectionActionPlan {
    /// 本次计划实际新增的条数。
    #[must_use]
    pub fn add_count(&self) -> usize {
        self.actions
            .iter()
            .filter(|planned| planned.action.consumes_add_quota())
            .count()
    }
}

/// 编排一次批处理的动作。
///
/// 先按准入过滤，再按显著度降序占用新增配额：单轮与单批各自独立计数，
/// 任一配额耗尽即整条跳过，原因记 `add_quota_exhausted`。
/// `added_this_turn` 为 `None` 表示本轮不设单轮上限（跨轮批次只看单批上限）。
/// Retire 与 Archive 不占新增配额，但同样要先过准入。
#[must_use]
pub fn plan_reflection_actions(
    candidates: &[ReflectionCandidate],
    added_this_turn: Option<usize>,
    added_this_batch: usize,
) -> ReflectionActionPlan {
    let mut plan = ReflectionActionPlan::default();
    let mut turn_used = added_this_turn;
    let mut batch_used = added_this_batch;

    let mut ordered: Vec<(usize, &ReflectionCandidate)> = candidates.iter().enumerate().collect();
    ordered.sort_by_key(|left| std::cmp::Reverse(left.1.salience_total()));

    let mut accepted: Vec<(usize, PlannedAction)> = Vec::new();
    let mut skipped_by_index: Vec<(usize, SkippedCandidate)> = Vec::new();

    for (index, candidate) in ordered {
        if let Err(reason) = candidate.admission() {
            skipped_by_index.push((
                index,
                SkippedCandidate {
                    kind: candidate.kind,
                    reason,
                },
            ));
            continue;
        }
        let action = resolve_action(candidate);
        if action.consumes_add_quota() {
            // `None` 表示「本轮不设单轮上限」：跨轮批次只受单批上限约束。
            // 不能用 `usize::MAX` 当哨兵：判定是 `>=`，"置满"会被读成"恒耗尽"。
            let turn_exhausted = turn_used.is_some_and(|used| used >= REFLECTION_TURN_ADD_LIMIT);
            if turn_exhausted || batch_used >= REFLECTION_BATCH_ADD_LIMIT {
                skipped_by_index.push((
                    index,
                    SkippedCandidate {
                        kind: candidate.kind,
                        reason: AdmissionSkipReason::AddQuotaExhausted,
                    },
                ));
                continue;
            }
            if let Some(used) = turn_used.as_mut() {
                *used += 1;
            }
            batch_used += 1;
        }
        accepted.push((
            index,
            PlannedAction {
                action,
                candidate: candidate.clone(),
            },
        ));
    }

    accepted.sort_by_key(|(index, _)| *index);
    skipped_by_index.sort_by_key(|(index, _)| *index);
    plan.actions = accepted.into_iter().map(|(_, planned)| planned).collect();
    plan.skipped = skipped_by_index
        .into_iter()
        .map(|(_, skipped)| skipped)
        .collect();
    plan
}

/// 由一次语义评估投影出候选集合。
///
/// 五维显著度统一走 `to_salience_vector`，因此 v3 与 v4 的 specificity 差异
/// 在进入准入前已被吸收，准入代码不必再关心评估版本。
/// `style` 候选一并保留：它由准入统一拒收并留痕，不在这里静默丢弃。
#[must_use]
pub fn candidates_from_assessment(
    assessment: &SemanticSalienceAssessment,
    user_message_ref: &str,
    has_verified_execution: bool,
) -> Vec<ReflectionCandidate> {
    let signals = assessment
        .signals
        .to_salience_vector(assessment.schema_version);
    assessment
        .candidate_types
        .iter()
        .map(|kind| ReflectionCandidate {
            kind: *kind,
            signals,
            scope_confidence_bp: assessment.signals.scope_confidence,
            correction_strength_bp: assessment.signals.correction_strength,
            evidence_refs: assessment.evidence_refs.clone(),
            user_message_ref: Some(user_message_ref.to_owned()),
            has_verified_execution,
            target_memory_id: None,
        })
        .collect()
}

/// 决定候选的三态动作。
///
/// 无目标记忆一律 Add；有目标时，用户当场纠正达到门槛才走 Retire（旧结论被判错），
/// 否则走 Archive（只是暂时不需要）。不引入 UPDATE。
fn resolve_action(candidate: &ReflectionCandidate) -> MemoryAction {
    if candidate.target_memory_id.is_none() {
        return MemoryAction::Add;
    }
    if candidate.correction_strength_bp >= CORRECTION_TRIGGER_BP {
        MemoryAction::Retire
    } else {
        MemoryAction::Archive
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signals(
        utility: u16,
        persistence: u16,
        verification: u16,
        specificity: u16,
    ) -> SalienceVector {
        SalienceVector {
            utility,
            persistence,
            verification,
            specificity,
            novelty: 7_000,
        }
    }

    fn procedural(utility: u16) -> ReflectionCandidate {
        ReflectionCandidate {
            kind: SemanticMemoryCandidateType::Procedure,
            signals: signals(utility, 7_000, 8_000, 8_000),
            scope_confidence_bp: 9_000,
            correction_strength_bp: 0,
            evidence_refs: vec!["receipt:1".to_owned()],
            user_message_ref: None,
            has_verified_execution: true,
            target_memory_id: None,
        }
    }

    fn personal(utility: u16, user_ref: Option<&str>) -> ReflectionCandidate {
        ReflectionCandidate {
            kind: SemanticMemoryCandidateType::Preference,
            signals: signals(utility, 7_500, 0, 0),
            scope_confidence_bp: 8_000,
            correction_strength_bp: 0,
            evidence_refs: user_ref
                .map(|reference| vec![reference.to_owned()])
                .unwrap_or_default(),
            user_message_ref: user_ref.map(str::to_owned),
            has_verified_execution: false,
            target_memory_id: None,
        }
    }

    #[test]
    fn style_never_enters_memory_line() {
        let mut candidate = procedural(9_000);
        candidate.kind = SemanticMemoryCandidateType::Style;

        assert_eq!(
            candidate.admission(),
            Err(AdmissionSkipReason::StyleHandledByPersona)
        );
    }

    #[test]
    fn personal_requires_user_message_evidence() {
        assert!(personal(8_000, Some("message:user:1")).admission().is_ok());
        assert_eq!(
            personal(8_000, None).admission(),
            Err(AdmissionSkipReason::MissingUserMessageRef)
        );
        assert_eq!(
            personal(6_000, Some("message:user:1")).admission(),
            Err(AdmissionSkipReason::BelowDimensionThreshold)
        );
    }

    #[test]
    fn execution_candidates_require_verified_evidence() {
        let mut candidate = procedural(8_000);
        candidate.has_verified_execution = false;

        assert_eq!(
            candidate.admission(),
            Err(AdmissionSkipReason::MissingVerifiedExecution)
        );
        assert_eq!(
            procedural(5_000).admission(),
            Err(AdmissionSkipReason::BelowDimensionThreshold)
        );
    }

    #[test]
    fn turn_quota_truncates_by_salience_descending() {
        let candidates = vec![
            procedural(9_000),
            procedural(7_000),
            procedural(8_000),
            procedural(6_600),
        ];

        let plan = plan_reflection_actions(&candidates, Some(0), 0);

        assert_eq!(plan.add_count(), REFLECTION_TURN_ADD_LIMIT);
        assert_eq!(plan.skipped.len(), 1);
        assert_eq!(
            plan.skipped[0].reason,
            AdmissionSkipReason::AddQuotaExhausted
        );
        assert_eq!(plan.actions[0].candidate.signals.utility, 9_000);
    }

    #[test]
    fn batch_quota_is_independent_from_turn_quota() {
        let candidates = vec![procedural(9_000), procedural(8_500)];

        let plan = plan_reflection_actions(&candidates, Some(0), REFLECTION_BATCH_ADD_LIMIT);

        assert!(plan.actions.is_empty());
        assert_eq!(plan.skipped.len(), 2);
    }

    #[test]
    fn actions_keep_candidate_order_after_quota_sorting() {
        let candidates = vec![procedural(7_000), procedural(9_000)];

        let plan = plan_reflection_actions(&candidates, Some(0), 0);

        assert_eq!(plan.actions[0].candidate.signals.utility, 7_000);
        assert_eq!(plan.actions[1].candidate.signals.utility, 9_000);
    }

    #[test]
    fn target_memory_resolves_retire_or_archive() {
        let mut retire = procedural(9_000);
        retire.target_memory_id = Some(42);
        retire.correction_strength_bp = CORRECTION_TRIGGER_BP;
        let mut archive = procedural(9_000);
        archive.target_memory_id = Some(43);

        let plan = plan_reflection_actions(&[retire, archive], Some(0), 0);

        assert_eq!(plan.actions[0].action, MemoryAction::Retire);
        assert_eq!(plan.actions[1].action, MemoryAction::Archive);
        assert_eq!(plan.add_count(), 0);
    }

    fn assessment_with(
        schema_version: u32,
        verification: u16,
        specificity: u16,
        kinds: Vec<SemanticMemoryCandidateType>,
    ) -> SemanticSalienceAssessment {
        SemanticSalienceAssessment {
            schema_version,
            should_reflect: true,
            candidate_types: kinds,
            signals: crate::SemanticSalienceSignals {
                future_utility: 8_000,
                stability: 7_500,
                verification_strength: verification,
                specificity,
                scope_confidence: 8_000,
                ..crate::SemanticSalienceSignals::default()
            },
            evidence_refs: vec!["receipt:1".to_owned()],
            reason: "边界用例".to_owned(),
            user_affect: None,
            user_fatigue: None,
            persona_drift_bp: None,
            drift_reason: None,
            style_candidate: None,
        }
    }

    #[test]
    fn assessment_v3_output_stays_admissible_for_execution() {
        let assessment = assessment_with(3, 9_000, 0, vec![SemanticMemoryCandidateType::Procedure]);
        let candidates = candidates_from_assessment(&assessment, "message:user:1", true);

        assert_eq!(candidates.len(), 1);
        assert_eq!(
            candidates[0].signals.specificity, EXECUTION_SPECIFICITY_MIN_BP,
            "v3 折中值必须落到门槛上，与升版前等价"
        );
        assert!(candidates[0].admission().is_ok());
    }

    #[test]
    fn assessment_v4_output_needs_real_specificity() {
        let assessment = assessment_with(
            4,
            9_000,
            6_000,
            vec![SemanticMemoryCandidateType::Procedure],
        );
        let candidates = candidates_from_assessment(&assessment, "message:user:1", true);

        assert_eq!(candidates[0].signals.specificity, 6_000);
        assert_eq!(
            candidates[0].admission(),
            Err(AdmissionSkipReason::BelowDimensionThreshold)
        );
    }

    #[test]
    fn assessment_keeps_style_candidate_for_audit() {
        let assessment = assessment_with(4, 9_000, 9_000, vec![SemanticMemoryCandidateType::Style]);
        let candidates = candidates_from_assessment(&assessment, "message:user:1", false);

        assert_eq!(candidates.len(), 1);
        assert_eq!(
            candidates[0].admission(),
            Err(AdmissionSkipReason::StyleHandledByPersona)
        );
    }
}
