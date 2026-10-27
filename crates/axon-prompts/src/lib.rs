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
mod tests {
    use super::*;

    #[test]
    fn agent_system_prompt_is_composed_from_agent_templates() {
        let prompt = render_agent_system_prompt();

        assert!(!prompt.contains("<identity>"));
        assert!(!prompt.contains("JARVIS"));
        assert!(prompt.contains("<workspace_instructions>"));
        assert!(!prompt.contains("<tool_orchestration_protocol>"));
        assert!(!prompt.contains("<lsp_policy>"));
        for removed_marker in [
            "<behavior_guidelines>",
            "<coding_policy>",
            "<instruction_authority>",
            "<task_semantics>",
            "<interrupt_policy>",
            "<work_protocol>",
            "<context_policy>",
            "<output_policy>",
            "<memory_policy>",
            "<core_policy>",
            "<prohibitions>",
        ] {
            assert!(
                !prompt.contains(removed_marker),
                "稳定提示词不应保留旧策略标签：{removed_marker}"
            );
        }
    }

    #[test]
    fn agent_template_package_exposes_all_auditable_sources_in_order() {
        let kinds = [
            AgentTemplateKind::WorkspaceInstructions,
            AgentTemplateKind::RuntimeConstraints,
            AgentTemplateKind::ToolPolicy,
            AgentTemplateKind::ContextWindowGuidance,
        ];
        for kind in kinds {
            assert!(
                !agent_template_source(kind).trim().is_empty(),
                "agent template source should be non-empty: {kind:?}"
            );
        }

        let prompt = render_agent_system_prompt();
        let ordered_markers = ["<workspace_instructions>"];
        let mut cursor = 0;
        for marker in ordered_markers {
            let relative = prompt[cursor..]
                .find(marker)
                .unwrap_or_else(|| panic!("agent prompt should contain marker {marker}"));
            cursor += relative + marker.len();
        }
    }

    #[test]
    fn agent_system_prompt_matches_snapshot() {
        let prompt = render_agent_system_prompt();

        assert_eq!(
            prompt,
            normalize_line_endings(AXON_AGENT_SYSTEM_PROMPT_SNAPSHOT.trim())
        );
    }

    #[test]
    fn stable_system_prompts_only_describe_agent_visible_contracts() {
        let agent = render_agent_system_prompt();
        for marker in [
            "Runtime-injected",
            "current runtime working directory",
            "运行时真实暴露",
            "动态上下文必须以独立 fragment",
            "provider 暴露",
            "MCP schema 可以延迟暴露",
            "JSON-RPC",
            "didOpen",
            "client pool",
            "后台队列",
        ] {
            assert!(
                !agent.contains(marker),
                "稳定 agent prompt 泄漏宿主实现：{marker}"
            );
        }
    }

