//! Skill evolution 父 Agent 自闭环领域模型。

use crate::memory_experience::{
    summary_contains_sensitive_content, summary_looks_like_private_log,
};

/// Skill evolution 风险等级。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillEvolutionRisk {
    /// 仅文档型、追加型、可回滚的低风险改进。
    Low,
    /// 可能改变任务边界或需要更多证据，默认静默沉淀经验。
    Medium,
    /// 涉及权限、隐私、不可回滚或未确认方向，必须静默沉淀经验。
    High,
}

/// Skill 改进提案输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillImprovementProposalInput {
    /// skill 名称。
    pub skill_name: String,
    /// 可审计证据引用。
    pub evidence_ref: String,
    /// 问题摘要。
    pub problem_summary: String,
    /// 建议动作。
    pub suggested_action: String,
    /// 风险等级。
    pub risk: SkillEvolutionRisk,
}

/// 新 skill 草案提案输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillDraftProposalInput {
    /// skill 名称。
    pub skill_name: String,
    /// 可审计证据引用。
    pub evidence_ref: String,
    /// 问题摘要。
    pub problem_summary: String,
    /// 建议动作。
    pub suggested_action: String,
    /// 风险等级。
    pub risk: SkillEvolutionRisk,
}

/// Skill 改进提案。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillImprovementProposal {
    /// skill 名称。
    pub skill_name: String,
    /// 可审计证据引用。
    pub evidence_ref: String,
    /// 问题摘要。
    pub problem_summary: String,
    /// 建议动作。
    pub suggested_action: String,
    /// 风险等级。
    pub risk: SkillEvolutionRisk,
}

/// 新 skill 草案提案。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillDraftProposal {
    /// skill 名称。
    pub skill_name: String,
    /// 可审计证据引用。
    pub evidence_ref: String,
    /// 问题摘要。
    pub problem_summary: String,
    /// 建议动作。
    pub suggested_action: String,
    /// 风险等级。
    pub risk: SkillEvolutionRisk,
}

/// 中高风险静默经验。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillEvolutionSilentExperience {
    /// 可审计证据引用。
    pub evidence_ref: String,
    /// 问题摘要。
    pub problem_summary: String,
    /// 建议动作。
    pub suggested_action: String,
    /// 风险等级。
    pub risk: SkillEvolutionRisk,
}

/// Skill evolution 领域决策。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillEvolutionDecision {
    /// 低风险已有 skill 改进提案。
    ImprovementProposal(SkillImprovementProposal),
    /// 低风险新 skill 草案提案。
    DraftProposal(SkillDraftProposal),
    /// 中高风险只沉淀内部经验，不生成 skill 提案。
    SilentExperience(SkillEvolutionSilentExperience),
    /// 输入不满足闭环条件。
    Rejected(SkillEvolutionRejectReason),
}

/// Skill evolution 拒绝原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillEvolutionRejectReason {
    /// 缺少 skill 名称。
    MissingSkillName,
    /// 缺少问题摘要。
    MissingProblemSummary,
    /// 缺少建议动作。
    MissingSuggestedAction,
    /// 缺少证据引用。
    MissingEvidenceRef,
    /// 摘要包含 secret 或 token。
    SensitiveContentDetected,
    /// 摘要疑似包含完整私有日志。
    PrivateLogDetected,
}

/// Skill evolution runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct SkillEvolutionRuntime;

impl SkillEvolutionRuntime {
    /// 生成 post-run self-iteration 的已有 skill evolution 决策。
    #[must_use]
    pub fn improvement_proposal(
        &self,
        input: SkillImprovementProposalInput,
    ) -> SkillEvolutionDecision {
        if let Some(reason) = validate_skill_evolution_fields(
            &input.skill_name,
            &input.evidence_ref,
            &input.problem_summary,
            &input.suggested_action,
        ) {
            return SkillEvolutionDecision::Rejected(reason);
        }
        if input.risk != SkillEvolutionRisk::Low {
            return SkillEvolutionDecision::SilentExperience(SkillEvolutionSilentExperience {
                evidence_ref: input.evidence_ref,
                problem_summary: input.problem_summary,
                suggested_action: input.suggested_action,
                risk: input.risk,
            });
        }

        SkillEvolutionDecision::ImprovementProposal(SkillImprovementProposal {
            skill_name: input.skill_name,
            evidence_ref: input.evidence_ref,
            problem_summary: input.problem_summary,
            suggested_action: input.suggested_action,
            risk: input.risk,
        })
    }

