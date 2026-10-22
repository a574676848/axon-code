//! 用户问答交互领域模型与状态机。

use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;

/// 默认的任务让位提示文案（面向用户，不讲内部状态机）。
pub const DEFAULT_SUSPENDED_HANDOFF: &str =
    "我需要你的回答才能继续这一步。你随时回答都可以，收到后我会接着往下做。";

/// 用户问答生命周期状态枚举（严格单向 CAS 前进）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserQuestionState {
    /// 等待用户作答。
    Pending,
    /// 用户已作答。
    Answered,
    /// 等待总预算耗尽，当前任务已挂起。
    Suspended,
    /// 已取消、中断或关停。
    Cancelled,
    /// 跨日超时已过期。
    Expired,
}

impl UserQuestionState {
    /// 返回可持久化的稳定小写字符串。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Answered => "answered",
            Self::Suspended => "suspended",
            Self::Cancelled => "cancelled",
            Self::Expired => "expired",
        }
    }

    /// 从持久化字符串解析状态。
    #[must_use]
    pub fn parse_state(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "answered" => Some(Self::Answered),
            "suspended" => Some(Self::Suspended),
            "cancelled" => Some(Self::Cancelled),
            "expired" => Some(Self::Expired),
            _ => None,
        }
    }

    /// 判断是否允许单向转换到目标状态。
    #[must_use]
    pub const fn can_transition_to(self, next: Self) -> bool {
        match (self, next) {
            // Pending 可刷新 deadline（保持 Pending），或前进到终态/挂起
            (Self::Pending, Self::Pending) => true,
            (Self::Pending, Self::Answered) => true,
            (Self::Pending, Self::Suspended) => true,
            (Self::Pending, Self::Cancelled) => true,
            (Self::Pending, Self::Expired) => true,

            // Suspended 仅可被回答、取消或超时过期
            (Self::Suspended, Self::Answered) => true,
            (Self::Suspended, Self::Cancelled) => true,
            (Self::Suspended, Self::Expired) => true,

            // Answered、Cancelled、Expired 为终态，不可逆
            _ => false,
        }
    }
}

impl fmt::Display for UserQuestionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// 领域校验错误。
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum UserQuestionValidationError {
    /// 必填字段缺失或为空。
    #[error("必填字段为空: {0}")]
    MissingRequiredField(&'static str),
    /// 非法的状态迁移尝试。
    #[error("状态迁移非法: 从 {from} 到 {to}")]
    IllegalStateTransition {
        /// 迁移前状态。
        from: UserQuestionState,
        /// 目标状态。
        to: UserQuestionState,
    },
    /// 已作答状态必须携带 answer_json。
    #[error("已作答状态 (answered) 必须携带 answer_json")]
    AnswerJsonRequired,
}

/// 用户问题选项（三渠道平台中立）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserQuestionOption {
    /// 选项展示标签。
    pub label: String,
    /// 选项描述或说明。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// 单个问题条目（三渠道平台中立）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserQuestionItem {
    /// 问题唯一标识。
    pub id: String,
    /// 问题正文。
    pub question: String,
    /// 问答卡片标题/分类。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    /// 辅助细节信息。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// 候选选项列表。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<UserQuestionOption>,
    /// 是否支持多选。
    ///
    /// 工具 schema 按全仓 snake_case 口径声明 `multi_select`，而本结构体对外序列化
    /// 是 camelCase；alias 让两种拼写都能进入解析，模型按 schema 传值不再被静默丢弃。
    #[serde(default, alias = "multi_select")]
    pub multi_select: bool,
}

/// 单个问题的作答条目（三渠道平台中立）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserAnswerItem {
    /// 对应问题 ID。
    pub id: String,
    /// 选中的选项标签列表。
    #[serde(default)]
    pub selected: Vec<String>,
    /// 自由输入自定义回答（单选时若非空可覆盖 selected）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom: Option<String>,
}

/// 用户问答权威记录（持久化于 Host 运行态库）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserQuestionRecord {
    /// 全局问题 ID（如 "uq-xxx"）。
    pub question_id: String,
    /// 幂等键（由 tool_call_id 与 turn 等信息生成）。
    pub idempotency_key: String,
    /// 所属车道 ID。
    pub lane_id: String,
    /// 所属会话 ID。
    pub session_id: String,
    /// 所属交互 ID。
    pub interaction_id: String,
    /// 所属轮次 ID。
    pub turn_id: String,
    /// 工具调用 ID。
    pub tool_call_id: String,
    /// 结构化问题 JSON（Vec<UserQuestionItem> 序列化字符串）。
    pub questions_json: String,
    /// 挂起交接正文（超预算挂起时显示）。
    pub handoff_text: Option<String>,
    /// 当前生命周期状态。
    pub state: UserQuestionState,
    /// 当前等待截止时刻毫秒时间戳。
    pub deadline_at_ms: i64,
    /// 用户作答 JSON（Vec<UserAnswerItem> 序列化字符串）。
    pub answer_json: Option<String>,
    /// 用户作答时刻毫秒时间戳。
    pub answered_at_ms: Option<i64>,
    /// 作答来源渠道（desktop / tui / feishu 等）。
    pub answered_channel: Option<String>,
    /// 答案是否已投递给等待者或下发到轮次。
    pub delivered: bool,
    /// 投递方式（tool_return / user_message）。
    pub delivery_mode: Option<String>,
    /// 创建时刻毫秒时间戳。
    pub created_at_ms: i64,
    /// 过期时刻毫秒时间戳。
    pub expires_at_ms: i64,
}

