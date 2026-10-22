//! 用户级 Soul 文档加载与 prompt fragment 投影。

use std::{fs, io::ErrorKind, path::Path};

use axon_agent_runtime::{
    AgentModelMessage, PromptFragment, PromptFragmentMetadata, PromptFragmentPipeline,
    SESSION_STABLE_SOUL_END_MARKER, SESSION_STABLE_SOUL_START_MARKER,
};
use axon_config::ConfigLoader;

/// 用户级 Soul 文件名。
pub(crate) const USER_SOUL_FILE_NAME: &str = "SOUL.md";

/// 读取当前用户的 Soul，并投影为 session-stable 消息。
pub(crate) fn load_user_soul_message() -> Result<Option<AgentModelMessage>, String> {
    let root = ConfigLoader::default_user_axon_root_path()
        .map_err(|error| format!("无法解析用户 Axon 目录：{error:?}"))?;
    load_user_soul_message_from_root(&root)
}

fn load_user_soul_message_from_root(root: &Path) -> Result<Option<AgentModelMessage>, String> {
    let path = root.join(USER_SOUL_FILE_NAME);
    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("读取 Soul 文件失败（{}）：{error}", path.display())),
    };
    let Some(fragment) = SoulSystemFragment::optional(content) else {
        return Ok(None);
    };
    let mut pipeline = PromptFragmentPipeline::new();
    pipeline.push_fragment(Box::new(fragment));
    Ok(pipeline.into_messages().into_iter().next())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SoulSystemFragment {
    content: String,
}

impl SoulSystemFragment {
    fn optional(content: impl Into<String>) -> Option<Self> {
        let content = normalize_soul_content(&content.into());
        (!content.is_empty()).then_some(Self { content })
    }
}

impl PromptFragment for SoulSystemFragment {
    fn metadata(&self) -> PromptFragmentMetadata {
        PromptFragmentMetadata::stable_system()
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        (
            SESSION_STABLE_SOUL_START_MARKER,
            SESSION_STABLE_SOUL_END_MARKER,
        )
    }

    fn body(&self) -> String {
        format!("\n{}\n", self.content)
    }
}

fn normalize_soul_content(content: &str) -> String {
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
    fn missing_or_empty_soul_is_skipped() {
        let root = axon_test_support::test_temp_dir();
        assert_eq!(load_user_soul_message_from_root(root.path()).unwrap(), None);

        fs::write(root.path().join(USER_SOUL_FILE_NAME), " \r\n ").unwrap();
        assert_eq!(load_user_soul_message_from_root(root.path()).unwrap(), None);
    }

    #[test]
    fn soul_is_cacheable_and_normalizes_line_endings() {
        let root = axon_test_support::test_temp_dir();
        fs::write(
            root.path().join(USER_SOUL_FILE_NAME),
            "\r\n[SOUL_DEFINITION]\r\n- name: Axon\r\n[/SOUL_DEFINITION]\r\n",
        )
        .unwrap();

        let message = load_user_soul_message_from_root(root.path())
            .unwrap()
            .unwrap();

        assert_eq!(message.role, "system");
        assert!(!message.cache_excluded);
        assert_eq!(
            message.content,
            "<assistant-profile-context>\n[SOUL_DEFINITION]\n- name: Axon\n[/SOUL_DEFINITION]\n</assistant-profile-context>"
        );
    }

    #[test]
    fn soul_change_is_visible_on_the_next_load() {
        let root = axon_test_support::test_temp_dir();
        let path = root.path().join(USER_SOUL_FILE_NAME);
        fs::write(&path, "soul-v1").unwrap();
        let first = load_user_soul_message_from_root(root.path())
            .unwrap()
            .unwrap();

        fs::write(path, "soul-v2").unwrap();
        let second = load_user_soul_message_from_root(root.path())
            .unwrap()
            .unwrap();

        assert!(first.content.contains("soul-v1"));
        assert!(second.content.contains("soul-v2"));
        assert_ne!(first.content, second.content);
    }

    #[test]
    fn soul_preserves_full_content_beyond_legacy_file_budget() {
        let root = axon_test_support::test_temp_dir();
        let body = format!("SOUL-BEGIN\n{}\nSOUL-END", "人格规则".repeat(24_000));
        fs::write(root.path().join(USER_SOUL_FILE_NAME), &body).unwrap();

        let message = load_user_soul_message_from_root(root.path())
            .unwrap()
            .unwrap();

        assert_eq!(
            message.content,
            format!("{SESSION_STABLE_SOUL_START_MARKER}\n{body}\n{SESSION_STABLE_SOUL_END_MARKER}")
        );
    }
}
