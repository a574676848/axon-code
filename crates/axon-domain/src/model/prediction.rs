//! 输入建议与 GhostText 领域合同。

use crate::{PaneId, SuggestionId, SuggestionUndoTokenId};

/// 默认建议展示最低置信度。
pub const SUGGESTION_DEFAULT_MIN_CONFIDENCE_BASIS_POINTS: u16 = 7_000;
/// GhostText 可展示文本最大字符数。
pub const GHOST_TEXT_MAX_CHARS: usize = 160;
/// 远控建议摘要最大字符数。
pub const REMOTE_SUGGESTION_SUMMARY_MAX_CHARS: usize = 120;
/// 默认 opt-out 文案。
pub const SUGGESTION_OPT_OUT_COPY: &str = "可在本项目关闭或降低此类建议";

/// 预测信号快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PredictionSignalSnapshot {
    /// Pane ID。
    pub pane_id: PaneId,
    /// 当前工作目录。
    pub cwd: String,
    /// Shell 类型。
    pub shell_kind: String,
    /// 历史命令尾部。
    pub history_tail: Vec<String>,
    /// Git 状态摘要。
    pub git_state: Option<String>,
    /// 当前选区摘要。
    pub selection: Option<String>,
    /// 最近失败摘要。
    pub recent_failures: Vec<String>,
    /// input buffer generation。
    pub input_generation: u64,
    /// cwd generation。
    pub cwd_generation: u64,
    /// git generation。
    pub git_generation: u64,
}

/// 建议类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuggestionKind {
    /// 命令补全。
    CommandCompletion,
    /// 下一步需求预测。
    NextDemand,
}

/// 建议安全等级。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuggestionSafetyClass {
    /// 只读或可逆草案。
    ReadOnlyDraft,
    /// 可能写 workspace。
    WritesWorkspace,
    /// 可能访问网络或安装依赖。
    NetworkOrInstall,
    /// 破坏性或提权。
    DestructiveOrPrivileged,
}

/// 建议副作用等级。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuggestionSideEffect {
    /// 无副作用。
    None,
    /// 只修改 input buffer。
    InputBufferOnly,
}

/// 建议展示模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuggestionDisplayMode {
    /// 内联 GhostText。
    GhostText,
    /// 命令面板候选。
    CommandPalette,
    /// Agent Pane 摘要卡。
    AgentPaneCard,
}

/// 接受建议后的最大效果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuggestionAcceptanceEffect {
    /// 只写本机 input buffer。
    InputBufferOnly,
    /// 被阻断。
    Blocked,
}

/// 建议阻断原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuggestionBlockReason {
    /// 用户或项目已关闭。
    DisabledByUser,
    /// cooldown 生效。
    CooldownActive,
    /// 置信度低于阈值。
    BelowConfidenceThreshold,
    /// 缺少解释文案。
    MissingExplanation,
    /// 风险不允许进入 GhostText。
    UnsafeForGhostText,
    /// 远控侧车不能执行。
    RemoteSidecarCannotExecute,
}

/// 预测候选。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuggestionCandidate {
    /// 建议 ID。
    pub id: SuggestionId,
    /// 建议类型。
    pub kind: SuggestionKind,
    /// 建议文本。
    pub text: String,
    /// 置信度，basis points。
    pub confidence_basis_points: u16,
    /// 安全等级。
    pub safety_class: SuggestionSafetyClass,
    /// 副作用等级。
    pub side_effect: SuggestionSideEffect,
    /// 解释文案。
    pub explanation: String,
    /// 首选展示模式。
    pub preferred_mode: SuggestionDisplayMode,
}

/// 建议策略快照。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SuggestionPolicy {
    /// 项目级关闭。
    pub project_disabled: bool,
    /// 最低展示置信度。
    pub min_confidence_basis_points: u16,
    /// 是否要求解释文案。
    pub require_explanation: bool,
    /// 当前时间。
    pub now_ms: u64,
    /// cooldown 截止时间。
    pub cooldown_until_ms: Option<u64>,
    /// 是否允许远控看到只读摘要。
    pub allow_remote_summary: bool,
}

