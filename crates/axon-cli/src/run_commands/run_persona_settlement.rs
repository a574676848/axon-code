//! 轮后人格结算：把一轮的终态写回人格库。
//!
//! 结算键为 `settle:{session}:{part}:{status}`：同一轮同一终态重放不会重复生效。

use std::path::Path;

use axon_domain::DisclosurePolicy;
use axon_persona::{
    PersonaEvent, TurnFinalStatus, TurnSettledInput, UserAffect,
    domain::scene_frame::Audience,
    infrastructure::{PersonaStore, persona_store_path},
};

/// 一轮的结算输入。
pub(super) struct PersonaSettlementInput<'a> {
    /// Axon 配置文件路径，用于解析运行根。
    pub(super) config_path: &'a Path,
    /// 会话标识。
    pub(super) session_id: &'a str,
    /// 助手消息片段标识。
    pub(super) assistant_part_id: &'a str,
    /// 本轮受众信任域。
    pub(super) disclosure: DisclosurePolicy,
    /// 本轮终态。
    pub(super) final_status: TurnFinalStatus,
    /// 评估器推断出的用户状态。
    pub(super) user_affect: Option<&'a str>,
    /// 评估器推断出的用户疲劳度。
    pub(super) user_fatigue: Option<&'a str>,
    /// 表达漂移程度。
    pub(super) persona_drift_bp: Option<u16>,
    /// 漂移原因。
    pub(super) drift_reason: Option<&'a str>,
    /// 轮前场景帧记录的受众记号；存在时以其为准。
    pub(super) scene_audience: Option<&'a str>,
}

/// 结算一轮的人格状态；任何失败只记日志，不影响治理任务结论。
pub(super) fn settle_persona_turn(input: &PersonaSettlementInput<'_>) {
    let Ok(root) = super::run_session_store::resolved_axon_root(input.config_path) else {
        axon_logger::warning("persona_settlement: 无法解析 Axon 根目录");
        return;
    };
    // INV-6：群聊轮只读，既不写状态也不发布风格。轮前场景帧优先于披露策略。
    if !persona_write_allowed(input.disclosure, input.scene_audience) {
        axon_logger::debug("persona_settlement: 非私聊轮只读，跳过结算与发布");
        return;
    }
    let owner = crate::runtime_principal_context::load_run_principal(&root, "cli")
        .ok()
        .flatten()
        .map(|principal| principal.owner_id.as_str().to_owned());
    let Some(owner) = owner else {
        axon_logger::debug("persona_settlement: 无 owner 身份，跳过");
        return;
    };
    let path = persona_store_path(&root);
    let Ok(mut store) = PersonaStore::open(&path) else {
        axon_logger::warning("persona_settlement: 人格库不可用");
        return;
    };
    let event = PersonaEvent::TurnSettled(TurnSettledInput {
        final_status: input.final_status,
        user_affect: input
            .user_affect
            .map_or(UserAffect::Unknown, UserAffect::parse),
        fatigue_level: input
            .user_fatigue
            .and_then(axon_persona::FatigueLevel::parse),
        persona_drift_bp: input.persona_drift_bp.unwrap_or(0),
        drift_reason: input.drift_reason.map(ToOwned::to_owned),
        now_ms: axon_time::unix_now_ms(),
    });
    let idempotency_key = settlement_idempotency_key(
        input.session_id,
        input.assistant_part_id,
        input.final_status,
    );
    match store.settle(&owner, &event, Audience::Private, &idempotency_key) {
        Ok(outcome) => axon_logger::debug(&format!("persona_settlement: {outcome:?}")),
        Err(error) => axon_logger::warning(&format!("persona_settlement: {error}")),
    }
    // 发布受 24 小时限频保护：窗口内直接返回 None，不会额外写库。
    match store.publish_style_if_due(&owner, axon_time::unix_now_ms()) {
        Ok(Some(revision)) => axon_logger::debug(&format!(
            "persona_settlement: 习得风格已发布，revision={revision}"
        )),
        Ok(None) => {}
        Err(error) => axon_logger::warning(&format!("persona_settlement: 发布失败：{error}")),
    }
}

