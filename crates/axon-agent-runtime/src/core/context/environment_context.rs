//! 本轮模型调用的环境补充。
//!
//! 这里只承载尚未由其他上下文 section 提供的派生事实，避免重复注入
//! CWD、Workspace、Git、Pane 和运行约束。预算不足时按完整字段块省略，
//! 不截断字段内部内容。

use crate::AgentModelMessage;

const ENVIRONMENT_CONTEXT_HEADER: &str = "<environment-context>";
const ENVIRONMENT_CONTEXT_FOOTER: &str = "</environment-context>";
// 1024 bytes covers Windows extended executable paths, long terminal labels and a
// realistic model catalog line; values beyond that are treated as untrusted
// environment noise and omitted.
const MAX_ENVIRONMENT_VALUE_BYTES: usize = 1_024;
// 4096 bytes keeps the decision facts (model catalog and effort tiers) inside the
// block instead of dropping them while shell/time lines survive.
const MAX_ENVIRONMENT_CONTEXT_BYTES: usize = 4_096;

/// 当前 turn 的环境补充。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentContext {
    /// Unix epoch 毫秒时间戳。
    pub utc_now_ms: u64,
    /// 本地时间文本。
    pub local_time: Option<String>,
    /// 时区名称；无法取得时省略。
    pub timezone_name: Option<String>,
    /// 本地 UTC offset（分钟）。
    pub local_offset_minutes: Option<i32>,
    /// 操作系统标识。
    pub platform: String,
    /// CPU 架构标识。
    pub architecture: String,
    /// Axon 实际执行 `shell_command` 时采用的 Shell 摘要。
    pub shell: Option<String>,
    /// 受控的 Terminal 摘要。
    pub terminal: Option<String>,
    /// 本轮所属会话车道的渠道标识。
    ///
    /// 只给事实，不给用法：这些字段由技能与文档解释，避免提示词随命令面变化而失效。
    pub session_channel: Option<String>,
    /// 本轮所属会话车道的目标标识。
    pub session_destination: Option<String>,
    /// 当前会话所属的 Workspace。
    ///
    /// 会话车道的完整身份含 workspace 维度：同一个会话表面在不同项目下是不同的
    /// 车道。只给 channel 与 destination，调用方无法表达「往哪个项目的这条会话发」。
    pub session_workspace: Option<String>,
    /// 本机全局通知飞书会话；未配置时省略，仅作提示值。
    pub notification_chat_id: Option<String>,
    /// 可为子代理显式选型的模型名，已去重并保持配置顺序。
    ///
    /// 只给事实与用途：具体消费它的工具与参数名由工具描述解释，环境上下文不写死命令面。
    pub delegatable_models: Vec<String>,
    /// 运行时可接受的思考强度档位。
    ///
    /// 取 OpenAI 与 Anthropic 官方值的并集：单协议不支持的档位由 provider
    /// 协商层按协议上限收敛，因此这里给的是可请求值域而不是逐模型能力矩阵。
    pub reasoning_effort_tiers: Vec<String>,
}

/// 模型清单的用途说明。
///
/// 清单本身只是事实：用工具无关的话说清它能被怎么用，模型才知道这是可选项依据
/// 而不是背景噪音；具体调用哪个工具、参数叫什么，由技能与工具描述解释，
/// 环境上下文不写死命令面。
const DELEGATABLE_MODELS_NOTE: &str = "（当前可用模型；可根据任务场景与模型能力偏向选择对应的模型，在需要指定子代理模型的调用中传入，省略即继承当前模型）";
/// 思考强度档位的用途说明。
const REASONING_EFFORT_TIERS_NOTE: &str =
    "（当前可用思考强度档次；可根据任务难度选择对应档次随调用传入，省略即继承当前档位）";

impl EnvironmentContext {
    /// 渲染为本轮动态上下文消息（`source=Context` + `kind=InjectedContext`）。
    ///
    /// 环境随每轮重建，不能进缓存；用 `injected_context` 而不是 `cache_excluded`，
    /// 是为了让换窗与角色转换按 kind 识别注入，不必再依赖缓存标志。
    #[must_use]
    pub fn to_message(&self) -> AgentModelMessage {
        AgentModelMessage::injected_context("developer", self.render())
    }

