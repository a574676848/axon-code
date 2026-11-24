//! 内置 update_plan tool 的完整快照与验证证据契约。

use axon_domain::{
    AxonError, AxonResult, ToolDefinition, ToolId, ToolOutputPolicy, ToolPermissionPolicy,
    ToolSchema,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// update_plan tool 名称。
pub const UPDATE_PLAN_TOOL_NAME: &str = "update_plan";
/// update_plan tool 完整名称。
pub const UPDATE_PLAN_TOOL_QUALIFIED_NAME: &str = "builtin.update_plan";
/// update_plan tool schema version。
pub const UPDATE_PLAN_TOOL_SCHEMA_VERSION: u32 = 2;
/// update_plan tool ID。
pub const UPDATE_PLAN_TOOL_ID: ToolId = ToolId(23);
/// 复杂任务计划最少步骤数。
pub const UPDATE_PLAN_MIN_STEPS: usize = 3;
/// 复杂任务计划最多步骤数。
pub const UPDATE_PLAN_MAX_STEPS: usize = 7;
/// plan receipt 类型标识。
pub const UPDATE_PLAN_RECEIPT_KIND: &str = UPDATE_PLAN_TOOL_NAME;
/// 默认计划标题。
pub const UPDATE_PLAN_TITLE: &str = "计划";

const PLAN_SIZE_ERROR: &str = "复杂任务计划必须包含 3 到 7 个步骤";
const PLAN_STEP_EMPTY_ERROR: &str = "计划步骤不能为空";
const PLAN_EXPLANATION_EMPTY_ERROR: &str = "计划更新说明不能为空";
const PLAN_MULTIPLE_ACTIVE_ERROR: &str = "计划同时最多只能有一个 in_progress 步骤";
const PLAN_COMPLETED_WITHOUT_VERIFICATION_ERROR: &str =
    "completed 步骤必须提供非空 verification 验证证据";

/// 计划步骤状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdatePlanStepStatus {
    /// 待处理。
    Pending,
    /// 正在处理。
    InProgress,
    /// 已完成且有验证证据。
    Completed,
}

impl UpdatePlanStepStatus {
    /// 返回 wire 使用的稳定状态名称。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
        }
    }
}

/// 计划中的一个可验证结果步骤。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdatePlanStep {
    /// 可验证的结果描述。
    pub step: String,
    /// 当前状态。
    pub status: UpdatePlanStepStatus,
    /// 完成步骤的验证证据。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<String>,
}

/// update_plan tool 输入。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdatePlanToolInput {
    /// 模型自述的本次更新目的；只用于界面折叠态展示，不参与计划语义。
    pub intent: String,
    /// 本次更新原因或范围变化。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explanation: Option<String>,
    /// 完整计划快照。
    pub plan: Vec<UpdatePlanStep>,
}

/// update_plan tool 执行回执。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdatePlanToolExecutionReceipt {
    /// receipt schema version。
    pub schema_version: u32,
    /// receipt 类型。
    pub kind: String,
    /// 计划标题。
    pub title: String,
    /// 本次更新说明。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explanation: Option<String>,
    /// 完整计划快照。
    pub plan: Vec<UpdatePlanStep>,
    /// 已完成步骤数。
    pub completed_steps: usize,
    /// 总步骤数。
    pub total_steps: usize,
}

/// 无状态计划校验 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct UpdatePlanToolRuntime;

impl UpdatePlanToolRuntime {
    /// 解析并校验完整计划快照。
    pub fn execute(&self, value: &Value) -> AxonResult<UpdatePlanToolExecutionReceipt> {
        let input = serde_json::from_value::<UpdatePlanToolInput>(value.clone())
            .map_err(|error| AxonError::tool_invalid_arguments(error.to_string()))?;
        validate_input(&input)?;
        let completed_steps = input
            .plan
            .iter()
            .filter(|step| step.status == UpdatePlanStepStatus::Completed)
            .count();
        let total_steps = input.plan.len();
        Ok(UpdatePlanToolExecutionReceipt {
            schema_version: UPDATE_PLAN_TOOL_SCHEMA_VERSION,
            kind: UPDATE_PLAN_RECEIPT_KIND.to_owned(),
            title: UPDATE_PLAN_TITLE.to_owned(),
            explanation: input.explanation,
            plan: input.plan,
            completed_steps,
            total_steps,
        })
    }
}

