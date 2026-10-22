//! Agent TUI typed submit 到 CLI runtime prompt 的投影。

use std::path::Path;

use crate::run_commands::{PreparedInteraction, prepare_interaction_from_prompt};
use axon_tui_runtime::{
    AgentTuiInputElement, AgentTuiInputElementKind, AgentTuiInputResult, AgentTuiQueuedAction,
    AgentTuiSubmit,
};
use uuid::Uuid;

pub(crate) const TUI_CHANNEL: &str = "tui";

/// Windows verbatim 路径前缀；`canonicalize` 会为其结果加上该前缀，
/// 而下游 Provider 与归档链路消费的是普通绝对路径。
const WINDOWS_VERBATIM_PREFIX: &str = r"\\?\";

/// 为当前 TUI 进程生成稳定的会话表面 ID。
///
/// 两个 TUI 进程属于两条主 turn 车道；同一进程内 submit、steer、恢复和完成
/// 收件箱必须复用同一个 destination。
#[must_use]
pub(crate) fn new_tui_reply_destination() -> String {
    format!("terminal-{}", Uuid::new_v4())
}

pub(super) fn interaction_from_tui_submit(
    submission: &AgentTuiSubmit,
    cwd: &Path,
    now_ms: u64,
    reply_destination: &str,
) -> Result<Option<PreparedInteraction>, String> {
    if !matches!(
        submission.input,
        AgentTuiInputResult::Submit { .. } | AgentTuiInputResult::Queued { .. }
    ) {
        return Ok(None);
    }
    let content = prompt_from_tui_submit(submission);
    if content.trim().is_empty() {
        return Ok(None);
    }
    let mut prepared =
        prepare_interaction_from_prompt(TUI_CHANNEL, reply_destination, &content, cwd, now_ms)?;
    // 附件与占位符绑定：图片在同一提示里会被 Provider 作为视觉输入内联，
    // 其他文件只提供只读引用路径，正文不进入提示词。
    prepared.interaction.attachments = attachment_paths(submission);
    Ok(Some(prepared))
}

/// 本次提交携带的附件绝对路径。
///
/// 按草稿顺序规范化去重：占位符可被重复粘贴，但同一条消息不应重复投递同一附件。
fn attachment_paths(submission: &AgentTuiSubmit) -> Vec<String> {
    let mut paths = Vec::new();
    for element in submission.input.elements() {
        let path = match &element.kind {
            AgentTuiInputElementKind::File { path } | AgentTuiInputElementKind::Image { path } => {
                path
            }
            AgentTuiInputElementKind::Mention { .. }
            | AgentTuiInputElementKind::LargePaste { .. } => continue,
        };
        let path = normalized_attachment_path(path);
        if path.is_empty() || paths.iter().any(|existing| existing == &path) {
            continue;
        }
        paths.push(path);
    }
    paths
}

fn normalized_attachment_path(path: &Path) -> String {
    let value = path.to_string_lossy();
    let trimmed = value
        .strip_prefix(WINDOWS_VERBATIM_PREFIX)
        .unwrap_or(value.as_ref());
    trimmed.trim().to_owned()
}

pub(super) fn prompt_from_tui_submit(submission: &AgentTuiSubmit) -> String {
    let mut prompt = match &submission.input {
        AgentTuiInputResult::Submit { text, .. }
        | AgentTuiInputResult::ControlCommand { text, .. } => text.clone(),
        AgentTuiInputResult::Queued { text, action, .. } => match action {
            AgentTuiQueuedAction::Plain => text.clone(),
            AgentTuiQueuedAction::RunShell => shell_prompt(text),
        },
    };
    append_tui_mentions(&mut prompt, submission.input.elements());
    prompt
}

fn shell_prompt(text: &str) -> String {
    text.strip_prefix('!').map_or_else(
        || text.to_owned(),
        |command| format!("请在本机 shell 中执行：{command}"),
    )
}

fn append_tui_mentions(prompt: &mut String, elements: &[AgentTuiInputElement]) {
    let mut lines = Vec::new();
    for element in elements {
        let AgentTuiInputElementKind::Mention {
            sigil,
            name,
            target,
        } = &element.kind
        else {
            continue;
        };
        let target = normalized_mention_target(target);
        match sigil {
            '$' if !prompt.contains(&format!("@skill:{name}")) => {
                lines.push(format!("@skill:{name}"));
            }
            '%' => lines.push(format!("已选择 connector：{target}")),
            '@' => lines.push(format!("已引用文件：{target}")),
            _ => {}
        }
    }
    if !lines.is_empty() {
        prompt.push_str("\n\n[结构化输入]\n");
        prompt.push_str(&lines.join("\n"));
    }
}

