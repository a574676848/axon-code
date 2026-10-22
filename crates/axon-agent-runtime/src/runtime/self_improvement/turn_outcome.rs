//! 任务结果事实包、执行证据与语义显著性合同。

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// 语义显著性输出 schema 版本。
pub const SEMANTIC_SALIENCE_SCHEMA_VERSION: u32 = 1;
/// 信号最大值，单位 basis point。
pub const SEMANTIC_SALIENCE_MAX_BASIS_POINTS: u16 = 10_000;
/// Personal 候选的稳定性阈值。
pub const PERSONAL_STABILITY_MIN_BASIS_POINTS: u16 = 7_000;
/// Personal 候选的未来复用价值阈值。
pub const PERSONAL_FUTURE_UTILITY_MIN_BASIS_POINTS: u16 = 7_000;
/// Personal 候选的 scope 置信度阈值。
pub const PERSONAL_SCOPE_CONFIDENCE_MIN_BASIS_POINTS: u16 = 7_000;
/// Experience/Knowledge/Procedure 候选的未来复用价值阈值。
pub const EXPERIENCE_FUTURE_UTILITY_MIN_BASIS_POINTS: u16 = 6_500;
/// Experience/Knowledge/Procedure 候选的执行验证阈值。
pub const EXPERIENCE_VERIFICATION_MIN_BASIS_POINTS: u16 = 7_000;

/// 单条运行时执行证据。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionEvidenceRecord {
    /// 本轮内稳定的证据引用。
    pub evidence_ref: String,
    /// 工具完整名称。
    pub tool_name: String,
    /// 工具执行状态。
    pub status: ExecutionEvidenceStatus,
    /// 证据类型。
    pub kind: ExecutionEvidenceKind,
    /// 是否由运行时回执确认完成。
    pub runtime_verified: bool,
    /// 后续验证回执引用。
    #[serde(default)]
    pub verification_refs: Vec<String>,
}

/// 执行证据状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExecutionEvidenceStatus {
    /// 工具已派发，但失败、取消或结果不明。
    Attempted,
    /// 运行时回执确认工具已实际执行，但尚无独立验证。
    Executed,
    /// 实际执行且存在运行时验证回执。
    Verified,
    /// 实际执行后的验证失败，与完成结论矛盾。
    Contradicted,
}

/// 执行证据类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExecutionEvidenceKind {
    /// 普通工具执行。
    ToolExecution,
    /// 文件系统写入。
    ArtifactWrite,
    /// 对结果的读取、检查或测试。
    Validation,
    /// Skill 指令加载；本身不代表 Skill 已完成执行。
    SkillInvocation,
}

/// 本轮执行证据索引。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionEvidenceIndex {
    /// 按运行顺序保存的回执事实。
    pub records: Vec<ExecutionEvidenceRecord>,
}

impl ExecutionEvidenceIndex {
    /// 是否存在已验证的非 Skill 工具执行。
    #[must_use]
    pub fn has_verified_execution(&self) -> bool {
        self.records.iter().any(|record| {
            record.runtime_verified
                && record.status == ExecutionEvidenceStatus::Verified
                && record.kind != ExecutionEvidenceKind::SkillInvocation
        })
    }

    /// 指定引用中是否至少存在一条已验证的非 Skill 执行证据。
    #[must_use]
    pub fn has_verified_evidence_ref(&self, evidence_refs: &[String]) -> bool {
        self.records.iter().any(|record| {
            record.runtime_verified
                && record.status == ExecutionEvidenceStatus::Verified
                && record.kind != ExecutionEvidenceKind::SkillInvocation
                && evidence_refs
                    .iter()
                    .any(|evidence_ref| evidence_ref == &record.evidence_ref)
        })
    }

    /// 返回所有可被 assessor 引用的证据引用。
    #[must_use]
    pub fn evidence_refs(&self) -> BTreeSet<&str> {
        self.records
            .iter()
            .flat_map(|record| {
                std::iter::once(record.evidence_ref.as_str())
                    .chain(record.verification_refs.iter().map(String::as_str))
            })
            .collect()
    }
}

/// 被选中 Skill 的实际执行状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectedSkillExecution {
    /// Skill 名称。
    pub skill_name: String,
    /// 是否有运行时回执证明其指令被加载并用于后续执行。
    pub execution_verified: bool,
    /// 支撑该结论的回执引用。
    #[serde(default)]
    pub evidence_refs: Vec<String>,
}

/// 主代理本轮终态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TurnFinalStatus {
    /// 正常完成。
    Completed,
    /// 执行失败。
    Failed,
    /// 被用户中断。
    Interrupted,
}

/// 供轻量 assessor 与深反思消费的有界事实包。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnOutcomeEnvelope {
    /// 会话 ID。
    pub conversation_id: String,
    /// 本轮 ID。
    pub turn_id: String,
    /// canonical 主代理路径。
    pub agent_path: String,
    /// 有界用户意图文本。
    pub user_intent_summary: String,
    /// 有界最终结果文本。
    pub assistant_outcome_summary: String,
    /// 用户消息证据引用。
    pub user_message_ref: String,
    /// 运行时执行证据。
    pub execution_evidence: ExecutionEvidenceIndex,
    /// 可选 Skill 执行状态。
    pub selected_skill: Option<SelectedSkillExecution>,
    /// 本轮终态。
    pub final_status: TurnFinalStatus,
}