impl Default for SuggestionPolicy {
    fn default() -> Self {
        Self {
            project_disabled: false,
            min_confidence_basis_points: SUGGESTION_DEFAULT_MIN_CONFIDENCE_BASIS_POINTS,
            require_explanation: true,
            now_ms: 0,
            cooldown_until_ms: None,
            allow_remote_summary: true,
        }
    }
}

/// Explainability copy。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandCompletionExplainabilityCopy {
    /// 使用的信号摘要。
    pub signals: Vec<String>,
    /// 置信度文案。
    pub confidence: String,
    /// 安全文案。
    pub safety: String,
    /// 副作用文案。
    pub side_effects: String,
    /// 接受效果文案。
    pub acceptance_effect: String,
    /// opt-out 入口文案。
    pub opt_out: String,
    /// 明确禁止暗示。
    pub forbidden_implications: Vec<SuggestionForbiddenImplication>,
}

/// 文案禁止暗示的能力。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuggestionForbiddenImplication {
    /// 自动执行。
    AutoExecute,
    /// 发送 Enter。
    SendEnter,
    /// 远控接受。
    RemoteAccept,
    /// 排序提升安全等级。
    SafetyUpgradeByRanking,
    /// 反馈直接写 strong memory。
    StrongMemoryFromFeedback,
}

/// GhostText 投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GhostTextProjection {
    /// 建议 ID。
    pub suggestion_id: SuggestionId,
    /// 投影文本。
    pub text: String,
    /// 解释文案。
    pub copy: CommandCompletionExplainabilityCopy,
    /// 接受后的最大效果。
    pub acceptance_effect: SuggestionAcceptanceEffect,
    /// 是否允许写入本机输入缓冲区。
    pub input_buffer_write_allowed: bool,
    /// 是否允许发送 Enter。
    pub send_enter_allowed: bool,
    /// 是否允许远控接受。
    pub remote_accept_allowed: bool,
    /// 是否允许直接写 strong memory。
    pub strong_memory_write_allowed: bool,
}

/// 建议展示决策。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SuggestionDisplayDecision {
    /// 展示建议。
    Displayed {
        /// 展示模式。
        mode: SuggestionDisplayMode,
        /// GhostText 投影。
        ghost_text: Option<Box<GhostTextProjection>>,
    },
    /// 阻断建议。
    Blocked {
        /// 阻断原因。
        reason: SuggestionBlockReason,
    },
}

/// 建议来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuggestionAcceptOrigin {
    /// 本机输入焦点。
    LocalInputFocus,
    /// 远控侧车。
    RemoteSidecar,
}

/// 建议接受请求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SuggestionAcceptRequest {
    /// 来源。
    pub origin: SuggestionAcceptOrigin,
    /// 请求效果。
    pub requested_effect: SuggestionAcceptanceEffect,
    /// undo token ID。
    pub undo_token_id: SuggestionUndoTokenId,
}

/// Undo token。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuggestionUndoRecord {
    /// token ID。
    pub token_id: SuggestionUndoTokenId,
    /// 建议 ID。
    pub suggestion_id: SuggestionId,
    /// 恢复前 input buffer。
    pub previous_input_buffer: String,
    /// 接受后 input buffer。
    pub accepted_input_buffer: String,
}

/// 建议接受决策。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SuggestionAcceptDecision {
    /// 已接受到 input buffer。
    AcceptedInputBuffer {
        /// 新 input buffer。
        input_buffer: String,
        /// undo 记录。
        undo: SuggestionUndoRecord,
        /// 是否允许写入本机输入缓冲区。
        input_buffer_write_allowed: bool,
        /// 是否允许发送 Enter。
        send_enter_allowed: bool,
        /// 是否允许远控接受。
        remote_accept_allowed: bool,
        /// 是否允许直接写 strong memory。
        strong_memory_write_allowed: bool,
    },
    /// 接受被阻断。
    Blocked {
        /// 阻断原因。
        reason: SuggestionBlockReason,
    },
}

