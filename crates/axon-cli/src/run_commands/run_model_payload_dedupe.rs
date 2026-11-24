//! 注入块的内容级去重。
//!
//! 每轮重算的路由提示与线索块如果与历史里最后一份逐字节相同，再注入一次不会改变
//! 模型判断，只会让本轮新增的尾巴变长。判据只有一条：同标记注入块里最新的那一条
//! 是否与候选完全一致；一致即跳过，不一致（或从未出现过）才注入。

use axon_agent_runtime::{AgentModelMessage, PromptFragment};

/// 历史里最后一份同标记的注入块是否与候选 fragment 逐字节一致。
pub(crate) fn context_block_already_replayed(
    history: &[AgentModelMessage],
    fragment: &dyn PromptFragment,
) -> bool {
    let (start_marker, _) = fragment.markers();
    context_block_matches(history, start_marker, &fragment.render())
}

/// 过滤掉已在历史里逐字节出现过的那批 fragment。
pub(crate) fn filter_replayed_fragments(
    history: &[AgentModelMessage],
    fragments: Vec<Box<dyn PromptFragment>>,
) -> Vec<Box<dyn PromptFragment>> {
    fragments
        .into_iter()
        .filter(|fragment| !context_block_already_replayed(history, fragment.as_ref()))
        .collect()
}

/// 同上的消息版本：标记取候选内容的首行。
pub(crate) fn context_message_already_replayed(
    history: &[AgentModelMessage],
    message: &AgentModelMessage,
) -> bool {
    let marker = message
        .content
        .split('\n')
        .next()
        .unwrap_or_default()
        .trim()
        .to_owned();
    context_block_matches(history, &marker, &message.content)
}

fn context_block_matches(history: &[AgentModelMessage], marker: &str, candidate: &str) -> bool {
    if marker.is_empty() {
        return false;
    }
    history
        .iter()
        .rev()
        .find(|message| message.content.contains(marker))
        .is_some_and(|message| message.content.trim() == candidate.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn injected(content: &str) -> AgentModelMessage {
        AgentModelMessage::injected_context("developer", content)
    }

    /// 与历史里最新一份同标记块一致时不再重复注入。
    #[test]
    fn identical_block_is_skipped() {
        let history = vec![
            injected("<memory-thread-context>\n旧线索\n</memory-thread-context>"),
            injected("<memory-thread-context>\n周报\n</memory-thread-context>"),
        ];
        let candidate = injected("<memory-thread-context>\n周报\n</memory-thread-context>");
        assert!(context_message_already_replayed(&history, &candidate));
    }

    /// 内容变化时必须重新注入，判据只看最新一份。
    #[test]
    fn changed_block_is_replayed() {
        let history = vec![
            injected("<memory-thread-context>\n周报\n</memory-thread-context>"),
            injected("<memory-thread-context>\n周报收尾\n</memory-thread-context>"),
        ];
        let candidate = injected("<memory-thread-context>\n周报\n</memory-thread-context>");
        assert!(
            !context_message_already_replayed(&history, &candidate),
            "判据必须只看最新一份同标记块"
        );
    }

    /// 从未出现过的块必须注入。
    #[test]
    fn absent_block_is_replayed() {
        let history = vec![injected("<other-context>别的块</other-context>")];
        let candidate = injected("<memory-thread-context>\n周报\n</memory-thread-context>");
        assert!(!context_message_already_replayed(&history, &candidate));
    }

    /// fragment 路径：渲染结果与落库内容同形时才判定为已回放。
    #[test]
    fn fragment_path_compares_rendered_block() {
        let fragment = ProbeFragment::new("周报");
        let rendered = fragment.render();
        let history = vec![
            injected("<probe-context>\n旧内容\n</probe-context>"),
            injected(&rendered),
        ];
        assert!(context_block_already_replayed(&history, &fragment));
        assert!(!context_block_already_replayed(&[], &fragment));
    }

    #[derive(Debug)]
    struct ProbeFragment {
        content: String,
    }

    impl ProbeFragment {
        fn new(content: &str) -> Self {
            Self {
                content: content.to_owned(),
            }
        }
    }

    impl PromptFragment for ProbeFragment {
        fn metadata(&self) -> axon_agent_runtime::PromptFragmentMetadata {
            axon_agent_runtime::PromptFragmentMetadata::dynamic_unbudgeted_developer()
        }

        fn markers(&self) -> (&'static str, &'static str) {
            Self::type_markers()
        }

        fn type_markers() -> (&'static str, &'static str) {
            ("<probe-context>", "</probe-context>")
        }

        fn body(&self) -> String {
            format!("\n{}\n", self.content)
        }
    }
}