    #[test]
    fn stable_prompt_defines_tool_and_workplace_boundaries() {
        let prompt = render_agent_system_prompt();

        for system_only_marker in [
            "builtin.delegate_to_fast",
            "builtin.update_plan",
            "<tool_orchestration_protocol>",
            "<memory_window_protocol>",
        ] {
            assert!(
                !prompt.contains(system_only_marker),
                "主 turn 工具策略不得进入稳定 system prompt：{system_only_marker}"
            );
        }

        let tool_policy = render_agent_tool_policy();
        let tool_usage_guidance = render_tool_usage_guidance();
        assert!(tool_policy.contains(AXON_TOOL_ORCHESTRATION_PROTOCOL_TAG));
        assert!(
            tool_policy
                .contains("当前会话内复杂、跨模块、多阶段或高风险的实施类任务（3 到 7 个步骤）"),
            "计划判据必须写明适用场景与步数上限"
        );
        assert!(tool_policy.contains("简单单步或纯问答任务严禁创建"));
        assert!(tool_policy.contains("仅描述可验证的「验收成果」"));
        assert!(tool_policy.contains("严禁拆解为“读文件”、“写代码”等原子操作"));
        assert!(tool_policy.contains("末尾必须包含明确的最终验证与回退兜底路径"));
        assert!(
            tool_policy.contains("每次提交完整计划快照，全局仅允许存在至多一个 `in_progress`"),
            "计划状态机必须限定单 in_progress"
        );
        assert!(tool_policy.contains("多个互不依赖的 sidecar 应在同一条消息里一次发出"));
        assert!(tool_policy.contains("写与编辑一律独占"));
        assert!(
            tool_policy.contains("仅当下一步动作依赖该结果时用 `run_in_background: false`"),
            "委托判据必须写明同步等待的唯一条件"
        );
        assert!(tool_policy.contains("严禁轮询空转"));
        assert!(tool_policy.contains("builtin.goal_*"));
        assert!(tool_policy.contains("builtin.goal_create"));
        assert!(tool_policy.contains("builtin.goal_list"));
        assert!(tool_policy.contains("builtin.goal_get"));
        assert!(tool_policy.contains("builtin.goal_update"));
        assert!(tool_policy.contains("builtin.goal_cancel"));
        assert!(tool_policy.contains("写清目标、步骤与验收标准"));
        assert!(tool_policy.contains("builtin.goal_start"));
        assert!(tool_policy.contains("翻页沿用上一页 `cursor`"));
        assert!(tool_policy.contains("使用最近查询返回的 `expectedRevision`"));
        assert!(
            tool_policy.contains("本轮可完成的即时工作使用当前 turn"),
            "Goal 创建判据必须写明，否则模型会把普通回答也建成 Goal"
        );
        assert!(tool_policy.contains("选定 Workspace 准备开展实质性工作前"));
        assert!(
            tool_policy.contains("首个工具调用必须调用 `builtin.memory_search`"),
            "Workspace 首发召回必须是该工作阶段的首个工具调用"
        );
        assert!(tool_policy.contains("召回该工作区的 Profile、Preferences 与 Experiences"));
        assert!(tool_policy.contains("从 Home 启动未选定 Workspace 时禁止触发"));
        assert!(tool_policy.contains("旧 Workspace 记忆严禁跨项目沿用"));
        assert!(tool_policy.contains("记忆仅作为推演线索，不构成当轮事实证明"));
        assert!(tool_policy.contains("builtin.memory_write"));
        assert!(tool_policy.contains("必须在明确 Workspace 且经实际验证后方可写入"));

        assert!(tool_usage_guidance.contains("仅适用于已读取且持有有效 `[path#TAG]` 快照的文件"));
        assert!(
            tool_usage_guidance.contains("新建文件与整文件重写；目标文件已存在时按整文件重写处理")
        );
        assert!(tool_usage_guidance.contains(
            "适用于删除、重命名/移动文件、跨文件原子批处理，或快照已失效/过期、超出快照上限的场景"
        ));
        assert!(
            tool_usage_guidance
                .contains("修改已存在的单个文件优先 Hashline；整文件重写与新建用 `*** Add File:`")
        );
        assert!(tool_usage_guidance.contains(
            "严禁将 Python、Shell、Rust 等裸脚本指令写入 Update/Edit 块；执行系统命令一律走 `builtin.shell_command`"
        ));
        assert!(tool_usage_guidance.contains("必须是原生裸脚本"));
        assert!(
            tool_usage_guidance.contains("每次调用还必须给出 `intent`"),
            "命令意图的产品态承诺必须由稳定 developer 片段承载"
        );
        assert!(
            tool_usage_guidance.contains("它是这次执行面向用户的说明"),
            "intent 的产品态语义必须写明它是这次执行面向用户的说明"
        );
        assert!(
            tool_usage_guidance.contains("不得复述命令原文，也不得省略"),
            "intent 的必填与写法约束必须写明，模型才知道这句话不能省"
        );
        assert!(
            tool_usage_guidance
                .contains("在外层嵌套 `bash -c`、`zsh -c`、`powershell` 或 `cmd /c`"),
            "命令必须以裸脚本形态下发，不得再包一层 shell"
        );
        assert!(
            tool_usage_guidance.contains("**长耗时作业 (后台)**"),
            "执行形态判据必须点名后台长任务的入口"
        );
        assert!(
            tool_usage_guidance.contains("`builtin.job_output`, `builtin.job_kill`"),
            "后台作业必须点名读取与终止工具"
        );
        assert!(
            tool_usage_guidance.contains("**绝对互斥**，同时声明将被系统直接驳回"),
            "三种执行形态的判据必须写明互斥约束，否则模型会同时请求终端与后台作业"
        );
        assert!(
            tool_usage_guidance.contains("`job_id` 与 `session_id` 属于两条物理隔离的通道"),
            "两个 ID 通道必须显式隔离，否则模型会把 job_id 交给 write_stdin"
        );
        assert!(
            tool_usage_guidance.contains(
                "严禁将 `job_id` 传给 `write_stdin`，严禁将 `session_id` 传给 `job_output`"
            ),
            "通道隔离必须给出两个方向的禁止表述"
        );
        assert!(
            tool_usage_guidance.contains(
                "只要主线无需立即依赖该结果，一律 `run_in_background: true`，主线继续推进其他工作"
            ),
            "后台默认判据必须写明「不依赖结果即后台」，否则模型会习惯性前台阻塞"
        );
        assert!(
            tool_usage_guidance
                .contains("仅当下一步行动强依赖该结果时才 `run_in_background: false`"),
            "同步等待必须是显式例外，否则后台默认会被前台兜底架空"
        );
        assert!(
            tool_usage_guidance.contains("**严禁用 `builtin.job_output` 做 Busy-poll 轮询**"),
            "后台作业必须禁止轮询等待，结果由运行时回流"
        );
        assert!(
            tool_usage_guidance.contains("互不阻塞的多条长耗时命令应各自后台启动后再统一收口"),
            "长耗时命令必须并行后台启动，不得串行前台等待"
        );
        assert!(
            tool_usage_guidance.contains("必须强制调用该 MCP 工具"),
            "MCP 利用率取决于正向选型规则，规约必须给出领域命中时的强制判据"
        );
        assert!(
            tool_usage_guidance.contains("严禁因熟悉本机命令而绕过专用 MCP"),
            "必须禁止用本机命令绕过专用 MCP"
        );
        assert!(
            tool_usage_guidance
                .contains("工具名必须使用系统下发的完整全名（以系统注册名为准，含可能的哈希后缀）"),
            "工具名必须以下发全名为准，不得自行拼接"
        );
        assert!(
            tool_usage_guidance.contains("直接判定该能力当前不可用"),
            "无匹配时必须给出终止判据，避免模型反复重试"
        );
        assert!(
            tool_usage_guidance.contains(
                "`grep`、`rg`、`diff`、`test`、`cmp`、`git diff --quiet` 等命令返回非零通常代表“无匹配”或“判断为假”"
            ),
            "条件性豁免命令的非零退出码不得被当作失败"
        );
        assert!(
            tool_usage_guidance
                .contains("编译器、打包构建工具、测试框架、包管理器的非零退出码代表真实失败"),
            "真实失败必须写明，避免模型忽略编译与测试错误"
        );
        assert!(tool_usage_guidance.contains("严禁原样重复运行同一报错命令"));
        assert!(
            tool_usage_guidance.contains("回执正文仅为局部预览"),
            "截断回执不得被当作完整输出"
        );
        assert!(tool_usage_guidance.contains("据此宣称“无错误”或“验证通过”"));
        assert!(
            tool_usage_guidance
                .contains("`full_output_available: true` 时必须按指定路径读取完整输出"),
            "可读取完整输出时不得停留在预览"
        );
        assert!(tool_usage_guidance.contains("`timed_out: true` 表示被强制掐断而非执行完毕"));
        assert!(
            tool_usage_guidance.contains("仅表明进程未结束"),
            "Running 只表示进程未结束，不得当作成功"
        );
        assert!(
            tool_usage_guidance.contains("返回的日志是**增量片段**，不代表任务全貌"),
            "增量语义必须写明，否则模型会把单次读取当作全部输出"
        );
        assert!(
            tool_usage_guidance
                .contains("会话与作业在保留窗口期后会被自动回收；若提示超时回收，应重新发起执行"),
            "回收后的处置必须写明"
        );
        assert!(
            tool_usage_guidance
                .contains("说明文件版本已滞后，必须重新读取取得最新 `[path#TAG]` 后再重试"),
            "hashline 失配必须先重读再编辑"
        );
        assert!(tool_usage_guidance.contains("必须作为第一优先级就地处理"));
        assert!(tool_usage_guidance.contains("说明文件内容未读完，必须按偏移量继续读取"));

        assert!(
            !tool_policy.contains("必须强制调用该 MCP 工具"),
            "能力选型属通用规约，必须由 <tool_execution_and_receipt_protocol> 承载才能同时覆盖子代理"
        );

        assert!(
            !tool_policy.contains("回执正文仅为局部预览"),
            "回执解读属通用规约，必须由 <tool_execution_and_receipt_protocol> 承载才能同时覆盖子代理"
        );

        assert!(
            !tool_policy.contains("外部内容信任边界"),
            "信任边界必须由 <delegation-policy> 承载，才能同时覆盖子代理；不得重复写入 <tool_orchestration_protocol>"
        );

        assert!(
            tool_usage_guidance
                .contains("修改已存在的单个文件优先 Hashline；整文件重写与新建用 `*** Add File:`")
        );
        for phrase in [
            "改完即重锚",
            "**严禁复用编辑前的旧 TAG**",
            "纯新增一律用插入操作",
            "删除用 `DEL`",
            "锚点不要打在闭合定界符",
            "不要用编辑工具做格式化",
        ] {
            assert!(
                tool_usage_guidance.contains(phrase),
                "编辑操作规范「{phrase}」必须由 <tool_execution_and_receipt_protocol> 片段承载"
            );
        }
    }

