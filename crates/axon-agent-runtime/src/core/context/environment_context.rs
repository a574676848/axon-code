//! 本轮模型调用的环境补充。
//!
//! 这里只承载尚未由其他上下文 section 提供的派生事实，避免重复注入
//! CWD、Workspace、Git、Pane 和运行约束。预算不足时按完整字段块省略，
//! 不截断字段内部内容。

use crate::AgentModelMessage;

const ENVIRONMENT_CONTEXT_HEADER: &str = "<environment-context>";
const ENVIRONMENT_CONTEXT_FOOTER: &str = "</environment-context>";
// 512 bytes covers Windows extended executable paths and long terminal labels;
// values beyond that are treated as untrusted environment noise and omitted.
const MAX_ENVIRONMENT_VALUE_BYTES: usize = 512;
const MAX_ENVIRONMENT_CONTEXT_BYTES: usize = 1_536;

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
    /// 本机全局通知飞书会话；未配置时省略，仅作提示值。
    pub notification_chat_id: Option<String>,
}

impl EnvironmentContext {
    /// 渲染为 history 后的动态开发者消息。
    #[must_use]
    pub fn to_message(&self) -> AgentModelMessage {
        AgentModelMessage::cache_excluded("developer", self.render())
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
            format_line("notification_chat_id", self.notification_chat_id.as_deref()),
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
    let sanitized = value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    let trimmed = sanitized.trim();
    (!trimmed.is_empty() && trimmed.len() <= MAX_ENVIRONMENT_VALUE_BYTES)
        .then_some(trimmed.to_owned())
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
            notification_chat_id: Some("oc_notify".to_owned()),
        }
    }

    #[test]
    fn environment_message_is_dynamic_and_bounded() {
        let message = context().to_message();
        assert_eq!(message.role, "developer");
        assert!(message.cache_excluded);
        assert!(message.content.starts_with("<environment-context>"));
        assert!(message.content.contains("platform: windows"));
        assert!(message.content.len() <= 1_536);
    }

    #[test]
    fn control_characters_and_long_values_are_sanitized() {
        let mut value = context();
        value.shell = Some(format!("bad\n{}", "x".repeat(300)));
        let rendered = value.render();
        assert!(rendered.len() <= 1_536);
        assert!(rendered.contains("shell: bad "));
        assert!(!rendered.contains("bad\n"));
        assert!(rendered.ends_with("</environment-context>"));
        assert!(rendered.contains("session_channel: desktop"));
        assert!(rendered.contains("session_destination: workbench-1"));
        assert!(rendered.contains("notification_chat_id: oc_notify"));
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
        value.notification_chat_id = None;
        let rendered = value.render();
        assert!(!rendered.contains("session_channel:"));
        assert!(!rendered.contains("local_time:"));
        assert!(!rendered.contains("shell:"));
    }

    #[test]
    fn realistic_windows_environment_keeps_all_complete_blocks_under_budget() {
        let mut value = context();
        value.shell = Some(format!("C:\\{}\\powershell.exe", "a".repeat(240)));
        value.terminal = Some(format!("WindowsTerminal-{}", "x".repeat(96)));
        value.timezone_name = Some(format!("Asia/Shanghai-{}", "中".repeat(16)));
        let rendered = value.render();

        assert!(rendered.len() <= 1_536);
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
            notification_chat_id: None,
        };
        let rendered = value.render();

        assert!(rendered.len() <= 1_536);
        assert!(rendered.contains("platform: linux"));
        assert!(rendered.contains("shell: /bin/bash"));
        assert!(rendered.contains("terminal: xterm-256color"));
        assert!(rendered.ends_with("</environment-context>"));
    }

    #[test]
    fn oversized_environment_value_is_removed_as_a_whole_block() {
        let mut value = context();
        value.shell = Some("x".repeat(513));
        let rendered = value.render();

        assert!(!rendered.contains("shell:"));
        assert!(!rendered.contains(&"x".repeat(128)));
        assert!(rendered.ends_with("</environment-context>"));
    }
}
