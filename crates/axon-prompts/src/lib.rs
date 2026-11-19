//! Axon prompt 模板渲染。

use std::borrow::Cow;

const AXON_WORKSPACE_INSTRUCTIONS_TEMPLATE: &str =
    include_str!("../templates/agent/workspace_policy.md");
const AXON_RUNTIME_CONSTRAINTS_TEMPLATE: &str =
    include_str!("../templates/agent/runtime_constraints.md");
const AXON_TOOL_POLICY_TEMPLATE: &str = include_str!("../templates/agent/tool_policy.md");
const AXON_CONTEXT_WINDOW_GUIDANCE_TEMPLATE: &str =
    include_str!("../templates/agent/context_window_guidance.md");
const AXON_TOOL_USAGE_GUIDANCE_TEMPLATE: &str =
    include_str!("../templates/agent/tool_usage_guidance.md");
const AXON_GOAL_EXECUTION_TEMPLATE: &str = include_str!("../templates/agent/goal_execution.md");
/// 主 turn 工具编排协议模板自带的标签。
///
/// 标签由模板正文承载（fragment markers 留空），运行时按它判定主 turn 稳定前缀
/// 是否装配了工具编排协议片段。
pub const AXON_TOOL_ORCHESTRATION_PROTOCOL_TAG: &str = "<tool_orchestration_protocol>";
#[cfg(test)]
const AXON_AGENT_SYSTEM_PROMPT_SNAPSHOT: &str =
    include_str!("../templates/agent/snapshots/agent_system_prompt.md");

/// Agent runtime constraints 模板输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRuntimeConstraintsTemplateInput {
    /// 入口 surface。
    pub surface: String,
    /// 选中的 skill 名称。
    pub selected_skill: String,
    /// 用户级 Home 的实际目录。
    pub home_root: String,
    /// 当前 Workspace 的实际工作目录。
    pub workspace_root: String,
    /// 命中的 workspace instruction 路径。
    pub workspace_instructions: String,
    /// 入口能力约束行。
    pub capability_lines: Vec<String>,
    /// Memory 工具建议。
    pub memory_tool_guidance: Option<String>,
    /// Memory 验证建议。
    pub memory_verify_candidate: Option<String>,
    /// Memory 引用协议。
    pub memory_citation_protocol: Option<String>,
}

/// 渲染 Axon agent system prompt。
///
/// 稳定 system prompt 只承载工作场所约束；工具与委托契约由紧随其后的稳定
/// developer 片段承载。
#[must_use]
pub fn render_agent_system_prompt() -> String {
    normalize_line_endings(AXON_WORKSPACE_INSTRUCTIONS_TEMPLATE.trim())
}

/// 渲染运行时约束 fragment。
#[must_use]
pub fn render_agent_runtime_constraints(input: &AgentRuntimeConstraintsTemplateInput) -> String {
    render_template(
        AXON_RUNTIME_CONSTRAINTS_TEMPLATE,
        &[
            ("surface", escape(&input.surface)),
            ("selected_skill", escape(&input.selected_skill)),
            ("home_root", escape(&input.home_root)),
            ("workspace_root", escape(&input.workspace_root)),
            (
                "workspace_instructions",
                escape(&input.workspace_instructions),
            ),
            (
                "capability_lines",
                optional_lines(input.capability_lines.as_slice()),
            ),
            (
                "memory_tool_guidance",
                optional_line(
                    "memory_tool_guidance",
                    input.memory_tool_guidance.as_deref(),
                ),
            ),
            (
                "memory_verify_candidate",
                optional_line(
                    "memory_verify_candidate",
                    input.memory_verify_candidate.as_deref(),
                ),
            ),
            (
                "memory_citation_protocol",
                optional_block(input.memory_citation_protocol.as_deref()),
            ),
        ],
    )
    .lines()
    .filter(|line| !line.trim().is_empty())
    .collect::<Vec<_>>()
    .join("\n")
}

/// 渲染主 turn 工具策略 developer fragment。
///
/// 该片段承载委托 fast 子代理与提交结构化解策的场景判据，只装配到主 turn 的
/// 稳定 developer 前缀；子代理身份不装配，避免子代理误用已屏蔽的工具。
#[must_use]
pub fn render_agent_tool_policy() -> String {
    normalize_line_endings(AXON_TOOL_POLICY_TEMPLATE.trim())
}

/// 渲染记忆窗口协议 developer fragment。
///
/// 该片段镜像 Codex 的 `context_window_guidance`，常驻主 turn 与子代理的稳定
/// developer 前缀，只描述模型私有便签与归档历史工具的使用方式。
#[must_use]
pub fn render_context_window_guidance() -> String {
    normalize_line_endings(AXON_CONTEXT_WINDOW_GUIDANCE_TEMPLATE.trim())
}

