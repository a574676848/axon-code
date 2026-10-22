//! Markdown 代码块类型与活动状态。

use pulldown_cmark::CodeBlockKind;

use super::transcript_markdown_helpers::code_block_language_token;

const INDENTED_CODE_BLOCK_PREFIX: &str = "    ";
const MERMAID_LANGUAGE: &str = "mermaid";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CodeBlockFlavor {
    Indented,
    Mermaid,
    Fenced { language: Option<String> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CodeBlockState {
    flavor: CodeBlockFlavor,
    line_started: bool,
}

impl CodeBlockState {
    pub(super) fn from_kind(kind: CodeBlockKind<'_>) -> Self {
        let flavor = match kind {
            CodeBlockKind::Indented => CodeBlockFlavor::Indented,
            CodeBlockKind::Fenced(info) => fenced_flavor(info.as_ref()),
        };
        Self {
            flavor,
            line_started: false,
        }
    }

    pub(super) const fn line_started(&self) -> bool {
        self.line_started
    }

    pub(super) const fn mark_line_started(&mut self) {
        self.line_started = true;
    }

    pub(super) fn language(&self) -> Option<&str> {
        match &self.flavor {
            CodeBlockFlavor::Fenced { language } => language.as_deref(),
            CodeBlockFlavor::Indented | CodeBlockFlavor::Mermaid => None,
        }
    }

    pub(super) const fn line_prefix(&self) -> &'static str {
        match &self.flavor {
            CodeBlockFlavor::Indented => INDENTED_CODE_BLOCK_PREFIX,
            CodeBlockFlavor::Mermaid | CodeBlockFlavor::Fenced { .. } => "",
        }
    }
}

fn fenced_flavor(info: &str) -> CodeBlockFlavor {
    let language = code_block_language_token(info).map(str::to_owned);
    if language
        .as_deref()
        .is_some_and(|language| language.eq_ignore_ascii_case(MERMAID_LANGUAGE))
    {
        CodeBlockFlavor::Mermaid
    } else {
        CodeBlockFlavor::Fenced { language }
    }
}

#[cfg(test)]
mod tests {
    use pulldown_cmark::CowStr;

    use super::{CodeBlockFlavor, CodeBlockState};

    #[test]
    fn recognizes_mermaid_fence_without_syntax_language_lookup() {
        let state = CodeBlockState::from_kind(pulldown_cmark::CodeBlockKind::Fenced(
            CowStr::Borrowed("Mermaid title=domain"),
        ));

        assert_eq!(state.flavor, CodeBlockFlavor::Mermaid);
        assert_eq!(state.language(), None);
        assert_eq!(state.line_prefix(), "");
    }
}