impl UserQuestionRecord {
    /// 校验当前记录是否满足领域不变量约束。
    pub fn validate(&self) -> Result<(), UserQuestionValidationError> {
        if self.question_id.trim().is_empty() {
            return Err(UserQuestionValidationError::MissingRequiredField(
                "question_id",
            ));
        }
        if self.idempotency_key.trim().is_empty() {
            return Err(UserQuestionValidationError::MissingRequiredField(
                "idempotency_key",
            ));
        }
        if self.lane_id.trim().is_empty() {
            return Err(UserQuestionValidationError::MissingRequiredField("lane_id"));
        }
        if self.session_id.trim().is_empty() {
            return Err(UserQuestionValidationError::MissingRequiredField(
                "session_id",
            ));
        }
        if self.interaction_id.trim().is_empty() {
            return Err(UserQuestionValidationError::MissingRequiredField(
                "interaction_id",
            ));
        }
        if self.turn_id.trim().is_empty() {
            return Err(UserQuestionValidationError::MissingRequiredField("turn_id"));
        }
        if self.tool_call_id.trim().is_empty() {
            return Err(UserQuestionValidationError::MissingRequiredField(
                "tool_call_id",
            ));
        }
        if self.questions_json.trim().is_empty() {
            return Err(UserQuestionValidationError::MissingRequiredField(
                "questions_json",
            ));
        }

        if self.state == UserQuestionState::Answered {
            match self.answer_json.as_deref() {
                Some(ans) if !ans.trim().is_empty() => {}
                _ => return Err(UserQuestionValidationError::AnswerJsonRequired),
            }
        }

        Ok(())
    }

    /// 尝试状态迁移并更新。
    pub fn transition_to(
        &mut self,
        next: UserQuestionState,
    ) -> Result<(), UserQuestionValidationError> {
        if !self.state.can_transition_to(next) {
            return Err(UserQuestionValidationError::IllegalStateTransition {
                from: self.state,
                to: next,
            });
        }
        self.state = next;
        Ok(())
    }

    /// 标记为已作答并写入作答详情。
    pub fn mark_answered(
        &mut self,
        answer_json: String,
        answered_at_ms: i64,
        channel: Option<String>,
        delivery_mode: Option<String>,
    ) -> Result<(), UserQuestionValidationError> {
        if answer_json.trim().is_empty() {
            return Err(UserQuestionValidationError::AnswerJsonRequired);
        }
        self.transition_to(UserQuestionState::Answered)?;
        self.answer_json = Some(answer_json);
        self.answered_at_ms = Some(answered_at_ms);
        self.answered_channel = channel;
        self.delivery_mode = delivery_mode;
        self.validate()
    }