/// 远控建议摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteSuggestionSummary {
    /// 建议 ID。
    pub suggestion_id: SuggestionId,
    /// bounded 文本。
    pub bounded_text: String,
    /// 本机复核提示。
    pub local_review_hint: String,
    /// 是否允许远控接受。
    pub remote_accept_allowed: bool,
    /// 是否允许远控写 input buffer。
    pub remote_write_input_buffer_allowed: bool,
    /// 是否允许远控发送 Enter。
    pub remote_send_enter_allowed: bool,
    /// 是否允许远控直接写 strong memory。
    pub remote_strong_memory_write_allowed: bool,
}

/// 反馈类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuggestionFeedbackKind {
    /// Escape 关闭。
    Dismissed,
    /// 明确拒绝。
    Rejected,
    /// 接受前被编辑。
    EditedBeforeAccept,
}

/// 反馈记忆准入。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuggestionFeedbackMemoryAdmission {
    /// 仅 ranking telemetry。
    TelemetryOnly,
    /// 需要本机复核。
    NeedsLocalReview,
    /// 被远控来源阻断。
    BlockedRemoteOrigin,
}

/// 反馈记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuggestionFeedbackRecord {
    /// 建议 ID。
    pub suggestion_id: SuggestionId,
    /// 反馈类型。
    pub kind: SuggestionFeedbackKind,
    /// 是否来自远控。
    pub remote_origin: bool,
    /// 记忆准入结论。
    pub memory_admission: SuggestionFeedbackMemoryAdmission,
    /// 是否创建 strong memory。
    pub creates_strong_memory: bool,
}

/// 预测领域门禁。
#[derive(Debug, Default, Clone, Copy)]
pub struct PredictionGate;

impl PredictionGate {
    /// 评估展示决策。
    /// 接受 GhostText 到本机输入缓冲区，不执行命令。
    #[must_use]
    pub fn display(
        signals: &PredictionSignalSnapshot,
        candidate: &SuggestionCandidate,
        policy: SuggestionPolicy,
    ) -> SuggestionDisplayDecision {
        if policy.project_disabled {
            return SuggestionDisplayDecision::Blocked {
                reason: SuggestionBlockReason::DisabledByUser,
            };
        }
        if policy
            .cooldown_until_ms
            .is_some_and(|until| policy.now_ms < until)
        {
            return SuggestionDisplayDecision::Blocked {
                reason: SuggestionBlockReason::CooldownActive,
            };
        }
        if candidate.confidence_basis_points < policy.min_confidence_basis_points {
            return SuggestionDisplayDecision::Blocked {
                reason: SuggestionBlockReason::BelowConfidenceThreshold,
            };
        }
        if policy.require_explanation && candidate.explanation.trim().is_empty() {
            return SuggestionDisplayDecision::Blocked {
                reason: SuggestionBlockReason::MissingExplanation,
            };
        }
        if candidate.preferred_mode == SuggestionDisplayMode::GhostText
            && !matches!(candidate.safety_class, SuggestionSafetyClass::ReadOnlyDraft)
        {
            return SuggestionDisplayDecision::Blocked {
                reason: SuggestionBlockReason::UnsafeForGhostText,
            };
        }

        let mode = candidate.preferred_mode;
        let ghost_text = (mode == SuggestionDisplayMode::GhostText).then(|| {
            let text = bounded_chars(&candidate.text, GHOST_TEXT_MAX_CHARS);
            Box::new(GhostTextProjection {
                suggestion_id: candidate.id,
                text,
                copy: explainability_copy(signals, candidate),
                acceptance_effect: SuggestionAcceptanceEffect::InputBufferOnly,
                input_buffer_write_allowed: false,
                send_enter_allowed: false,
                remote_accept_allowed: false,
                strong_memory_write_allowed: false,
            })
        });

        SuggestionDisplayDecision::Displayed { mode, ghost_text }
    }
    /// 接受 GhostText 到本机输入缓冲区，不执行命令。
    #[must_use]
    pub fn accept_to_input_buffer(
        projection: &GhostTextProjection,
        request: SuggestionAcceptRequest,
        current_input_buffer: &str,
    ) -> SuggestionAcceptDecision {
        if request.origin == SuggestionAcceptOrigin::RemoteSidecar {
            return SuggestionAcceptDecision::Blocked {
                reason: SuggestionBlockReason::RemoteSidecarCannotExecute,
            };
        }
        if request.requested_effect != SuggestionAcceptanceEffect::InputBufferOnly {
            return SuggestionAcceptDecision::Blocked {
                reason: SuggestionBlockReason::UnsafeForGhostText,
            };
        }

        let accepted_input_buffer = projection.text.clone();
        SuggestionAcceptDecision::AcceptedInputBuffer {
            input_buffer: accepted_input_buffer.clone(),
            undo: SuggestionUndoRecord {
                token_id: request.undo_token_id,
                suggestion_id: projection.suggestion_id,
                previous_input_buffer: current_input_buffer.to_owned(),
                accepted_input_buffer,
            },
            input_buffer_write_allowed: false,
            send_enter_allowed: false,
            remote_accept_allowed: false,
            strong_memory_write_allowed: false,
        }
    }