fn normalized_mention_target(value: &str) -> &str {
    value.split_once("://").map_or(value, |(_, target)| target)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use axon_tui_runtime::{
        AgentTuiInputElement, AgentTuiInputElementKind, AgentTuiInputResult, AgentTuiQueuedAction,
        AgentTuiRunOverrides, AgentTuiSubmit,
    };

    use super::{interaction_from_tui_submit, prompt_from_tui_submit};

    #[test]
    fn plain_submit_becomes_workspace_interaction() {
        let cwd = axon_test_support::test_temp_dir();
        let submission = submit(AgentTuiInputResult::Submit {
            text: "检查集成".to_owned(),
            elements: Vec::new(),
        });

        let prepared = interaction_from_tui_submit(&submission, cwd.path(), 42, "terminal-1")
            .expect("构造 Interaction")
            .expect("普通输入应进入 durable ingress");
        assert_eq!(
            prepared
                .interaction
                .scope
                .workspace_id()
                .map(|id| id.as_str()),
            Some(prepared.workspace.workspace_id.as_str())
        );
        let interaction = prepared.interaction;

        assert_eq!(interaction.source.channel(), "tui");
        assert_eq!(interaction.reply_route.channel(), "tui");
        assert_eq!(interaction.reply_route.destination(), "terminal-1");
        assert!(interaction.scope.workspace_id().is_some());
        assert_eq!(interaction.content, "检查集成");
        assert_eq!(interaction.received_at_ms, 42);
    }

    #[test]
    fn control_input_skips_interaction() {
        let submission = submit(AgentTuiInputResult::ControlCommand {
            command: "/status".to_owned(),
            args: String::new(),
            text: "/status".to_owned(),
        });
        let cwd = axon_test_support::test_temp_dir();
        assert!(
            interaction_from_tui_submit(&submission, cwd.path(), 42, "terminal-1")
                .expect("控制输入应跳过")
                .is_none()
        );
    }

    #[test]
    fn empty_submit_skips_interaction() {
        let submission = submit(AgentTuiInputResult::Submit {
            text: "   ".to_owned(),
            elements: Vec::new(),
        });
        let cwd = axon_test_support::test_temp_dir();

        assert!(
            interaction_from_tui_submit(&submission, cwd.path(), 42, "terminal-1")
                .expect("空输入应跳过")
                .is_none()
        );
    }

    #[test]
    fn popup_mentions_survive_prompt_projection() {
        let submission = submit(AgentTuiInputResult::Submit {
            text: "检查集成".to_owned(),
            elements: vec![
                mention('$', "review"),
                mention('%', "github"),
                mention('@', "README.md"),
            ],
        });

        let prompt = prompt_from_tui_submit(&submission);

        assert!(prompt.contains("@skill:review"));
        assert!(prompt.contains("已选择 connector：github"));
        assert!(prompt.contains("已引用文件：README.md"));
    }

    #[test]
    fn attachment_elements_reach_interaction_attachments() {
        let cwd = axon_test_support::test_temp_dir();
        let image = cwd.path().join("pasted.png");
        let file = cwd.path().join("report.txt");
        let submission = submit(AgentTuiInputResult::Submit {
            text: "分析 [Image #1] 与 [File #2] report.txt".to_owned(),
            elements: vec![image_element(&image), file_element(&file)],
        });

        let prepared = interaction_from_tui_submit(&submission, cwd.path(), 42, "terminal-1")
            .expect("构造 Interaction")
            .expect("带附件的普通输入应进入 durable ingress");

        assert_eq!(
            prepared.interaction.attachments,
            vec![image.display().to_string(), file.display().to_string(),]
        );
    }

    #[test]
    fn duplicated_attachment_elements_collapse_to_one_path() {
        let cwd = axon_test_support::test_temp_dir();
        let image = cwd.path().join("pasted.png");
        let submission = submit(AgentTuiInputResult::Submit {
            text: "分析图片".to_owned(),
            elements: vec![image_element(&image), image_element(&image)],
        });

        let prepared = interaction_from_tui_submit(&submission, cwd.path(), 42, "terminal-1")
            .expect("构造 Interaction")
            .expect("重复附件不应阻断提交");

        assert_eq!(
            prepared.interaction.attachments,
            vec![image.display().to_string()]
        );
    }

    #[test]
    fn queued_shell_action_projects_explicit_shell_intent() {
        let submission = submit(AgentTuiInputResult::Queued {
            text: "!cargo check -p axon-cli".to_owned(),
            elements: Vec::new(),
            action: AgentTuiQueuedAction::RunShell,
        });

        assert_eq!(
            prompt_from_tui_submit(&submission),
            "请在本机 shell 中执行：cargo check -p axon-cli"
        );
    }

    fn submit(input: AgentTuiInputResult) -> AgentTuiSubmit {
        AgentTuiSubmit {
            input,
            overrides: AgentTuiRunOverrides::default(),
            cwd: axon_test_support::test_temp_root(),
            question_answer: None,
        }
    }

    fn file_element(path: &Path) -> AgentTuiInputElement {
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        AgentTuiInputElement {
            placeholder: format!("[File #1] {name}"),
            kind: AgentTuiInputElementKind::File {
                path: path.to_path_buf(),
            },
        }
    }

    fn image_element(path: &Path) -> AgentTuiInputElement {
        AgentTuiInputElement {
            placeholder: "[Image #1]".to_owned(),
            kind: AgentTuiInputElementKind::Image {
                path: path.to_path_buf(),
            },
        }
    }

    fn mention(sigil: char, name: &str) -> AgentTuiInputElement {
        AgentTuiInputElement {
            placeholder: format!("{sigil}{name}"),
            kind: AgentTuiInputElementKind::Mention {
                sigil,
                name: name.to_owned(),
                target: mention_target(sigil, name),
            },
        }
    }

    fn mention_target(sigil: char, name: &str) -> String {
        let protocol = match sigil {
            '$' => "skill",
            '%' => "connector",
            '@' => "file",
            _ => "mention",
        };
        format!("{protocol}://{name}")
    }
}