/// 创建 update_plan tool 定义。
#[must_use]
pub fn update_plan_tool_definition() -> ToolDefinition {
    ToolDefinition {
        id: UPDATE_PLAN_TOOL_ID,
        namespace: crate::AGENT_TOOL_NAMESPACE.to_owned(),
        name: UPDATE_PLAN_TOOL_NAME.to_owned(),
        description:
            "复杂任务开始和阶段切换时更新完整计划；步骤必须是可验证结果，完成项必须附验证证据。intent 是本次更新必填的一句话目的，写法见该字段描述。"
                .to_owned(),
        schema: ToolSchema::new(UPDATE_PLAN_TOOL_SCHEMA_VERSION, update_plan_schema()),
        enabled: true,
        concurrency_policy: axon_domain::ToolConcurrencyPolicy::Exclusive,
        output_policy: ToolOutputPolicy::BoundedSummary,
        permission_policy: ToolPermissionPolicy::ReadOnly,
    }
}

fn validate_input(input: &UpdatePlanToolInput) -> AxonResult<()> {
    if !(UPDATE_PLAN_MIN_STEPS..=UPDATE_PLAN_MAX_STEPS).contains(&input.plan.len()) {
        return Err(AxonError::tool_invalid_arguments(PLAN_SIZE_ERROR));
    }
    if input
        .explanation
        .as_deref()
        .is_some_and(|value| value.trim().is_empty())
    {
        return Err(AxonError::tool_invalid_arguments(
            PLAN_EXPLANATION_EMPTY_ERROR,
        ));
    }
    if input.plan.iter().any(|step| step.step.trim().is_empty()) {
        return Err(AxonError::tool_invalid_arguments(PLAN_STEP_EMPTY_ERROR));
    }
    if input
        .plan
        .iter()
        .filter(|step| step.status == UpdatePlanStepStatus::InProgress)
        .count()
        > 1
    {
        return Err(AxonError::tool_invalid_arguments(
            PLAN_MULTIPLE_ACTIVE_ERROR,
        ));
    }
    if input.plan.iter().any(|step| {
        step.status == UpdatePlanStepStatus::Completed
            && step
                .verification
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
    }) {
        return Err(AxonError::tool_invalid_arguments(
            PLAN_COMPLETED_WITHOUT_VERIFICATION_ERROR,
        ));
    }
    Ok(())
}

fn update_plan_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "intent": {
                "type": "string",
                "pattern": "\\S",
                "description": crate::TOOL_INTENT_FIELD_DESCRIPTION
            },
            "explanation": {
                "type": "string",
                "pattern": "\\S",
                "description": "可选的更新原因、范围变化或验证失败说明。"
            },
            "plan": {
                "type": "array",
                "minItems": UPDATE_PLAN_MIN_STEPS,
                "maxItems": UPDATE_PLAN_MAX_STEPS,
                "description": "3 到 7 个可验证结果组成的完整计划快照。",
                "items": {
                    "type": "object",
                    "properties": {
                        "step": {
                            "type": "string",
                            "pattern": "\\S",
                            "description": "可独立验收的结果描述，不写机械操作。"
                        },
                        "status": {
                            "type": "string",
                            "enum": ["pending", "in_progress", "completed"],
                            "description": "步骤状态；同时最多一个 in_progress。"
                        },
                        "verification": {
                            "type": "string",
                            "pattern": "\\S",
                            "description": "completed 步骤必填的测试、检查或工具证据。"
                        }
                    },
                    "required": ["step", "status"],
                    "additionalProperties": false,
                    "allOf": [{
                        "if": {
                            "properties": {"status": {"const": "completed"}},
                            "required": ["status"]
                        },
                        "then": {"required": ["verification"]}
                    }]
                }
            }
        },
        "required": ["intent", "plan"],
        "additionalProperties": false
    })
}

#[cfg(test)]
mod tests {
    use axon_domain::ToolCallInput;

    use super::*;

    #[test]
    fn accepts_single_active_step_and_completed_verification() {
        let receipt = UpdatePlanToolRuntime
            .execute(&json!({
                "intent": "进入实现阶段",
                "explanation": "进入实现阶段",
                "plan": [
                    {"step": "调用链证据闭环", "status": "completed", "verification": "图谱覆盖检查通过"},
                    {"step": "普通 tool loop 可更新计划", "status": "in_progress"},
                    {"step": "质量门禁通过", "status": "pending"}
                ]
            }))
            .expect("合法计划应执行成功");

        assert_eq!(receipt.completed_steps, 1);
        assert_eq!(receipt.total_steps, 3);
        assert_eq!(receipt.kind, UPDATE_PLAN_RECEIPT_KIND);
    }

