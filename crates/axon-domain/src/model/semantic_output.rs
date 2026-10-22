use serde::{Deserialize, Serialize};
use std::fmt;

/// 语义输出文本的最大字符数。
pub const MAX_SEMANTIC_OUTPUT_LENGTH: usize = 100_000;

/// JARVIS 的渠道无关语义输出。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SemanticOutput {
    /// 直接回答用户。
    Answer {
        /// 回答正文。
        text: String,
    },
    /// 请求用户补充信息。
    Clarification {
        /// 澄清问题。
        question: String,
    },
    /// 展示当前工作的有界进度。
    Progress {
        /// 进度说明。
        text: String,
    },
    /// 持久工作完成后的语义回执。
    Completion {
        /// 完成摘要。
        text: String,
    },
    /// 以稳定错误码表达本轮失败。
    Failure {
        /// 稳定错误码。
        code: String,
        /// 面向用户的错误信息。
        message: String,
        /// 是否允许用户显式重试。
        retryable: bool,
    },
    /// 保留即时错误语义，供非持久运行时错误使用。
    Error {
        /// 稳定错误码。
        code: String,
        /// 面向用户的错误信息。
        message: String,
    },
}

impl SemanticOutput {
    /// 创建回答输出。
    pub fn answer(text: impl Into<String>) -> Result<Self, SemanticOutputError> {
        Ok(Self::Answer {
            text: validate_text(text.into(), "text")?,
        })
    }

    /// 创建澄清输出。
    pub fn clarification(question: impl Into<String>) -> Result<Self, SemanticOutputError> {
        Ok(Self::Clarification {
            question: validate_text(question.into(), "question")?,
        })
    }

    /// 创建进度输出。
    pub fn progress(text: impl Into<String>) -> Result<Self, SemanticOutputError> {
        Ok(Self::Progress {
            text: validate_text(text.into(), "text")?,
        })
    }

    /// 创建完成输出。
    pub fn completion(text: impl Into<String>) -> Result<Self, SemanticOutputError> {
        Ok(Self::Completion {
            text: validate_text(text.into(), "text")?,
        })
    }

    /// 创建失败输出。
    pub fn failure(
        code: impl Into<String>,
        message: impl Into<String>,
        retryable: bool,
    ) -> Result<Self, SemanticOutputError> {
        Ok(Self::Failure {
            code: validate_text(code.into(), "code")?,
            message: validate_text(message.into(), "message")?,
            retryable,
        })
    }

    /// 创建即时错误输出。
    pub fn error(
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Result<Self, SemanticOutputError> {
        Ok(Self::Error {
            code: validate_text(code.into(), "code")?,
            message: validate_text(message.into(), "message")?,
        })
    }
}

/// 语义输出校验错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticOutputError {
    /// 必填字段为空。
    EmptyField(&'static str),
    /// 字段超过长度上限。
    FieldTooLong(&'static str),
}

impl fmt::Display for SemanticOutputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyField(field) => write!(formatter, "{field} 不能为空"),
            Self::FieldTooLong(field) => {
                write!(
                    formatter,
                    "{field} 不能超过 {MAX_SEMANTIC_OUTPUT_LENGTH} 个字符"
                )
            }
        }
    }
}

impl std::error::Error for SemanticOutputError {}

fn validate_text(value: String, field: &'static str) -> Result<String, SemanticOutputError> {
    if value.is_empty() {
        return Err(SemanticOutputError::EmptyField(field));
    }
    if value.chars().count() > MAX_SEMANTIC_OUTPUT_LENGTH {
        return Err(SemanticOutputError::FieldTooLong(field));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_outputs_round_trip_without_channel_fields() {
        let values = [
            SemanticOutput::answer("已完成").expect("回答"),
            SemanticOutput::clarification("需要哪个 Workspace？").expect("澄清"),
            SemanticOutput::progress("正在执行").expect("进度"),
            SemanticOutput::completion("任务已完成").expect("完成"),
            SemanticOutput::failure("provider_timeout", "Provider 超时", true).expect("失败"),
            SemanticOutput::error("runtime_failed", "运行时失败").expect("即时错误"),
        ];
        for value in values {
            let encoded = serde_json::to_value(&value).expect("输出应可序列化");
            assert!(encoded.get("channel").is_none());
            let decoded: SemanticOutput =
                serde_json::from_value(encoded).expect("输出应可反序列化");
            assert_eq!(decoded, value);
        }
    }

    #[test]
    fn semantic_outputs_reject_empty_and_oversized_fields() {
        assert!(SemanticOutput::answer("").is_err());
        assert!(SemanticOutput::clarification("").is_err());
        assert!(SemanticOutput::error("", "message").is_err());
        assert!(SemanticOutput::failure("", "message", false).is_err());
        assert!(SemanticOutput::error("code", "").is_err());
        assert!(SemanticOutput::failure("code", "", false).is_err());
        let oversized = "x".repeat(MAX_SEMANTIC_OUTPUT_LENGTH + 1);
        assert!(SemanticOutput::answer(oversized).is_err());
    }
}
