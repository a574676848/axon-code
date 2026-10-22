//! 用户级 AGENTS.md 加载与独立 prompt fragment 投影。

use std::{fs, io::ErrorKind, path::Path};

use axon_agent_runtime::{
    AgentModelMessage, PromptFragment, PromptFragmentMetadata, PromptFragmentPipeline,
    SESSION_STABLE_USER_AGENTS_END_MARKER, SESSION_STABLE_USER_AGENTS_START_MARKER,
};
use axon_config::ConfigLoader;

/// 用户级 AGENTS 文件名。
pub(crate) const USER_AGENTS_FILE_NAME: &str = "AGENTS.md";

/// 读取用户级 AGENTS.md，并投影为独立 session-stable 消息。
pub(crate) fn load_user_agents_message() -> Result<Option<AgentModelMessage>, String> {
    let root = ConfigLoader::default_user_axon_root_path()
        .map_err(|error| format!("无法解析用户 Axon 目录：{error:?}"))?;
    load_user_agents_message_from_root(&root)
}

fn load_user_agents_message_from_root(root: &Path) -> Result<Option<AgentModelMessage>, String> {
    let path = root.join(USER_AGENTS_FILE_NAME);
    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "读取用户级 AGENTS.md 失败（{}）：{error}",
                path.display()
            ));
        }
    };
    let Some(fragment) = UserAgentsDeveloperFragment::optional(content) else {
        return Ok(None);
    };
    let mut pipeline = PromptFragmentPipeline::new();
    pipeline.push_fragment(Box::new(fragment));
    Ok(pipeline.into_messages().into_iter().next())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct UserAgentsDeveloperFragment {
    content: String,
}

impl UserAgentsDeveloperFragment {
    fn optional(content: impl Into<String>) -> Option<Self> {
        let content = normalize_user_agents_content(&content.into());
        (!content.is_empty()).then_some(Self { content })
    }
}

impl PromptFragment for UserAgentsDeveloperFragment {
    fn metadata(&self) -> PromptFragmentMetadata {
        PromptFragmentMetadata::stable_developer()
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        (
            SESSION_STABLE_USER_AGENTS_START_MARKER,
            SESSION_STABLE_USER_AGENTS_END_MARKER,
        )
    }

    fn body(&self) -> String {
        format!("\n{}\n", self.content)
    }
}

fn normalize_user_agents_content(content: &str) -> String {
    content
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_or_empty_user_agents_is_skipped() {
        let root = axon_test_support::test_temp_dir();
        assert_eq!(
            load_user_agents_message_from_root(root.path()).unwrap(),
            None
        );

        fs::write(root.path().join(USER_AGENTS_FILE_NAME), " \r\n ").unwrap();
        assert_eq!(
            load_user_agents_message_from_root(root.path()).unwrap(),
            None
        );
    }

    #[test]
    fn user_agents_are_cacheable_and_normalize_line_endings() {
        let root = axon_test_support::test_temp_dir();
        fs::write(
            root.path().join(USER_AGENTS_FILE_NAME),
            "\r\n[GLOBAL_RULES]\r\n- use evidence\r\n[/GLOBAL_RULES]\r\n",
        )
        .unwrap();

        let message = load_user_agents_message_from_root(root.path())
            .unwrap()
            .unwrap();

        assert_eq!(message.role, "developer");
        assert!(!message.cache_excluded);
        assert_eq!(
            message.content,
            "<user-agents-context>\n[GLOBAL_RULES]\n- use evidence\n[/GLOBAL_RULES]\n</user-agents-context>"
        );
    }

    #[test]
    fn user_agents_change_is_visible_on_the_next_load() {
        let root = axon_test_support::test_temp_dir();
        let path = root.path().join(USER_AGENTS_FILE_NAME);
        fs::write(&path, "global-v1").unwrap();
        let first = load_user_agents_message_from_root(root.path())
            .unwrap()
            .unwrap();

        fs::write(path, "global-v2").unwrap();
        let second = load_user_agents_message_from_root(root.path())
            .unwrap()
            .unwrap();

        assert!(first.content.contains("global-v1"));
        assert!(second.content.contains("global-v2"));
        assert_ne!(first.content, second.content);
    }

    #[test]
    fn user_agents_preserve_full_content_beyond_legacy_file_budget() {
        let root = axon_test_support::test_temp_dir();
        let body = format!("AGENTS-BEGIN\n{}\nAGENTS-END", "全局规则".repeat(24_000));
        fs::write(root.path().join(USER_AGENTS_FILE_NAME), &body).unwrap();

        let message = load_user_agents_message_from_root(root.path())
            .unwrap()
            .unwrap();

        assert_eq!(
            message.content,
            format!(
                "{SESSION_STABLE_USER_AGENTS_START_MARKER}\n{body}\n{SESSION_STABLE_USER_AGENTS_END_MARKER}"
            )
        );
    }
}
