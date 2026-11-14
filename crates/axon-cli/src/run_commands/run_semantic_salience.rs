//! 语义显著性评估：把一轮的有界事实包交给快速模型，得到五维信号与候选类型。
//!
//! 评估只产出判断，不写记忆；它决定「这一轮值不值得反思」，以及候选进来后按什么
//! 量级过准入。输出被判无效时重试一次（格式类失败重试比重写提示便宜），仍不合法
//! 才整轮丢弃，并在质量表里记成一次拒绝。

use axon_agent_runtime::{
    SEMANTIC_SALIENCE_SCHEMA_VERSION, SemanticSalienceAssessment, SubagentDelegationEntry,
    SubagentInheritance, TurnOutcomeEnvelope,
};

use super::{
    RunRequest,
    run_self_iteration_subagent::{SelfIterationExecutionOptions, execute_self_iteration_subagent},
    run_subagent_executor::RunSubagentExecutionStatus,
};

/// 评估子任务的标签：会话投影与钩子匹配都靠它。
pub(super) const SEMANTIC_SALIENCE_LABEL: &str = "semantic_salience";
/// 轻量评估的单次预算。
const SEMANTIC_SALIENCE_MAX_RUNTIME_MS: u64 = 15_000;
const SEMANTIC_SALIENCE_SYSTEM_PROMPT: &str = "你是 AXON 的轻量语义显著性评估器。只判断随附 turn outcome 是否包含值得跨会话复用的长期信息，不执行工具、不延续任务、不依据关键词或正则做判断。执行事实只能来自 runtimeVerified=true 的 evidence；assistantOutcomeSummary 不能单独证明执行。用户明确、稳定、可复用的画像或偏好可引用 userMessageRef；experience、knowledge、procedure 必须同时有运行时执行证据。普通任务、临时状态和一次性事实必须 shouldReflect=false，此时 candidateTypes 与 evidenceRefs 必须为空。specificity 判断本轮结论是否指向具体对象（文件、命令、符号、字段）：泛泛而谈写低分，能说出确切对象才写高分。userAffect 判断用户在本轮表达出的状态，证据不足写 unknown。personaDrift 判断助手回复偏离人格底色的程度（0 至 10000 基点），正常遵循基线时写 0。用户疲劳度写进 userFatigue（low / normal / high）：从措辞、时段与连轴转迹象判断，拿不准填 normal。用户明确表达对说话方式、措辞、语气或详略的偏好时，candidateTypes 使用 style，并把这条偏好本身写进 styleCandidate（不超过 120 字），不要把判断理由写进去。personaDrift 达到 7000 时必须同时给出 driftReason（不超过 80 字），说明偏在哪。只输出符合 schema 的 JSON。";

/// 语义评估的最大尝试次数：格式类失败重试一次，仍不合法才整轮丢弃。
const SEMANTIC_SALIENCE_MAX_ATTEMPTS: u32 = 2;
/// 什么时候该判真：把「值得记」的口径写死，避免评估器一律保守判否。
const SEMANTIC_SALIENCE_DECISION_GUIDE: &str = "\n\n判定口径：只要本轮让助手对「怎么做事」或「这个人怎么工作」多知道一点，就写 shouldReflect=true —— 包括可复用的排查顺序、环境与依赖事实（路径、端口、服务职责、版本约束）、用户对工具与流程的偏好、被运行时验证有效的做法。只有纯查询、纯复述、临时状态与一次性事实才写 false；判 false 时 reason 必须写清「这轮为什么没有可复用结论」。";

/// 一次评估的结果：`completed` 表示子代理真的跑完，`rejected` 表示输出被判不合法。
#[derive(Debug, Default)]
pub(super) struct SemanticAssessmentOutcome {
    pub(super) assessment: Option<SemanticSalienceAssessment>,
    pub(super) completed: bool,
    pub(super) rejected: bool,
}

pub(super) fn assess_semantic_salience(
    request: &RunRequest,
    envelope: &TurnOutcomeEnvelope,
) -> SemanticAssessmentOutcome {
    // 只有 root 才评估：子代理的轮次不是用户的轮次，评出来的信号只会污染人格线。
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
    let mut outcome = SemanticAssessmentOutcome::default();
    for _ in 0..SEMANTIC_SALIENCE_MAX_ATTEMPTS {
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
        outcome = completed_semantic_assessment(output.summary.trim(), envelope);
        // 只在输出被判无效时重试：模型偶发不守格式，重试比重写提示更便宜。
        if !outcome.rejected {
            break;
        }
    }
    outcome
}

pub(super) fn completed_semantic_assessment(
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

pub(super) fn parse_semantic_assessment(
    raw: &str,
    envelope: &TurnOutcomeEnvelope,
) -> Option<SemanticSalienceAssessment> {
    let assessment: SemanticSalienceAssessment = serde_json::from_str(raw).ok()?;
    assessment.is_valid_for(envelope).then_some(assessment)
}

/// 评估输出的 JSON Schema；供结构化输出通道与 prompt-only 降级共用。
pub(super) fn semantic_salience_output_schema() -> serde_json::Value {
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
                    "scopeConfidence": signal.clone(),
                    "specificity": signal
                },
                "required": ["identityRelevance", "stability", "futureUtility", "novelty", "correctionStrength", "verificationStrength", "scopeConfidence", "specificity"]
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

/// 语义评估的系统提示：判定口径与 schema 一起构成完整契约。
pub(super) fn semantic_salience_system_prompt() -> String {
    format!("{SEMANTIC_SALIENCE_SYSTEM_PROMPT}{SEMANTIC_SALIENCE_DECISION_GUIDE}")
}