    /// 渲染有界环境文本；超限字段按完整块省略，不截断字段内容。
    #[must_use]
    pub fn render(&self) -> String {
        let mut lines = vec![
            ENVIRONMENT_CONTEXT_HEADER.to_owned(),
            format!("utc_now_ms: {}", self.utc_now_ms),
        ];
        let footer = ENVIRONMENT_CONTEXT_FOOTER;
        append_complete_line(
            &mut lines,
            format_line("platform", Some(&self.platform)),
            footer,
        );
        append_complete_line(
            &mut lines,
            format_line("architecture", Some(&self.architecture)),
            footer,
        );
        // 会话事实排在时间与 shell 之前：预算不足时它们是最后才该被丢弃的信息。
        append_complete_line(
            &mut lines,
            format_line("session_channel", self.session_channel.as_deref()),
            footer,
        );
        append_complete_line(
            &mut lines,
            format_line("session_destination", self.session_destination.as_deref()),
            footer,
        );
        append_complete_line(
            &mut lines,
            format_line("session_workspace", self.session_workspace.as_deref()),
            footer,
        );
        append_complete_line(
            &mut lines,
            format_line("notification_chat_id", self.notification_chat_id.as_deref()),
            footer,
        );
        // 子代理选型事实紧贴会话事实：它们是模型本轮要做决策的输入，预算不足时
        // 应当比时间与 shell 更晚被丢弃。
        append_complete_line(
            &mut lines,
            format_list_line(
                "delegatable_models",
                &self.delegatable_models,
                DELEGATABLE_MODELS_NOTE,
            ),
            footer,
        );
        append_complete_line(
            &mut lines,
            format_list_line(
                "reasoning_effort_tiers",
                &self.reasoning_effort_tiers,
                REASONING_EFFORT_TIERS_NOTE,
            ),
            footer,
        );
        append_complete_line(
            &mut lines,
            format_line("local_time", self.local_time.as_deref()),
            footer,
        );
        append_complete_line(
            &mut lines,
            format_line("timezone_name", self.timezone_name.as_deref()),
            footer,
        );
        if let Some(offset) = self.local_offset_minutes {
            append_complete_line(
                &mut lines,
                Some(format!("local_offset_minutes: {offset}")),
                footer,
            );
        }
        append_complete_line(
            &mut lines,
            format_line("shell", self.shell.as_deref()),
            footer,
        );
        append_complete_line(
            &mut lines,
            format_line("terminal", self.terminal.as_deref()),
            footer,
        );
        lines.push(footer.to_owned());

        lines.join("\n")
    }
}

fn format_line(key: &str, value: Option<&str>) -> Option<String> {
    bounded_value(value?).map(|value| format!("{key}: {value}"))
}

/// 渲染有序清单行。
///
/// 超出单值上限时按完整项从尾部保留，并用 `…(+N)` 显式标注被省略的数量：
/// 模型名不会被截断成半个名字，清单也不会在模型不知情的情况下静默消失。
/// 用途说明永远保留在行尾，它才是这行事实被消费的方式。
fn format_list_line(key: &str, values: &[String], note: &str) -> Option<String> {
    let names = values
        .iter()
        .map(|value| sanitized_value(value))
        .filter(|value| !value.is_empty())
        .collect::<Vec<String>>();
    if names.is_empty() {
        return None;
    }
    let total = names.len();
    let mut kept = names;
    loop {
        let omitted = total.saturating_sub(kept.len());
        let mut line = format!("{key}: {}", kept.join(", "));
        if omitted > 0 {
            line.push_str(&format!(" …(+{omitted})"));
        }
        line.push_str(note);
        if line.len() <= MAX_ENVIRONMENT_VALUE_BYTES {
            return Some(line);
        }
        if kept.len() <= 1 {
            return None;
        }
        kept.pop();
    }
}

