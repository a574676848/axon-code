//! Axon 提示词工程：稳定 Agent 行为准则与动态投影块。

use axon_prompts::render_agent_system_prompt;

/// 提示词上下文块头。
pub const AXON_PROMPT_CONTEXT_HEADER: &str = "[Axon Prompt Context]";
/// 运行时约束块头。
pub const AXON_RUNTIME_CONSTRAINTS_HEADER: &str = "<runtime_constraints>";
/// 运行时约束块尾。
const AXON_RUNTIME_CONSTRAINTS_FOOTER: &str = "</runtime_constraints>";

#[derive(Debug, Clone, PartialEq, Eq, Default)]
/// 提示词工程运行时上下文。
pub struct AgentPromptContext {
    /// 当前选中的 skill。
    pub selected_skill_name: Option<String>,
    /// 当前命中的 workspace instruction 路径。
    pub workspace_instruction_paths: Vec<String>,
}

/// 提示词工程 runtime。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct AgentPromptEngineeringRuntime;

impl AgentPromptEngineeringRuntime {
    /// 生成稳定 system prompt。
    #[must_use]
    pub fn system_prompt(&self) -> String {
        render_agent_system_prompt()
    }

    /// 生成 prompt context block。
    #[must_use]
    pub fn prompt_context_block(&self, context: &AgentPromptContext) -> String {
        let selected_skill = context.selected_skill_name.as_deref().unwrap_or("none");
        let workspace_instructions = if context.workspace_instruction_paths.is_empty() {
            "none".to_owned()
        } else {
            context.workspace_instruction_paths.join(", ")
        };
        format!(
            "{AXON_PROMPT_CONTEXT_HEADER}\nselected_skill={selected_skill}\nworkspace_instructions={workspace_instructions}"
        )
    }

    /// 生成运行时约束 block。
    #[must_use]
    pub fn runtime_constraints_block(&self, runtime_constraints: Option<&str>) -> Option<String> {
        let runtime_constraints = runtime_constraints
            .map(str::trim)
            .filter(|value| !value.is_empty())?;
        if runtime_constraints.starts_with(AXON_RUNTIME_CONSTRAINTS_HEADER) {
            return Some(runtime_constraints.to_owned());
        }
        Some(format!(
            "{AXON_RUNTIME_CONSTRAINTS_HEADER}\n{runtime_constraints}\n{AXON_RUNTIME_CONSTRAINTS_FOOTER}"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_constraints_block_wraps_plain_text_in_xml_once() {
        let block = AgentPromptEngineeringRuntime
            .runtime_constraints_block(Some("surface=cli"))
            .expect("非空运行时约束应生成 XML block");

        assert_eq!(block.matches(AXON_RUNTIME_CONSTRAINTS_HEADER).count(), 1);
        assert_eq!(
            block,
            "<runtime_constraints>\nsurface=cli\n</runtime_constraints>"
        );
    }
}
