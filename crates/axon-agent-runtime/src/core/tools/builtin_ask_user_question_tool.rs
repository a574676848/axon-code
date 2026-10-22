//! 内置 ask_user_question 工具定义与参数契约。

use axon_domain::{
    AxonError, AxonResult, ToolDefinition, ToolId, ToolOutputPolicy, ToolPermissionPolicy,
    ToolSchema, UserAnswerItem, UserQuestionItem,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::user_question_limits::{
    MAX_DETAIL_CHARS, MAX_OPTIONS_PER_QUESTION, MAX_QUESTION_CHARS, MAX_QUESTIONS_PER_CALL,
    validate_question_payload,
};

/// ask_user_question 工具简短名称。
pub const ASK_USER_QUESTION_TOOL_NAME: &str = "ask_user_question";
/// ask_user_question 工具完整限定名。
pub const ASK_USER_QUESTION_TOOL_QUALIFIED_NAME: &str = "builtin.ask_user_question";
/// ask_user_question 工具 schema 版本号。
pub const ASK_USER_QUESTION_TOOL_SCHEMA_VERSION: u32 = 1;
/// ask_user_question 工具 ID。
pub const ASK_USER_QUESTION_TOOL_ID: ToolId = ToolId(24);
/// ask_user_question 工具执行回执 kind 标识。
pub const ASK_USER_QUESTION_RECEIPT_KIND: &str = ASK_USER_QUESTION_TOOL_NAME;

/// 工具描述最大字符上限（守卫 B11）。
pub const ASK_USER_QUESTION_MAX_DESCRIPTION_CHARS: usize = 200;

/// 折叠摘要最大字符上限：它是折叠行里唯一可见的文本，必须一行放得下。
pub const ASK_USER_QUESTION_MAX_SUMMARY_CHARS: usize = 60;

/// 工具描述（只描述模型可感知的运行时事实：何时用、会发生什么；不复述字段与上限）。
pub const ASK_USER_QUESTION_DESCRIPTION: &str = "需要用户做决定，或缺少只有用户才知道的信息时使用。提问后你会等待回答：等到答案就继续；若用户没有及时回答，这一步会先交回用户，用户之后回答时答案会送到你这里。没有得到回答前，不要把它当成用户的同意或默认选择。";

/// 把校验错误码翻成模型可读、可据以改正的说明。
fn validation_error_message(code: &str) -> String {
    match code {
        "EMPTY_QUESTIONS" => "至少要提出一个问题".to_owned(),
        "EXCEEDED_MAX_QUESTIONS" => format!("一次最多提 {MAX_QUESTIONS_PER_CALL} 个问题"),
        "EMPTY_QUESTION_TEXT" => "问题正文不能为空".to_owned(),
        "EXCEEDED_MAX_QUESTION_CHARS" => {
            format!("单个问题正文最多 {MAX_QUESTION_CHARS} 个字符")
        }
        "EXCEEDED_MAX_OPTIONS" => format!("单个问题最多 {MAX_OPTIONS_PER_QUESTION} 个选项"),
        "EXCEEDED_MAX_DETAIL_CHARS" => "补充说明过长，请精简后重试".to_owned(),
        "EXCEEDED_MAX_PAYLOAD_BYTES" => "提问内容总长超出上限，请精简后重试".to_owned(),
        "EMPTY_SUMMARY" => "摘要不能为空：用一句话说明你在问什么".to_owned(),
        "EXCEEDED_MAX_SUMMARY_CHARS" => {
            format!("摘要超出 {ASK_USER_QUESTION_MAX_SUMMARY_CHARS} 字，请精简")
        }
        other => format!("提问参数不合法: {other}"),
    }
}

/// ask_user_question 输入结构。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AskUserQuestionToolInput {
    /// 折叠展示的一句话摘要；用户收起这条调用时只看得到它。
    pub summary: String,
    /// 向用户提出的结构化问题列表。
    pub questions: Vec<UserQuestionItem>,
    /// 预算耗尽挂起时的交接提示文本。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff: Option<String>,
}

/// ask_user_question 工具执行回执。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AskUserQuestionToolReceipt {
    /// 回执 schema 版本号。
    pub schema_version: u32,
    /// 回执类型。
    pub kind: String,
    /// 状态（answered / suspended / aborted）。
    pub status: String,
    /// 是否挂起中。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<bool>,
    /// 是否总预算耗尽挂起。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suspended: Option<bool>,
    /// 工具调用 ID。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    /// 提示通知文本。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notice: Option<String>,
    /// 用户回答条目列表。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answers: Option<Vec<UserAnswerItem>>,
}