    /// 应用 undo token，仅恢复 input buffer。
    #[must_use]
    pub fn undo_input_buffer(record: &SuggestionUndoRecord) -> String {
        record.previous_input_buffer.clone()
    }

    /// 生成远控只读摘要。
    #[must_use]
    pub fn remote_summary(
        candidate: &SuggestionCandidate,
        policy: SuggestionPolicy,
    ) -> Option<RemoteSuggestionSummary> {
        policy
            .allow_remote_summary
            .then(|| RemoteSuggestionSummary {
                suggestion_id: candidate.id,
                bounded_text: bounded_chars(&candidate.text, REMOTE_SUGGESTION_SUMMARY_MAX_CHARS),
                local_review_hint: "请回到本机 Axon 终端查看和接受建议".to_owned(),
                remote_accept_allowed: false,
                remote_write_input_buffer_allowed: false,
                remote_send_enter_allowed: false,
                remote_strong_memory_write_allowed: false,
            })
    }

    /// 记录反馈，反馈默认不能直接写 strong memory。
    #[must_use]
    pub fn feedback(
        suggestion_id: SuggestionId,
        kind: SuggestionFeedbackKind,
        remote_origin: bool,
    ) -> SuggestionFeedbackRecord {
        let memory_admission = if remote_origin {
            SuggestionFeedbackMemoryAdmission::BlockedRemoteOrigin
        } else {
            match kind {
                SuggestionFeedbackKind::Dismissed | SuggestionFeedbackKind::Rejected => {
                    SuggestionFeedbackMemoryAdmission::TelemetryOnly
                }
                SuggestionFeedbackKind::EditedBeforeAccept => {
                    SuggestionFeedbackMemoryAdmission::NeedsLocalReview
                }
            }
        };

        SuggestionFeedbackRecord {
            suggestion_id,
            kind,
            remote_origin,
            memory_admission,
            creates_strong_memory: false,
        }
    }
}

fn explainability_copy(
    signals: &PredictionSignalSnapshot,
    candidate: &SuggestionCandidate,
) -> CommandCompletionExplainabilityCopy {
    CommandCompletionExplainabilityCopy {
        signals: vec![
            format!("cwd={}", signals.cwd),
            format!("shell={}", signals.shell_kind),
            format!("history_tail={}", signals.history_tail.len()),
            format!("recent_failures={}", signals.recent_failures.len()),
        ],
        confidence: format!("置信度 {}bp", candidate.confidence_basis_points),
        safety: format!("安全等级 {:?}", candidate.safety_class),
        side_effects: "不会自动执行，只生成输入缓冲区草案".to_owned(),
        acceptance_effect: "接受后只填入本机 input buffer，仍需用户自行按 Enter".to_owned(),
        opt_out: SUGGESTION_OPT_OUT_COPY.to_owned(),
        forbidden_implications: vec![
            SuggestionForbiddenImplication::AutoExecute,
            SuggestionForbiddenImplication::SendEnter,
            SuggestionForbiddenImplication::RemoteAccept,
            SuggestionForbiddenImplication::SafetyUpgradeByRanking,
            SuggestionForbiddenImplication::StrongMemoryFromFeedback,
        ],
    }
}

fn bounded_chars(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}