    #[test]
    fn rejects_multiple_active_steps() {
        let error = UpdatePlanToolRuntime
            .execute(&json!({
                "intent": "推进两个并行步骤",
                "plan": [
                    {"step": "步骤一", "status": "in_progress"},
                    {"step": "步骤二", "status": "in_progress"},
                    {"step": "步骤三", "status": "pending"}
                ]
            }))
            .expect_err("多个活跃步骤必须被拒绝");

        assert!(format!("{error:?}").contains(PLAN_MULTIPLE_ACTIVE_ERROR));
    }

    #[test]
    fn rejects_completed_step_without_verification() {
        let error = UpdatePlanToolRuntime
            .execute(&json!({
                "intent": "提交阶段快照",
                "plan": [
                    {"step": "步骤一", "status": "completed"},
                    {"step": "步骤二", "status": "pending"},
                    {"step": "步骤三", "status": "pending"}
                ]
            }))
            .expect_err("缺少验证证据的完成步骤必须被拒绝");

        assert!(format!("{error:?}").contains(PLAN_COMPLETED_WITHOUT_VERIFICATION_ERROR));
    }

    #[test]
    fn schema_requires_completed_verification() {
        let mut registry = crate::ToolRegistry::new();
        registry
            .register(update_plan_tool_definition())
            .expect("update_plan 应注册成功");

        assert!(
            registry
                .validate_call_input(
                    UPDATE_PLAN_TOOL_QUALIFIED_NAME,
                    &ToolCallInput::json(json!({
                        "intent": "切换到实现阶段",
                        "plan": [
                            {"step": "定位", "status": "completed"},
                            {"step": "实现", "status": "in_progress"},
                            {"step": "验证", "status": "pending"}
                        ]
                    }))
                )
                .is_err()
        );
        assert!(
            registry
                .validate_call_input(
                    UPDATE_PLAN_TOOL_QUALIFIED_NAME,
                    &ToolCallInput::json(json!({
                        "intent": "切换到实现阶段",
                        "plan": [
                            {"step": "定位", "status": "completed", "verification": "调用链已核验"},
                            {"step": "实现", "status": "in_progress"},
                            {"step": "验证", "status": "pending"}
                        ]
                    }))
                )
                .is_ok()
        );
    }

    #[test]
    fn schema_requires_intent_for_the_collapsed_row() {
        let mut registry = crate::ToolRegistry::new();
        registry
            .register(update_plan_tool_definition())
            .expect("update_plan 应注册成功");

        assert!(
            registry
                .validate_call_input(
                    UPDATE_PLAN_TOOL_QUALIFIED_NAME,
                    &ToolCallInput::json(json!({
                        "plan": [
                            {"step": "步骤一", "status": "pending"},
                            {"step": "步骤二", "status": "pending"},
                            {"step": "步骤三", "status": "pending"}
                        ]
                    }))
                )
                .is_err(),
            "缺 intent 时折叠态只能摆参数 JSON，因此 schema 必须拦下"
        );
        assert_eq!(
            crate::tool_call_intent(
                UPDATE_PLAN_TOOL_NAME,
                r#"{"intent":"首次提交计划","plan":[]}"#
            )
            .as_deref(),
            Some("首次提交计划")
        );
    }

    #[test]
    fn schema_allows_zero_active_steps_and_omits_provider_unsupported_contains() {
        let mut registry = crate::ToolRegistry::new();
        registry
            .register(update_plan_tool_definition())
            .expect("update_plan 应注册成功");

        assert!(
            registry
                .validate_call_input(
                    UPDATE_PLAN_TOOL_QUALIFIED_NAME,
                    &ToolCallInput::json(json!({
                        "intent": "切换到实现阶段",
                        "plan": [
                            {"step": "步骤一", "status": "pending"},
                            {"step": "步骤二", "status": "pending"},
                            {"step": "步骤三", "status": "pending"}
                        ]
                    }))
                )
                .is_ok()
        );

        // Provider 兼容层（Gemini/第三方网关）不支持 contains/minContains/maxContains，
        // 「最多一个 in_progress」由运行时校验保证。
        let plan_schema = &update_plan_schema()["properties"]["plan"];
        assert!(plan_schema.get("contains").is_none());
        assert!(plan_schema.get("minContains").is_none());
        assert!(plan_schema.get("maxContains").is_none());
    }
}
