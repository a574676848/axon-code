//! 自我迭代与任务结果事实包对外导出。

pub use crate::self_iteration::{
    SELF_ITERATION_MEMORY_PROMPT_MODE, SELF_ITERATION_MEMORY_TASK_MESSAGE,
    SELF_ITERATION_MEMORY_UPDATE_MAX_RUNTIME_MS, SELF_ITERATION_SKILL_REFLECTION_MAX_RUNTIME_MS,
    SELF_ITERATION_SKILL_REFLECTION_PROMPT_MODE, SELF_ITERATION_SKILL_REFLECTION_TASK_PREFIX,
    SelfIterationCostMerge, SelfIterationHookKind, SelfIterationHookPlan, SelfIterationInput,
    SelfIterationPlan, SelfIterationPlanReason, SelfIterationRuntime, SelfIterationSkillExecution,
    SelfIterationSkillSource, SelfIterationSkip, SelfIterationSkipReason,
    SelfIterationSubagentResult, SelfIterationSubagentStatus,
};
pub use crate::turn_outcome::{
    EXPERIENCE_FUTURE_UTILITY_MIN_BASIS_POINTS, EXPERIENCE_VERIFICATION_MIN_BASIS_POINTS,
    ExecutionEvidenceIndex, ExecutionEvidenceKind, ExecutionEvidenceRecord,
    ExecutionEvidenceStatus, PERSONAL_FUTURE_UTILITY_MIN_BASIS_POINTS,
    PERSONAL_SCOPE_CONFIDENCE_MIN_BASIS_POINTS, PERSONAL_STABILITY_MIN_BASIS_POINTS,
    SEMANTIC_SALIENCE_MAX_BASIS_POINTS, SEMANTIC_SALIENCE_SCHEMA_VERSION, SelectedSkillExecution,
    SemanticMemoryCandidateType, SemanticSalienceAssessment, SemanticSalienceSignals,
    TurnFinalStatus, TurnOutcomeEnvelope,
};