/// 校验输入并返回输入对象。
pub fn parse_and_validate_input(value: &Value) -> AxonResult<AskUserQuestionToolInput> {
    let raw_str = serde_json::to_string(value).unwrap_or_default();
    let input: AskUserQuestionToolInput = serde_json::from_value(value.clone())
        .map_err(|e| AxonError::tool_invalid_arguments(format!("提问参数无法解析: {e}")))?;

    validate_question_payload(&input.questions, raw_str.len()).map_err(|err_code| {
        AxonError::tool_invalid_arguments(validation_error_message(err_code))
    })?;

    // 摘要是折叠行里唯一可见的文本：空摘要会让用户收起后什么都看不到。
    let summary = input.summary.trim();
    if summary.is_empty() {
        return Err(AxonError::tool_invalid_arguments(validation_error_message(
            "EMPTY_SUMMARY",
        )));
    }
    if summary.chars().count() > ASK_USER_QUESTION_MAX_SUMMARY_CHARS {
        return Err(AxonError::tool_invalid_arguments(validation_error_message(
            "EXCEEDED_MAX_SUMMARY_CHARS",
        )));
    }

    Ok(input)
}

/// 构造 ask_user_question 工具定义。
#[must_use]
pub fn ask_user_question_tool_definition() -> ToolDefinition {
    assert!(
        ASK_USER_QUESTION_DESCRIPTION.chars().count() <= ASK_USER_QUESTION_MAX_DESCRIPTION_CHARS,
        "工具描述超出最大字符上限"
    );

    ToolDefinition {
        id: ASK_USER_QUESTION_TOOL_ID,
        namespace: crate::AGENT_TOOL_NAMESPACE.to_owned(),
        name: ASK_USER_QUESTION_TOOL_NAME.to_owned(),
        description: ASK_USER_QUESTION_DESCRIPTION.to_owned(),
        schema: ToolSchema::new(
            ASK_USER_QUESTION_TOOL_SCHEMA_VERSION,
            ask_user_question_schema(),
        ),
        enabled: true,
        concurrency_policy: axon_domain::ToolConcurrencyPolicy::Exclusive,
        output_policy: ToolOutputPolicy::BoundedSummary,
        permission_policy: ToolPermissionPolicy::ReadOnly,
    }
}

