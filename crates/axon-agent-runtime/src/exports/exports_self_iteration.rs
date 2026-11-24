//! 自我迭代与任务结果事实包对外导出。

pub use crate::builtin_turn_trace_tool::{
    TURN_TRACE_DEFAULT_MAX_CHARS, TURN_TRACE_MAX_CHARS_FIELD, TURN_TRACE_MAX_CHARS_LIMIT,
    TURN_TRACE_RECEIPT_REF_FIELD, TURN_TRACE_SEQUENCE_FIELD, TURN_TRACE_SESSION_ID_FIELD,
    TURN_TRACE_TOOL_ID, TURN_TRACE_TOOL_NAME, TURN_TRACE_TOOL_SCHEMA_VERSION,
    TURN_TRACE_TRACE_ID_FIELD,
};
pub use crate::persona_rules::{
    FatigueAdvice, PERSONA_RULES_AFFECT_SEQUENCE_MAX, PERSONA_RULES_ENABLED_DEFAULT,
    PersonaRuleAdvice, advise as advise_persona,
};
pub use crate::reflection_action::{
    AdmissionSkipReason, MemoryAction, PlannedAction, REFLECTION_BATCH_ADD_LIMIT,
    REFLECTION_TURN_ADD_LIMIT, ReflectionActionPlan, ReflectionCandidate, SkippedCandidate,
    candidates_from_assessment, plan_reflection_actions,
};
pub use crate::reflection_card::{
    BatchBlock, EvidencePointer, REFLECTION_BATCH_MAX_BYTES, REFLECTION_CARD_MAX_BYTES,
    REFLECTION_CARD_TOP_EVIDENCE_LIMIT, REFLECTION_CARD_TRIMMED_EVIDENCE_LIMIT,
    REFLECTION_MAX_SEGMENTS, REFLECTION_SEGMENT_MAX_RECEIPTS, ReflectionCard, SalienceVector,
    WhatBlock, WhenBlock, WhereBlock,
};
pub use crate::reflection_ledger::{
    REFLECTION_INFLIGHT_FILE, REFLECTION_PROCESSED_FILE, REFLECTION_WRITTEN_FILE,
    ReflectionLedgerRuntime,
};
pub use crate::reflection_pool::{
    BATCH_MAX_AGE_MS, BATCH_MIN_ACCUMULATED_BP, BATCH_MIN_CANDIDATES, BatchTrigger,
    BatchTriggerInput, CORRECTION_TRIGGER_BP, RECURRENCE_MIN_CARDS, ReflectionPool,
    ReflectionPoolEntry, SINGLE_CARD_MIN_BP, STALE_POOL_MS, card_salience_total,
    evaluate_batch_trigger, pool_bucket_key, split_pool_by_bucket,
};
pub use crate::reflection_pool_store::{
    REFLECTION_POOL_STORE_FILE, ReflectionPoolLock, ReflectionPoolStoreRuntime, take_cold_start,
};
pub use crate::reflection_report::{
    REFLECTION_REPORT_MAX_CANDIDATES, REFLECTION_REPORT_MAX_SKIPS,
    REFLECTION_REPORT_MAX_TEXT_CHARS, ReflectionCandidateDraft, ReflectionReportDraft,
    ReflectionSkipDraft, parse_reflection_report,
};
pub use crate::self_iteration::{
    SELF_ITERATION_MEMORY_PROMPT_MODE, SELF_ITERATION_MEMORY_TASK_MESSAGE,
    SELF_ITERATION_MEMORY_UPDATE_MAX_RUNTIME_MS, SelfIterationCostMerge, SelfIterationHookKind,
    SelfIterationHookPlan, SelfIterationInput, SelfIterationPlan, SelfIterationPlanReason,
    SelfIterationRuntime, SelfIterationSkip, SelfIterationSkipReason, SelfIterationSubagentResult,
    SelfIterationSubagentStatus,
};
pub use crate::turn_outcome::{
    EXECUTION_SPECIFICITY_MIN_BP, EXPERIENCE_FUTURE_UTILITY_MIN_BASIS_POINTS,
    EXPERIENCE_VERIFICATION_MIN_BASIS_POINTS, ExecutionEvidenceIndex, ExecutionEvidenceKind,
    ExecutionEvidenceRecord, ExecutionEvidenceStatus, PERSONAL_FUTURE_UTILITY_MIN_BASIS_POINTS,
    PERSONAL_SCOPE_CONFIDENCE_MIN_BASIS_POINTS, PERSONAL_STABILITY_MIN_BASIS_POINTS,
    REFLECTION_WEAK_MIN_BASIS_POINTS, ReflectionTier, SEMANTIC_SALIENCE_ACCEPTED_MIN_VERSION,
    SEMANTIC_SALIENCE_MAX_BASIS_POINTS, SEMANTIC_SALIENCE_SCHEMA_VERSION,
    SEMANTIC_SALIENCE_SPECIFICITY_SCHEMA_VERSION, SelectedSkillExecution,
    SemanticMemoryCandidateType, SemanticSalienceAssessment, SemanticSalienceSignals,
    TurnFinalStatus, TurnOutcomeEnvelope,
};
pub use crate::turn_trace::{
    AXON_DEFAULT_UTC_OFFSET_MINUTES, EvidenceRange, ExplicitSignals, HourBucket, MessageRef,
    TURN_TRACE_DIGEST_HEX_CHARS, TURN_TRACE_DIGEST_PLACEHOLDER, TURN_TRACE_SCHEMA_VERSION,
    ToolReceiptFingerprint, ToolReceiptRef, TurnTrace, hour_bucket_of,
};