fn append_complete_line(lines: &mut Vec<String>, line: Option<String>, footer: &str) {
    let Some(line) = line else {
        return;
    };
    let current_bytes = lines.join("\n").len();
    let candidate_bytes = current_bytes + 1 + line.len() + 1 + footer.len();
    if candidate_bytes <= MAX_ENVIRONMENT_CONTEXT_BYTES {
        lines.push(line);
    }
}

fn bounded_value(value: &str) -> Option<String> {
    let trimmed = sanitized_value(value);
    (!trimmed.is_empty() && trimmed.len() <= MAX_ENVIRONMENT_VALUE_BYTES).then_some(trimmed)
}

/// 把控制字符替换为空格并裁剪首尾空白。
fn sanitized_value(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::EnvironmentContext;

    fn context() -> EnvironmentContext {
        EnvironmentContext {
            utc_now_ms: 1_757_600_000_000,
            local_time: Some("2026-09-12 10:00:00".to_owned()),
            timezone_name: Some("Asia/Shanghai".to_owned()),
            local_offset_minutes: Some(480),
            platform: "windows".to_owned(),
            architecture: "x86_64".to_owned(),
            shell: Some(
                "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe".to_owned(),
            ),
            terminal: Some("WindowsTerminal".to_owned()),
            session_channel: Some("desktop".to_owned()),
            session_destination: Some("workbench-1".to_owned()),
            session_workspace: Some("workspace-1".to_owned()),
            notification_chat_id: Some("oc_notify".to_owned()),
            delegatable_models: vec!["gpt-6-astra".to_owned(), "claude-opus-5-5".to_owned()],
            reasoning_effort_tiers: vec![
                "none".to_owned(),
                "minimal".to_owned(),
                "low".to_owned(),
                "medium".to_owned(),
                "high".to_owned(),
                "xhigh".to_owned(),
                "max".to_owned(),
            ],
        }
    }

    #[test]
    fn environment_message_is_dynamic_and_bounded() {
        let message = context().to_message();
        assert_eq!(message.role, "developer");
        assert!(message.cache_excluded);
        assert!(message.content.starts_with("<environment-context>"));
        assert!(message.content.contains("platform: windows"));
        assert!(message.content.len() <= 4_096);
    }

    #[test]
    fn control_characters_and_long_values_are_sanitized() {
        let mut value = context();
        value.shell = Some(format!("bad\n{}", "x".repeat(300)));
        let rendered = value.render();
        assert!(rendered.len() <= 4_096);
        assert!(rendered.contains("shell: bad "));
        assert!(!rendered.contains("bad\n"));
        assert!(rendered.ends_with("</environment-context>"));
        assert!(rendered.contains("session_channel: desktop"));
        assert!(rendered.contains("session_destination: workbench-1"));
        assert!(rendered.contains("notification_chat_id: oc_notify"));
    }

    /// 子代理选型事实必须是纯字段行，并排在时间与 shell 之前。
    #[test]
    fn subagent_selection_facts_precede_time_and_shell() {
        let rendered = context().render();
        assert!(rendered.contains("delegatable_models: gpt-6-astra, claude-opus-5-5"));
        assert!(
            rendered
                .contains("reasoning_effort_tiers: none, minimal, low, medium, high, xhigh, max")
        );
        // 事实必须自带用途，但不得写死工具名与参数名。
        assert!(rendered.contains("可根据任务场景与模型能力偏向选择对应的模型"));
        assert!(rendered.contains("可根据任务难度选择对应档次"));
        assert!(rendered.contains("省略即继承当前模型"));
        assert!(rendered.contains("省略即继承当前档位"));
        assert!(!rendered.contains("spawn_agent"));
        assert!(!rendered.contains("delegate_to_fast"));
        let models = rendered.find("delegatable_models:").unwrap();
        let time = rendered.find("local_time:").unwrap();
        let shell = rendered.find("shell:").unwrap();
        assert!(models < time && time < shell);
    }

    /// 清单超长时保留完整项并显式标注省略数量，绝不出现半个模型名。
    #[test]
    fn oversized_model_catalog_keeps_complete_names_and_reports_the_omitted_count() {
        let mut value = context();
        value.delegatable_models = (0..40)
            .map(|index| format!("model-with-a-long-routing-key-{index:02}"))
            .collect();
        let rendered = value.render();
        let line = rendered
            .lines()
            .find(|line| line.starts_with("delegatable_models: "))
            .expect("模型清单行必须保留");
        assert!(line.len() <= 1_024);
        assert!(line.contains("…(+"));
        // 截断只作用于清单，用途说明必须完整保留。
        assert!(line.ends_with("省略即继承当前模型）"));
        let listed = line
            .trim_start_matches("delegatable_models: ")
            .split(" …(+")
            .next()
            .expect("清单正文必须存在");
        for name in listed.split(", ") {
            assert!(
                value.delegatable_models.iter().any(|model| model == name),
                "被保留的项必须是完整模型名：{name}"
            );
        }
    }

    #[test]
    fn session_facts_are_emitted_as_plain_fields() {
        let rendered = context().render();
        // 只输出事实字段，不写死任何命令示例：用法由技能与文档解释。
        assert!(rendered.contains("session_channel: desktop"));
        assert!(!rendered.contains("axon cron"));
    }

    #[test]
    fn optional_fields_can_be_omitted() {
        let mut value = context();
        value.local_time = None;
        value.timezone_name = None;
        value.local_offset_minutes = None;
        value.shell = None;
        value.terminal = None;
        value.session_channel = None;
        value.session_destination = None;
        value.session_workspace = None;
        value.notification_chat_id = None;
        value.delegatable_models = Vec::new();
        value.reasoning_effort_tiers = Vec::new();
        let rendered = value.render();
        assert!(!rendered.contains("session_channel:"));
        assert!(!rendered.contains("local_time:"));
        assert!(!rendered.contains("shell:"));
        assert!(!rendered.contains("delegatable_models:"));
        assert!(!rendered.contains("reasoning_effort_tiers:"));
    }

    #[test]
    fn realistic_windows_environment_keeps_all_complete_blocks_under_budget() {
        let mut value = context();
        value.shell = Some(format!("C:\\{}\\powershell.exe", "a".repeat(240)));
        value.terminal = Some(format!("WindowsTerminal-{}", "x".repeat(96)));
        value.timezone_name = Some(format!("Asia/Shanghai-{}", "中".repeat(16)));
        let rendered = value.render();

        assert!(rendered.len() <= 4_096);
        assert!(rendered.contains("shell: C:\\"));
        assert!(rendered.contains("terminal: WindowsTerminal-"));
        assert!(rendered.contains("timezone_name: Asia/Shanghai-"));
        assert!(rendered.ends_with("</environment-context>"));
    }

    #[test]
    fn realistic_wsl_environment_keeps_all_complete_blocks_under_budget() {
        let value = EnvironmentContext {
            utc_now_ms: 1_757_600_000_000,
            local_time: Some("2026-09-12 10:20:07".to_owned()),
            timezone_name: Some("CST".to_owned()),
            local_offset_minutes: Some(480),
            platform: "linux".to_owned(),
            architecture: "x86_64".to_owned(),
            shell: Some("/bin/bash".to_owned()),
            terminal: Some("xterm-256color".to_owned()),
            session_channel: None,
            session_destination: None,
            session_workspace: None,
            notification_chat_id: None,
            delegatable_models: Vec::new(),
            reasoning_effort_tiers: Vec::new(),
        };
        let rendered = value.render();

        assert!(rendered.len() <= 4_096);
        assert!(rendered.contains("platform: linux"));
        assert!(rendered.contains("shell: /bin/bash"));
        assert!(rendered.contains("terminal: xterm-256color"));
        assert!(rendered.ends_with("</environment-context>"));
    }

    #[test]
    fn oversized_environment_value_is_removed_as_a_whole_block() {
        let mut value = context();
        value.shell = Some("x".repeat(1_025));
        let rendered = value.render();

        assert!(!rendered.contains("shell:"));
        assert!(!rendered.contains(&"x".repeat(128)));
        assert!(rendered.ends_with("</environment-context>"));
    }
}