impl TurnOutcomeEnvelope {
    /// 返回 assessor 可合法引用的全部证据引用。
    #[must_use]
    pub fn evidence_refs(&self) -> BTreeSet<&str> {
        let mut refs = self.execution_evidence.evidence_refs();
        refs.insert(self.user_message_ref.as_str());
        refs
    }
}

/// 语义显著性候选类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SemanticMemoryCandidateType {
    /// 用户画像。
    Profile,
    /// 用户长期偏好。
    Preference,
    /// 可复用实践经验。
    Experience,
    /// 稳定知识。
    Knowledge,
    /// 可复用流程。
    Procedure,
}

/// 语义显著性信号，统一使用 basis point。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticSalienceSignals {
    /// 与用户身份和长期背景的相关度。
    pub identity_relevance: u16,
    /// 结论跨时间保持成立的概率。
    pub stability: u16,
    /// 对未来任务的复用价值。
    pub future_utility: u16,
    /// 相对既有认知的新颖度。
    pub novelty: u16,
    /// 明确纠正旧认知的强度。
    pub correction_strength: u16,
    /// 运行时执行证据的闭环强度。
    pub verification_strength: u16,
    /// Personal 与 Workspace 归属判断置信度。
    pub scope_confidence: u16,
}

/// 轻量语义显著性评估结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticSalienceAssessment {
    /// 输出 schema 版本。
    pub schema_version: u32,
    /// 是否值得启动完整反思。
    pub should_reflect: bool,
    /// 候选记忆类型。
    #[serde(default)]
    pub candidate_types: Vec<SemanticMemoryCandidateType>,
    /// 多维语义信号。
    pub signals: SemanticSalienceSignals,
    /// 仅允许引用 envelope 内证据。
    #[serde(default)]
    pub evidence_refs: Vec<String>,
    /// 有界可审计原因。
    pub reason: String,
}

impl SemanticSalienceAssessment {
    /// 严格校验模型输出，任何越界或伪造引用都保守拒绝。
    #[must_use]
    pub fn is_valid_for(&self, envelope: &TurnOutcomeEnvelope) -> bool {
        if self.schema_version != SEMANTIC_SALIENCE_SCHEMA_VERSION
            || self.reason.trim().is_empty()
            || !signals_in_range(&self.signals)
            // Provider 兼容层不接受 schema 的 `uniqueItems`，去重约束改由运行时保守校验。
            || has_duplicates(&self.candidate_types)
            || has_duplicates(&self.evidence_refs)
        {
            return false;
        }
        if !self.should_reflect {
            return self.candidate_types.is_empty() && self.evidence_refs.is_empty();
        }
        if self.candidate_types.is_empty() || self.evidence_refs.is_empty() {
            return false;
        }
        let allowed_refs = envelope.evidence_refs();
        self.evidence_refs
            .iter()
            .all(|evidence_ref| allowed_refs.contains(evidence_ref.as_str()))
    }

    /// 将语义判断与确定性执行证据结合，决定是否启动完整反思。
    #[must_use]
    pub fn qualifies_for_reflection(&self, envelope: &TurnOutcomeEnvelope) -> bool {
        self.is_valid_for(envelope)
            && self.should_reflect
            && self
                .candidate_types
                .iter()
                .any(|candidate| match candidate {
                    SemanticMemoryCandidateType::Profile
                    | SemanticMemoryCandidateType::Preference => {
                        self.signals.stability >= PERSONAL_STABILITY_MIN_BASIS_POINTS
                            && self.signals.future_utility
                                >= PERSONAL_FUTURE_UTILITY_MIN_BASIS_POINTS
                            && self.signals.scope_confidence
                                >= PERSONAL_SCOPE_CONFIDENCE_MIN_BASIS_POINTS
                            && self
                                .evidence_refs
                                .iter()
                                .any(|reference| reference == &envelope.user_message_ref)
                    }
                    SemanticMemoryCandidateType::Experience
                    | SemanticMemoryCandidateType::Knowledge
                    | SemanticMemoryCandidateType::Procedure => {
                        self.signals.future_utility >= EXPERIENCE_FUTURE_UTILITY_MIN_BASIS_POINTS
                            && self.signals.verification_strength
                                >= EXPERIENCE_VERIFICATION_MIN_BASIS_POINTS
                            && envelope
                                .execution_evidence
                                .has_verified_evidence_ref(&self.evidence_refs)
                    }
                })
    }
}

fn signals_in_range(signals: &SemanticSalienceSignals) -> bool {
    [
        signals.identity_relevance,
        signals.stability,
        signals.future_utility,
        signals.novelty,
        signals.correction_strength,
        signals.verification_strength,
        signals.scope_confidence,
    ]
    .into_iter()
    .all(|value| value <= SEMANTIC_SALIENCE_MAX_BASIS_POINTS)
}

/// 判断列表是否包含重复项；列表规模固定很小，线性比较即可。
fn has_duplicates<T: PartialEq>(items: &[T]) -> bool {
    items
        .iter()
        .enumerate()
        .any(|(index, item)| items[..index].contains(item))
}

#[cfg(test)]
#[path = "turn_outcome_tests.rs"]
mod tests;
