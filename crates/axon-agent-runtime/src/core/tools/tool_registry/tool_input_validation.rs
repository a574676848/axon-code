//! 工具输入格式与 JSON Schema 执行前校验。

use axon_domain::{ToolCallInput, ToolInputFormat};
use serde::Serialize;

use super::ToolRegistry;

/// 工具输入的单个字段级校验问题。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolInputValidationIssue {
    /// 实例 JSON Pointer；根节点为 /。
    pub instance_path: String,
    /// Schema JSON Pointer。
    pub schema_path: String,
    /// 面向模型的修复说明。
    pub message: String,
}

/// 工具输入校验失败报告。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolInputValidationError {
    /// 完整工具名。
    pub tool_name: String,
    /// 全部字段级问题。
    pub issues: Vec<ToolInputValidationIssue>,
}

impl std::fmt::Display for ToolInputValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "工具 {} 参数不符合 schema", self.tool_name)?;
        for issue in &self.issues {
            write!(
                formatter,
                "；{}: {}（schema {}）",
                issue.instance_path, issue.message, issue.schema_path
            )?;
        }
        Ok(())
    }
}

impl std::error::Error for ToolInputValidationError {}

impl ToolRegistry {
    /// 按完整工具名执行 JSON Schema 或 freeform 输入校验。
    pub fn validate_call_input(
        &self,
        qualified_name: &str,
        input: &ToolCallInput,
    ) -> Result<(), ToolInputValidationError> {
        let Some(tool) = self.tool_by_qualified_name(qualified_name) else {
            return Err(validation_error(qualified_name, "/", "/", "工具未注册"));
        };
        match (&tool.schema.input_format, input) {
            (ToolInputFormat::JsonSchema, ToolCallInput::Json(value)) => {
                let Some(validator) = self.validators.get(qualified_name) else {
                    return Err(validation_error(
                        qualified_name,
                        "/",
                        "/",
                        "工具 schema 校验器不存在",
                    ));
                };
                let mut issues = validator
                    .iter_errors(value)
                    .map(|error| ToolInputValidationIssue {
                        instance_path: pointer_path(error.instance_path().as_str()),
                        schema_path: pointer_path(error.schema_path().as_str()),
                        message: error.to_string(),
                    })
                    .collect::<Vec<_>>();
                issues.sort_by(|left, right| {
                    left.instance_path
                        .cmp(&right.instance_path)
                        .then_with(|| left.schema_path.cmp(&right.schema_path))
                        .then_with(|| left.message.cmp(&right.message))
                });
                if issues.is_empty() {
                    Ok(())
                } else {
                    Err(ToolInputValidationError {
                        tool_name: qualified_name.to_owned(),
                        issues,
                    })
                }
            }
            (ToolInputFormat::JsonSchema, ToolCallInput::Text(_)) => Err(validation_error(
                qualified_name,
                "/",
                "/type",
                "工具需要 JSON object，不能使用 freeform 文本",
            )),
            (ToolInputFormat::Freeform { .. }, ToolCallInput::Text(value))
                if !value.trim().is_empty() =>
            {
                Ok(())
            }
            (ToolInputFormat::Freeform { .. }, ToolCallInput::Text(_)) => Err(validation_error(
                qualified_name,
                "/",
                "/type",
                "freeform 输入不能为空",
            )),
            (ToolInputFormat::Freeform { .. }, ToolCallInput::Json(_)) => Err(validation_error(
                qualified_name,
                "/",
                "/type",
                "工具需要 freeform 文本；function fallback 必须先还原原始文本",
            )),
        }
    }
}

fn validation_error(
    tool_name: &str,
    instance_path: &str,
    schema_path: &str,
    message: &str,
) -> ToolInputValidationError {
    ToolInputValidationError {
        tool_name: tool_name.to_owned(),
        issues: vec![ToolInputValidationIssue {
            instance_path: instance_path.to_owned(),
            schema_path: schema_path.to_owned(),
            message: message.to_owned(),
        }],
    }
}

fn pointer_path(path: &str) -> String {
    if path.is_empty() {
        "/".to_owned()
    } else {
        path.to_owned()
    }
}