    /// 生成 post-run self-iteration 的新 skill 草案 evolution 决策。
    #[must_use]
    pub fn draft_proposal(&self, input: SkillDraftProposalInput) -> SkillEvolutionDecision {
        if let Some(reason) = validate_skill_evolution_fields(
            &input.skill_name,
            &input.evidence_ref,
            &input.problem_summary,
            &input.suggested_action,
        ) {
            return SkillEvolutionDecision::Rejected(reason);
        }
        if input.risk != SkillEvolutionRisk::Low {
            return SkillEvolutionDecision::SilentExperience(SkillEvolutionSilentExperience {
                evidence_ref: input.evidence_ref,
                problem_summary: input.problem_summary,
                suggested_action: input.suggested_action,
                risk: input.risk,
            });
        }

        SkillEvolutionDecision::DraftProposal(SkillDraftProposal {
            skill_name: input.skill_name,
            evidence_ref: input.evidence_ref,
            problem_summary: input.problem_summary,
            suggested_action: input.suggested_action,
            risk: input.risk,
        })
    }
}

fn validate_skill_evolution_fields(
    skill_name: &str,
    evidence_ref: &str,
    problem_summary: &str,
    suggested_action: &str,
) -> Option<SkillEvolutionRejectReason> {
    if skill_name.trim().is_empty() {
        return Some(SkillEvolutionRejectReason::MissingSkillName);
    }
    if problem_summary.trim().is_empty() {
        return Some(SkillEvolutionRejectReason::MissingProblemSummary);
    }
    if suggested_action.trim().is_empty() {
        return Some(SkillEvolutionRejectReason::MissingSuggestedAction);
    }
    if evidence_ref.trim().is_empty() {
        return Some(SkillEvolutionRejectReason::MissingEvidenceRef);
    }
    if [skill_name, evidence_ref, problem_summary, suggested_action]
        .into_iter()
        .any(summary_contains_sensitive_content)
    {
        return Some(SkillEvolutionRejectReason::SensitiveContentDetected);
    }
    if [problem_summary, suggested_action]
        .into_iter()
        .any(summary_looks_like_private_log)
    {
        return Some(SkillEvolutionRejectReason::PrivateLogDetected);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{
        SkillDraftProposalInput, SkillEvolutionDecision, SkillEvolutionRisk, SkillEvolutionRuntime,
    };

    #[test]
    fn high_risk_draft_is_silent_experience() {
        let decision = SkillEvolutionRuntime.draft_proposal(SkillDraftProposalInput {
            skill_name: "repo-maintenance".to_owned(),
            evidence_ref: "axon-run:session-1".to_owned(),
            problem_summary: "涉及权限和跨仓库写入".to_owned(),
            suggested_action: "不要生成 skill，只沉淀经验".to_owned(),
            risk: SkillEvolutionRisk::High,
        });

        assert!(matches!(
            decision,
            SkillEvolutionDecision::SilentExperience(experience)
                if experience.risk == SkillEvolutionRisk::High
        ));
    }

    #[test]
    fn low_risk_draft_can_produce_proposal() {
        let problem_summary = "完成验证后的重复步骤".repeat(64);
        let decision = SkillEvolutionRuntime.draft_proposal(SkillDraftProposalInput {
            skill_name: "repo-maintenance".to_owned(),
            evidence_ref: "axon-run:session-1".to_owned(),
            problem_summary: problem_summary.clone(),
            suggested_action: "追加已验证命令说明".to_owned(),
            risk: SkillEvolutionRisk::Low,
        });

        assert!(matches!(
            decision,
            SkillEvolutionDecision::DraftProposal(proposal)
                if proposal.problem_summary == problem_summary
        ));
    }
}