/// 生成 ask_user_question 的 JSON Schema（同源引用常量）。
#[must_use]
pub fn ask_user_question_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "summary": {
                "type": "string",
                "description": "折叠展示的一句话摘要：用户收起这条调用时只看得到它，因此要说清你在问什么",
                "minLength": 1,
                "maxLength": ASK_USER_QUESTION_MAX_SUMMARY_CHARS
            },
            "questions": {
                "type": "array",
                "description": "向用户提出的问题列表",
                "maxItems": MAX_QUESTIONS_PER_CALL,
                "items": {
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "string",
                            "description": "问题唯一标识"
                        },
                        "question": {
                            "type": "string",
                            "description": "问题内容",
                            "maxLength": MAX_QUESTION_CHARS
                        },
                        "header": {
                            "type": "string",
                            "description": "简短分类或标题"
                        },
                        "detail": {
                            "type": "string",
                            "description": "补充说明细节",
                            "maxLength": MAX_DETAIL_CHARS
                        },
                        "options": {
                            "type": "array",
                            "description": "候选选项列表",
                            "maxItems": MAX_OPTIONS_PER_QUESTION,
                            "items": {
                                "type": "object",
                                "properties": {
                                    "label": {
                                        "type": "string",
                                        "description": "选项标签"
                                    },
                                    "description": {
                                        "type": "string",
                                        "description": "选项说明"
                                    }
                                },
                                "required": ["label"],
                                "additionalProperties": false
                            }
                        },
                        "multi_select": {
                            "type": "boolean",
                            "description": "是否允许多选"
                        }
                    },
                    "required": ["id", "question"],
                    "additionalProperties": false
                }
            },
            "handoff": {
                "type": "string",
                "description": "用户没有及时回答时展示给用户的话：说明你在等什么，以及用户回答后你会接着做什么"
            }
        },
        "required": ["summary", "questions"],
        "additionalProperties": false
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_domain::ToolConcurrencyPolicy;

    #[test]
    fn test_ask_user_question_tool_definition_properties() {
        let def = ask_user_question_tool_definition();
        assert_eq!(def.id, ASK_USER_QUESTION_TOOL_ID);
        assert_eq!(def.name, ASK_USER_QUESTION_TOOL_NAME);
        assert_eq!(def.concurrency_policy, ToolConcurrencyPolicy::Exclusive);
        assert!(def.description.chars().count() <= ASK_USER_QUESTION_MAX_DESCRIPTION_CHARS);
    }

    #[test]
    fn test_parse_and_validate_rejects_empty_or_oversized() {
        // 空问题
        let empty_input = json!({
            "summary": "确认部署分支",
            "questions": []
        });
        assert!(parse_and_validate_input(&empty_input).is_err());

        // 超过 8 个问题
        let mut questions = Vec::new();
        for i in 0..9 {
            questions.push(json!({
                "id": format!("q{i}"),
                "question": "测试问题"
            }));
        }
        let nine_questions = json!({
            "summary": "确认部署分支",
            "questions": questions
        });
        assert!(parse_and_validate_input(&nine_questions).is_err());

        // 超过 20 个选项
        let mut options = Vec::new();
        for i in 0..21 {
            options.push(json!({ "label": format!("opt{i}") }));
        }
        let too_many_options = json!({
            "summary": "确认部署分支",
            "questions": [{
                "id": "q1",
                "question": "测试问题",
                "options": options
            }]
        });
        assert!(parse_and_validate_input(&too_many_options).is_err());

        // 缺摘要：schema 必填，缺失必须在解析阶段就被拦下
        let missing_summary = json!({
            "questions": [{ "id": "q1", "question": "确认继续？" }]
        });
        assert!(parse_and_validate_input(&missing_summary).is_err());

        // 摘要为空或只有空白
        for blank in ["", "   "] {
            let blank_summary = json!({
                "summary": blank,
                "questions": [{ "id": "q1", "question": "确认继续？" }]
            });
            let error = parse_and_validate_input(&blank_summary).expect_err("空摘要必须被拒");
            let rendered = format!("{error:?}");
            assert!(
                rendered.contains("摘要不能为空"),
                "空摘要要给出可据以改正的说明: {rendered}"
            );
        }

        // 摘要超长
        let oversized_summary = json!({
            "summary": "摘".repeat(ASK_USER_QUESTION_MAX_SUMMARY_CHARS + 1),
            "questions": [{ "id": "q1", "question": "确认继续？" }]
        });
        assert!(parse_and_validate_input(&oversized_summary).is_err());

        // 合法输入
        let valid = json!({
            "summary": "确认部署分支",
            "questions": [{
                "id": "q1",
                "question": "确认继续？",
                "options": [{ "label": "是" }, { "label": "否" }]
            }],
            "handoff": "等待确认部署分支"
        });
        let parsed = parse_and_validate_input(&valid).expect("合法输入应成功解析");
        assert_eq!(parsed.summary, "确认部署分支");
        assert_eq!(parsed.questions.len(), 1);
        assert_eq!(parsed.handoff.as_deref(), Some("等待确认部署分支"));
    }

    #[test]
    fn schema_requires_the_folded_summary() {
        let schema = ask_user_question_schema();
        let required = schema["required"]
            .as_array()
            .expect("schema 必须给出 required");
        assert!(
            required.iter().any(|value| value == "summary"),
            "摘要必须在 schema 的必填项里: {required:?}"
        );
        assert_eq!(
            schema["properties"]["summary"]["maxLength"].as_u64(),
            Some(ASK_USER_QUESTION_MAX_SUMMARY_CHARS as u64)
        );
    }

    /// schema 按全仓口径声明 `multi_select`，而结构体序列化是 `multiSelect`：
    /// 两种拼写都必须进入解析，模型按 schema 传值不得被静默丢弃成单选。
    #[test]
    fn multi_select_accepts_the_schema_spelling_and_the_serialized_spelling() {
        let payload_with = |field: &str| {
            json!({
                "summary": "选择部署窗口",
                "questions": [{
                    "id": "q1",
                    "question": "部署到哪个窗口？",
                    (field): true,
                    "options": [{ "label": "今晚" }, { "label": "明天" }]
                }]
            })
        };

        for field in ["multi_select", "multiSelect"] {
            let parsed = parse_and_validate_input(&payload_with(field))
                .unwrap_or_else(|error| panic!("{field} 必须可解析: {error:?}"));
            assert!(
                parsed.questions[0].multi_select,
                "{field} 必须真的打开多选语义"
            );
        }

        // schema 声明的名字必须与字段描述一起保留在 schema 里，供模型按名传值。
        let schema = ask_user_question_schema();
        assert!(
            schema["properties"]["questions"]["items"]["properties"]
                .get("multi_select")
                .is_some(),
            "schema 必须继续声明 multi_select"
        );
    }

    #[test]
    fn test_receipt_shapes() {
        let answered_receipt = AskUserQuestionToolReceipt {
            schema_version: ASK_USER_QUESTION_TOOL_SCHEMA_VERSION,
            kind: ASK_USER_QUESTION_RECEIPT_KIND.to_string(),
            status: "answered".to_string(),
            pending: None,
            suspended: None,
            call_id: Some("call-1".to_string()),
            notice: None,
            answers: Some(vec![UserAnswerItem {
                id: "q1".to_string(),
                selected: vec!["是".to_string()],
                custom: None,
            }]),
        };
        let json = serde_json::to_string(&answered_receipt).expect("序列化");
        assert!(json.contains(r#""status":"answered""#));
        assert!(json.contains(r#""answers""#));

        let suspended_receipt = AskUserQuestionToolReceipt {
            schema_version: ASK_USER_QUESTION_TOOL_SCHEMA_VERSION,
            kind: ASK_USER_QUESTION_RECEIPT_KIND.to_string(),
            status: "suspended".to_string(),
            pending: Some(true),
            suspended: Some(true),
            call_id: Some("call-2".to_string()),
            notice: Some("任务已挂起".to_string()),
            answers: None,
        };
        let susp_json = serde_json::to_string(&suspended_receipt).expect("序列化");
        assert!(susp_json.contains(r#""pending":true"#));
        assert!(susp_json.contains(r#""suspended":true"#));
    }
}