    #[test]
    fn runtime_constraints_template_renders_optional_memory_lines() {
        let prompt = render_agent_runtime_constraints(&AgentRuntimeConstraintsTemplateInput {
            surface: "cli_run".to_owned(),
            selected_skill: "review".to_owned(),
            home_root: "C:/Users/test/.axon".to_owned(),
            workspace_root: "E:/zbg/axon-code".to_owned(),
            workspace_instructions: "AGENTS.md".to_owned(),
            capability_lines: vec!["client_fs_write=true".to_owned()],
            memory_tool_guidance: Some("memory-guidance: run fmt".to_owned()),
            memory_verify_candidate: None,
            memory_citation_protocol: Some("memory_citation_required=true".to_owned()),
        });

        assert!(!prompt.contains("[Axon Runtime Constraints]"));
        assert!(prompt.contains("<runtime_context>"));
        assert!(prompt.contains("当前入口：cli_run"));
        assert!(prompt.contains("个人 Home：C:/Users/test/.axon"));
        assert!(prompt.contains("当前工作地点（Workspace）：E:/zbg/axon-code"));
        assert!(prompt.contains("当前工作地点适用的 instruction 文件：AGENTS.md"));
        assert!(prompt.contains("memory_tool_guidance=memory-guidance: run fmt"));
        assert!(!prompt.contains("memory_verify_candidate="));
        assert!(prompt.contains("memory_citation_required=true"));
        assert!(!prompt.contains("<fast_path>"));
    }

    #[test]
    fn context_window_guidance_matches_notes_and_history_protocol() {
        let guidance = render_context_window_guidance();

        assert!(guidance.contains("builtin.notes_write_file"));
        assert!(guidance.contains("builtin.notes_append_to_file"));
        assert!(guidance.contains("builtin.get_context_remaining"));
        assert!(guidance.contains("builtin.new_context"));
        assert!(guidance.contains("builtin.history_read_item"));
        assert!(guidance.contains("builtin.history_list_items"));
        assert!(guidance.contains("builtin.history_search_contents"));
        assert!(guidance.contains("[id: "));
        assert!(guidance.contains("<memory_window_protocol>"));
        assert!(guidance.contains("严禁向用户提及"));
        assert!(guidance.contains("未写入会被系统直接拦截"));
        assert!(!guidance.contains("{{"));
    }
}
