//! invoke_skill meta tool.

use std::path::Path;

use axon_domain::{
    AxonError, AxonResult, ToolDefinition, ToolId, ToolInput, ToolOutput, ToolOutputPolicy,
    ToolPermissionPolicy, ToolSchema,
};

use crate::skills::{SkillsManager, invocation::render_skill_invocation};

/// invoke_skill 工具名称。
pub const INVOKE_SKILL_TOOL_NAME: &str = "invoke_skill";
/// invoke_skill 工具命名空间。
pub const INVOKE_SKILL_TOOL_NAMESPACE: &str = "builtin";
/// invoke_skill 工具 ID。
pub const INVOKE_SKILL_TOOL_ID: ToolId = ToolId(20);
/// invoke_skill 工具 schema 版本。
pub const INVOKE_SKILL_TOOL_SCHEMA_VERSION: u32 = 3;

/// invoke_skill 工具定义。
///
/// 通过单一元工具入口执行预定义的skill workflows，避免将大量skill直接注册到tool schema。
#[must_use]
pub fn invoke_skill_tool_definition() -> ToolDefinition {
    let schema = ToolSchema::new(
        INVOKE_SKILL_TOOL_SCHEMA_VERSION,
        serde_json::json!({
            "type": "object",
            "properties": {
                "skill_name": {
                    "type": "string",
                    "pattern": "\\S",
                    "description": "要加载说明的已注入 Skill 名称；例如 feishu-cli。"
                }
            },
            "required": ["skill_name"],
            "additionalProperties": false
        }),
    );
    ToolDefinition {
        id: INVOKE_SKILL_TOOL_ID,
        namespace: INVOKE_SKILL_TOOL_NAMESPACE.into(),
        name: INVOKE_SKILL_TOOL_NAME.into(),
        description: "加载一个已注入 Skill 的操作说明，不直接执行用户任务；只能提供 skill_name，加载后按返回说明调用实际工具。".into(),
        schema,
        enabled: true,
        concurrency_policy: axon_domain::ToolConcurrencyPolicy::ParallelSafe,
        output_policy: ToolOutputPolicy::RefOnOverflow,
        permission_policy: ToolPermissionPolicy::ReadOnly,
    }
}

/// invoke_skill 工具输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvokeSkillInput {
    /// Skill 名称。
    pub skill_name: String,
}

/// 从 ToolInput 解析 invoke_skill 参数。
pub fn parse_invoke_skill_input(input: &ToolInput) -> AxonResult<InvokeSkillInput> {
    if input.fields.is_empty() {
        return Err(AxonError::tool_invalid_arguments("missing skill_name"));
    }
    if input.fields.len() > 1 {
        return Err(AxonError::tool_invalid_arguments(
            "invoke_skill 参数包含未知字段",
        ));
    }

    let skill_name = input.fields[0].clone();
    if skill_name.trim().is_empty() {
        return Err(AxonError::tool_invalid_arguments(
            "invoke_skill skill_name 不能为空",
        ));
    }
    Ok(InvokeSkillInput { skill_name })
}

/// invoke_skill 执行器。
pub fn execute_invoke_skill(input: &InvokeSkillInput) -> AxonResult<ToolOutput> {
    execute_invoke_skill_with_manager(input, &SkillsManager::from_runtime(None))
}

/// Execute `invoke_skill` with runtime-provided skill roots.
pub fn execute_invoke_skill_with_manager(
    input: &InvokeSkillInput,
    manager: &SkillsManager,
) -> AxonResult<ToolOutput> {
    let skill = manager
        .find(&input.skill_name)
        .ok_or(AxonError::tool_invalid_definition(
            "invoke_skill: skill not found",
        ))?;
    let text = render_skill_invocation(&skill, None).map_err(|message| {
        if message.contains("disables model invocation") {
            AxonError::tool_invalid_definition("invoke_skill: skill disables model invocation")
        } else {
            AxonError::tool_invalid_definition("invoke_skill: render failed")
        }
    })?;
    Ok(ToolOutput { text })
}

/// Execute `invoke_skill` using environment roots plus an explicit cwd.
pub fn execute_invoke_skill_with_cwd(
    input: &InvokeSkillInput,
    cwd: Option<&Path>,
) -> AxonResult<ToolOutput> {
    execute_invoke_skill_with_manager(input, &SkillsManager::from_runtime(cwd))
}

#[cfg(test)]
mod tests {

    use super::*;

    fn official_manager(document: &str) -> (axon_test_support::TestTempDir, SkillsManager) {
        let directory = axon_test_support::test_temp_dir();
        let skill_dir = directory.path().join("test-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), document).unwrap();
        let manager = SkillsManager::with_roots(vec![crate::skills::SkillRoot::new(
            directory.path(),
            crate::skills::SkillScope::User,
            1,
        )]);
        (directory, manager)
    }

    #[test]
    fn parse_invoke_skill_input_requires_exact_single_name() {
        let parsed = parse_invoke_skill_input(&ToolInput {
            fields: vec!["test-skill".to_owned()],
        })
        .unwrap();
        assert_eq!(parsed.skill_name, "test-skill");
        assert!(parse_invoke_skill_input(&ToolInput { fields: vec![] }).is_err());
        assert!(
            parse_invoke_skill_input(&ToolInput {
                fields: vec![" ".to_owned()],
            })
            .is_err()
        );
        assert!(
            parse_invoke_skill_input(&ToolInput {
                fields: vec!["test-skill".to_owned(), "extra".to_owned()],
            })
            .is_err()
        );
    }

    #[test]
    fn execute_invoke_skill_loads_official_instructions() {
        let (_directory, manager) = official_manager(
            "---\nname: test-skill\ndescription: Test skill\n---\nDo the test skill.",
        );
        let result = execute_invoke_skill_with_manager(
            &InvokeSkillInput {
                skill_name: "test-skill".to_owned(),
            },
            &manager,
        )
        .unwrap();

        assert!(result.text.contains("description=Test skill"));
        assert!(
            result
                .text
                .contains("status=instructions_loaded_task_not_executed")
        );
        assert!(result.text.contains("Do the test skill."));
    }

    #[test]
    fn execute_invoke_skill_accepts_additional_frontmatter_and_rejects_aliases() {
        let (_directory, manager) = official_manager(
            "---\nname: test-skill\ndescription: Test skill\nmodel: fast\n---\nBody",
        );
        assert!(
            execute_invoke_skill_with_manager(
                &InvokeSkillInput {
                    skill_name: "test-skill".to_owned(),
                },
                &manager,
            )
            .is_ok()
        );

        let (_directory, manager) =
            official_manager("---\nname: test-skill\ndescription: Test skill\n---\nBody");
        assert!(
            execute_invoke_skill_with_manager(
                &InvokeSkillInput {
                    skill_name: "TEST-SKILL".to_owned(),
                },
                &manager,
            )
            .is_err()
        );
    }
}