/// 渲染工具使用规约 developer fragment。
///
/// 该片段承载 edit 与 shell 的**跨调用**使用规约（格式选型、命令形态），
/// 常驻主 turn 与子代理的稳定 developer 前缀：这两类工具在两种身份下都可用，
/// 因此规约对两类身份同时有效；子代理不装配 `<tool_orchestration_protocol>`，若把规约留在
/// 工具 `description` 里，它只能随每次请求的工具定义重复下发。
#[must_use]
pub fn render_tool_usage_guidance() -> String {
    normalize_line_endings(AXON_TOOL_USAGE_GUIDANCE_TEMPLATE.trim())
}

/// Goal 执行期的写权限口径。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalExecutionScope {
    /// 允许改动文件。
    AllowWrites,
    /// 只读执行。
    ReadOnly,
}

/// Goal 执行模板输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalExecutionTemplateInput {
    /// 目标标题。
    pub title: String,
    /// 可验收目标。
    pub objective: String,
    /// 有序执行步骤。
    pub steps: Vec<String>,
    /// 验收条件。
    pub acceptance: Vec<String>,
    /// 执行期写权限。
    pub scope: GoalExecutionScope,
}

/// 渲染后台 Goal 执行指令。
///
/// 模板不对字段做 XML 转义：执行步骤与验收条件可能含代码片段，转义会把它们改坏；
/// 这里只做占位符替换。
#[must_use]
pub fn render_goal_execution(input: &GoalExecutionTemplateInput) -> String {
    render_template(
        AXON_GOAL_EXECUTION_TEMPLATE,
        &[
            (
                "execution_scope",
                goal_execution_scope_text(input.scope).to_owned(),
            ),
            ("title", input.title.clone()),
            ("objective", input.objective.clone()),
            ("steps", bullet_list(&input.steps)),
            ("acceptance", bullet_list(&input.acceptance)),
        ],
    )
}

/// 写权限对应的执行边界说明。
///
/// 目标执行是无人值守的后台 turn，提示词是执行开始前唯一能表达「能不能动文件」的位置：
/// 只读档必须明说不得改文件，允许档必须限定改动范围，避免顺手改无关代码。
fn goal_execution_scope_text(scope: GoalExecutionScope) -> &'static str {
    match scope {
        GoalExecutionScope::ReadOnly => {
            "本次执行为只读：不得修改、创建或删除仓库内任何文件，也不得执行有副作用的命令。需要改动时，\
在结论里写清应改什么、由谁执行，不要自己动手。"
        }
        GoalExecutionScope::AllowWrites => {
            "只改目标与执行步骤能追溯到的那部分：不要顺手重构、清理或格式化无关文件；不要提交、推送或删除\
仓库内文件，除非目标明确要求。"
        }
    }
}

fn bullet_list(items: &[String]) -> String {
    items
        .iter()
        .map(|item| format!("- {item}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_template(template: &str, values: &[(&str, String)]) -> String {
    let mut rendered = template.to_owned();
    for (key, value) in values {
        rendered = rendered.replace(&format!("{{{{ {key} }}}}"), value);
    }
    rendered
}

fn normalize_line_endings(value: &str) -> String {
    value.replace("\r\n", "\n")
}

fn escape(value: &str) -> String {
    escape_xml_text(value)
}

fn escape_xml_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Agent 模板类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentTemplateKind {
    /// Workspace instructions 策略模板。
    WorkspaceInstructions,
    /// 运行时约束模板。
    RuntimeConstraints,
    /// 主 turn 工具策略模板。
    ToolPolicy,
    /// 记忆窗口协议模板。
    ContextWindowGuidance,
    /// 工具使用规约模板。
    ToolUsageGuidance,
    /// 后台 Goal 执行指令模板。
    GoalExecution,
}

/// 返回 Agent 模板源码，供审计和 snapshot 测试使用。
#[must_use]
pub fn agent_template_source(kind: AgentTemplateKind) -> Cow<'static, str> {
    Cow::Borrowed(match kind {
        AgentTemplateKind::WorkspaceInstructions => AXON_WORKSPACE_INSTRUCTIONS_TEMPLATE,
        AgentTemplateKind::RuntimeConstraints => AXON_RUNTIME_CONSTRAINTS_TEMPLATE,
        AgentTemplateKind::ToolPolicy => AXON_TOOL_POLICY_TEMPLATE,
        AgentTemplateKind::ContextWindowGuidance => AXON_CONTEXT_WINDOW_GUIDANCE_TEMPLATE,
        AgentTemplateKind::ToolUsageGuidance => AXON_TOOL_USAGE_GUIDANCE_TEMPLATE,
        AgentTemplateKind::GoalExecution => AXON_GOAL_EXECUTION_TEMPLATE,
    })
}

fn optional_line(key: &str, value: Option<&str>) -> String {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map_or_else(String::new, |value| format!("{key}={}", escape(value)))
}

fn optional_lines(values: &[String]) -> String {
    values
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(escape)
        .collect::<Vec<_>>()
        .join("\n")
}

fn optional_block(value: Option<&str>) -> String {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map_or_else(String::new, escape)
}

#[cfg(test)]
#[cfg(test)]
#[path = "tests.rs"]
mod tests;
