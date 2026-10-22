//! IntentPredictionRuntime 的 M1 合同实现。

use axon_domain::{
    GhostTextProjection, PredictionGate, PredictionSignalSnapshot, RemoteSuggestionSummary,
    SuggestionAcceptDecision, SuggestionAcceptRequest, SuggestionCandidate,
    SuggestionDisplayDecision, SuggestionFeedbackKind, SuggestionFeedbackRecord, SuggestionId,
    SuggestionPolicy, SuggestionUndoRecord,
};

/// 预测 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct IntentPredictionRuntime;

impl IntentPredictionRuntime {
    /// 评估建议展示决策。
    #[must_use]
    pub fn display(
        &self,
        signals: &PredictionSignalSnapshot,
        candidate: &SuggestionCandidate,
        policy: SuggestionPolicy,
    ) -> SuggestionDisplayDecision {
        PredictionGate::display(signals, candidate, policy)
    }

    /// 接受 GhostText 到 input buffer。
    #[must_use]
    pub fn accept_ghost_text(
        &self,
        projection: &GhostTextProjection,
        request: SuggestionAcceptRequest,
        current_input_buffer: &str,
    ) -> SuggestionAcceptDecision {
        PredictionGate::accept_to_input_buffer(projection, request, current_input_buffer)
    }

    /// 撤销 GhostText 接受，只恢复 input buffer。
    #[must_use]
    pub fn undo_input_buffer(&self, record: &SuggestionUndoRecord) -> String {
        PredictionGate::undo_input_buffer(record)
    }

    /// 生成远控只读建议摘要。
    #[must_use]
    pub fn remote_summary(
        &self,
        candidate: &SuggestionCandidate,
        policy: SuggestionPolicy,
    ) -> Option<RemoteSuggestionSummary> {
        PredictionGate::remote_summary(candidate, policy)
    }

    /// 记录负反馈，不直接写 strong memory。
    #[must_use]
    pub fn record_feedback(
        &self,
        suggestion_id: SuggestionId,
        kind: SuggestionFeedbackKind,
        remote_origin: bool,
    ) -> SuggestionFeedbackRecord {
        PredictionGate::feedback(suggestion_id, kind, remote_origin)
    }
}
