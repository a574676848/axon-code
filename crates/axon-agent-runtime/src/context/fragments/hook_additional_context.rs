//! Hook additional context fragment。

use axon_context_fragments::PromptFragment;

/// Hook additional context fragment。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookAdditionalContextFragment {
    /// Hook 名称。
    pub hook_name: String,
    /// 额外上下文内容。
    pub content: String,
}

impl HookAdditionalContextFragment {
    /// 创建新的 hook additional context fragment。
    #[must_use]
    pub fn new(hook_name: String, content: String) -> Self {
        Self { hook_name, content }
    }
}

impl PromptFragment for HookAdditionalContextFragment {
    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        ("<hook-additional-context>", "</hook-additional-context>")
    }

    fn body(&self) -> String {
        format!(
            "\n<hook name=\"{}\">\n{}\n</hook>\n",
            escape_xml_attr(&self.hook_name),
            self.content.trim()
        )
    }
}

fn escape_xml_attr(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_additional_context_uses_stable_markers() {
        let fragment =
            HookAdditionalContextFragment::new("post\"apply".to_owned(), "ctx".to_owned());

        let rendered = fragment.render();

        assert!(rendered.starts_with("<hook-additional-context>"));
        assert!(rendered.ends_with("</hook-additional-context>"));
        assert!(rendered.contains("name=\"post&quot;apply\""));
    }
}