    /// 标记为已挂起，若未提供 handoff_text 则使用默认文案。
    pub fn mark_suspended(
        &mut self,
        handoff_text: Option<String>,
    ) -> Result<(), UserQuestionValidationError> {
        self.transition_to(UserQuestionState::Suspended)?;
        self.handoff_text = Some(
            handoff_text
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_SUSPENDED_HANDOFF.to_string()),
        );
        self.validate()
    }

    /// 刷新等待截止时刻。
    pub fn extend_deadline(
        &mut self,
        new_deadline_at_ms: i64,
    ) -> Result<(), UserQuestionValidationError> {
        self.transition_to(UserQuestionState::Pending)?;
        self.deadline_at_ms = new_deadline_at_ms;
        Ok(())
    }

    /// 标记为已取消。
    pub fn mark_cancelled(&mut self) -> Result<(), UserQuestionValidationError> {
        self.transition_to(UserQuestionState::Cancelled)?;
        self.validate()
    }

    /// 标记为已过期。
    pub fn mark_expired(&mut self) -> Result<(), UserQuestionValidationError> {
        self.transition_to(UserQuestionState::Expired)?;
        self.validate()
    }

    /// 标记已投递。
    pub fn mark_delivered(&mut self) {
        self.delivered = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_record() -> UserQuestionRecord {
        UserQuestionRecord {
            question_id: "uq-001".to_string(),
            idempotency_key: "idem-001".to_string(),
            lane_id: "lane-main".to_string(),
            session_id: "sess-1".to_string(),
            interaction_id: "inter-1".to_string(),
            turn_id: "turn-1".to_string(),
            tool_call_id: "call-1".to_string(),
            questions_json: r#"[{"id":"q1","question":"确认部署？"}]"#.to_string(),
            handoff_text: None,
            state: UserQuestionState::Pending,
            deadline_at_ms: 30_000,
            answer_json: None,
            answered_at_ms: None,
            answered_channel: None,
            delivered: false,
            delivery_mode: None,
            created_at_ms: 0,
            expires_at_ms: 86_400_000,
        }
    }

    #[test]
    fn test_validation_non_empty_fields() {
        let mut record = create_test_record();
        assert!(record.validate().is_ok());

        record.question_id = "  ".to_string();
        assert_eq!(
            record.validate(),
            Err(UserQuestionValidationError::MissingRequiredField(
                "question_id"
            ))
        );

        let mut record2 = create_test_record();
        record2.tool_call_id = "".to_string();
        assert_eq!(
            record2.validate(),
            Err(UserQuestionValidationError::MissingRequiredField(
                "tool_call_id"
            ))
        );
    }

    #[test]
    fn test_state_single_direction_transitions() {
        let mut record = create_test_record();
        assert_eq!(record.state, UserQuestionState::Pending);

        // Pending -> Pending 允许（刷新 deadline）
        assert!(record.extend_deadline(60_000).is_ok());
        assert_eq!(record.deadline_at_ms, 60_000);

        // Pending -> Suspended 允许
        assert!(record.mark_suspended(None).is_ok());
        assert_eq!(record.state, UserQuestionState::Suspended);
        assert_eq!(
            record.handoff_text.as_deref(),
            Some(DEFAULT_SUSPENDED_HANDOFF)
        );

        // Suspended -> Answered 允许
        assert!(
            record
                .mark_answered(
                    r#"[{"id":"q1","selected":["是"]}]"#.to_string(),
                    70_000,
                    Some("desktop".to_string()),
                    Some("tool_return".to_string()),
                )
                .is_ok()
        );
        assert_eq!(record.state, UserQuestionState::Answered);

        // Answered 为终态，不可逆转为 Pending 或 Suspended
        assert!(record.transition_to(UserQuestionState::Pending).is_err());
        assert!(record.transition_to(UserQuestionState::Suspended).is_err());
    }

    #[test]
    fn test_answered_requires_answer_json() {
        let mut record = create_test_record();
        record.state = UserQuestionState::Answered;
        record.answer_json = None;
        assert_eq!(
            record.validate(),
            Err(UserQuestionValidationError::AnswerJsonRequired)
        );

        record.answer_json = Some("   ".to_string());
        assert_eq!(
            record.validate(),
            Err(UserQuestionValidationError::AnswerJsonRequired)
        );

        record.answer_json = Some(r#"[{"id":"q1","selected":["继续"]}]"#.to_string());
        assert!(record.validate().is_ok());
    }

    #[test]
    fn test_suspended_with_custom_or_default_handoff() {
        let mut record1 = create_test_record();
        assert!(
            record1
                .mark_suspended(Some("请选择分支后再继续".to_string()))
                .is_ok()
        );
        assert_eq!(record1.handoff_text.as_deref(), Some("请选择分支后再继续"));

        let mut record2 = create_test_record();
        assert!(record2.mark_suspended(None).is_ok());
        assert_eq!(
            record2.handoff_text.as_deref(),
            Some(DEFAULT_SUSPENDED_HANDOFF)
        );
    }

    #[test]
    fn test_json_roundtrip_platform_neutral() {
        let question = UserQuestionItem {
            id: "q_mode".to_string(),
            question: "请选择运行模式".to_string(),
            header: Some("模式选择".to_string()),
            detail: Some("生产或测试".to_string()),
            options: vec![
                UserQuestionOption {
                    label: "Prod".to_string(),
                    description: Some("生产环境".to_string()),
                },
                UserQuestionOption {
                    label: "Dev".to_string(),
                    description: None,
                },
            ],
            multi_select: false,
        };

        let json = serde_json::to_string(&question).expect("序列化应成功");
        // 确保无渠道特定字段（例如没有 desktop/tui/feishu 专有标记）
        assert!(!json.contains("channel"));
        assert!(!json.contains("feishu"));
        assert!(!json.contains("desktop"));

        let restored: UserQuestionItem = serde_json::from_str(&json).expect("反序列化应成功");
        assert_eq!(question, restored);

        let answer = UserAnswerItem {
            id: "q_mode".to_string(),
            selected: vec!["Prod".to_string()],
            custom: None,
        };
        let ans_json = serde_json::to_string(&answer).expect("答案序列化应成功");
        let ans_restored: UserAnswerItem =
            serde_json::from_str(&ans_json).expect("答案反序列化应成功");
        assert_eq!(answer, ans_restored);
    }
}