/// 记录评估器给出的表达风格候选；只接受 `style` 候选且要求有证据引用。
///
/// 只接受 `style` 候选、只接受私聊轮、只接受 `styleCandidate` 字段（不是 `reason`），
/// 还要过敏感内容与不可违背项去重。条目不直接生效：先入 pending，等发布窗口到点才进稳定段。
pub(super) fn capture_style_candidate(
    config_path: &Path,
    disclosure: DisclosurePolicy,
    assessment: Option<&axon_agent_runtime::SemanticSalienceAssessment>,
    user_message_ref: Option<&str>,
) {
    let Some(assessment) = assessment else {
        return;
    };
    // INV-6：群聊轮不采集风格，避免私聊稳定段被群聊内容污染。
    if disclosure != DisclosurePolicy::PrivateOnly {
        return;
    }
    if !assessment
        .candidate_types
        .contains(&axon_agent_runtime::SemanticMemoryCandidateType::Style)
    {
        return;
    }
    let Some(candidate) = assessment.style_candidate.as_deref() else {
        axon_logger::debug("persona_settlement: style 候选缺少 styleCandidate，忽略");
        return;
    };
    // 合同第 8.3 条：证据必须引用本轮用户消息，不能用其他引用顶替。
    let Some(evidence_ref) = select_user_evidence(&assessment.evidence_refs, user_message_ref)
    else {
        axon_logger::debug("persona_settlement: style 候选未引用本轮用户消息，忽略");
        return;
    };
    let Ok(root) = super::run_session_store::resolved_axon_root(config_path) else {
        return;
    };
    let Ok(soul) = std::fs::read_to_string(root.join("SOUL.md")) else {
        axon_logger::debug("persona_settlement: 人设不可读，跳过风格采集");
        return;
    };
    let invariants = axon_persona::SoulDocument::parse(&soul)
        .map(|document| document.core.invariants)
        .unwrap_or_default();
    let Ok(text) = axon_persona::application::style_candidate::validate_style_candidate(
        candidate,
        &invariants,
    ) else {
        axon_logger::debug("persona_settlement: 风格候选未通过校验，丢弃");
        return;
    };
    let owner = crate::runtime_principal_context::load_run_principal(&root, "cli")
        .ok()
        .flatten()
        .map(|principal| principal.owner_id.as_str().to_owned());
    let Some(owner) = owner else {
        return;
    };
    let Ok(mut store) = PersonaStore::open(&persona_store_path(&root)) else {
        return;
    };
    match store.add_pending_style(&owner, &text, evidence_ref, axon_time::unix_now_ms()) {
        Ok(true) => axon_logger::debug("persona_settlement: 已记录待发布风格"),
        Ok(false) => {}
        Err(error) => axon_logger::warning(&format!("persona_settlement: 记录风格失败：{error}")),
    }
}

/// 结算幂等键：同一轮同一终态重放必须命中同一条记录。
pub(super) fn settlement_idempotency_key(
    session_id: &str,
    assistant_part_id: &str,
    final_status: TurnFinalStatus,
) -> String {
    format!("settle:{session_id}:{assistant_part_id}:{final_status:?}")
}

/// 本轮是否允许写人格状态与习得风格。
///
/// 群聊只读：轮前场景帧记为 group，或披露边界不是私聊，都不写。
pub(super) fn persona_write_allowed(
    disclosure: DisclosurePolicy,
    scene_audience: Option<&str>,
) -> bool {
    disclosure == DisclosurePolicy::PrivateOnly && scene_audience != Some("group")
}

/// 在评估证据里挑出引用本轮用户消息的那一条。
pub(super) fn select_user_evidence<'a>(
    evidence_refs: &'a [String],
    user_message_ref: Option<&str>,
) -> Option<&'a String> {
    let expected = user_message_ref?;
    evidence_refs
        .iter()
        .find(|reference| reference.as_str() == expected)
}

#[cfg(test)]
#[path = "run_persona_settlement_tests.rs"]
mod tests;
